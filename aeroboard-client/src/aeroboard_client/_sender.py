"""Background delivery of telemetry batches to the Aeroboard Flight server.

Training code must never wait on the network, and a server outage must neither
slow training down nor grow memory without bound. ``BatchSender`` owns a worker
thread that sends queued batches one at a time, in order:

* Transient failures (server down, timeouts) are retried with exponential
  backoff. While the server is unreachable new batches keep queueing up to a
  byte/batch budget; beyond it the *oldest* batches are dropped and counted.
* Batches the server rejects as invalid are dropped right away: resending the
  same bytes cannot succeed.
* ``wait_idle`` lets ``flush()`` block until everything queued so far was
  acknowledged, without blocking when the server is known to be unreachable.
"""

from __future__ import annotations

import logging
import threading
import time
from collections import deque
from dataclasses import dataclass
from typing import Callable, Deque, Optional

import pyarrow as pa
import pyarrow.flight as fl

PutFn = Callable[[pa.RecordBatch, pa.Schema, bytes], None]
ErrorFn = Callable[[str], None]

DEFAULT_MAX_QUEUE_BYTES = 256 * 1024 * 1024
DEFAULT_MAX_QUEUE_BATCHES = 1000

# The server answers INVALID_ARGUMENT for malformed batches; pyarrow surfaces it as these.
_PERMANENT_ERRORS = (TypeError, ValueError, pa.ArrowInvalid, pa.ArrowTypeError, fl.FlightUnauthenticatedError, fl.FlightUnauthorizedError)


@dataclass
class _Item:
    batch: pa.RecordBatch
    schema: pa.Schema
    path: bytes
    size: int


class BatchSender:
    def __init__(
        self,
        put: PutFn,
        *,
        on_error: ErrorFn,
        max_queue_bytes: int = DEFAULT_MAX_QUEUE_BYTES,
        max_queue_batches: int = DEFAULT_MAX_QUEUE_BATCHES,
        initial_retry_delay: float = 0.5,
        max_retry_delay: float = 10.0,
        put_many=None,
    ) -> None:
        self._put = put
        self._put_many=put_many
        self._on_error = on_error
        self._max_queue_bytes = max_queue_bytes
        self._max_queue_batches = max(1, max_queue_batches)
        self._initial_retry_delay = initial_retry_delay
        self._max_retry_delay = max_retry_delay

        self._queue: Deque[_Item] = deque()
        self._queued_bytes = 0
        self._cond = threading.Condition()
        self._in_flight = False
        self._failing = False
        self._closing = False
        self._abandon = False
        # Set when close() gives up; interrupts the retry backoff (which must not be
        # woken by new submissions, or an outage would turn into a busy loop).
        self._abandon_event = threading.Event()
        self._dropped_rows = 0

        self._thread = threading.Thread(target=self._run, name="aeroboard-sender", daemon=True)
        self._thread.start()

    # -- producer side -----------------------------------------------------

    def submit(self, batch: pa.RecordBatch, schema: pa.Schema, path: bytes) -> None:
        """Queue a batch for delivery. Never blocks on the network."""
        item = _Item(batch, schema, path, batch.nbytes)
        with self._cond:
            if self._closing:
                raise RuntimeError("Aeroboard client is closed")
            self._queue.append(item)
            self._queued_bytes += item.size
            self._enforce_budget()
            self._cond.notify_all()

    def wait_idle(self, timeout: Optional[float]) -> bool:
        """Wait until every queued batch was delivered.

        Returns immediately (``False``) while the server is unreachable, so a
        ``flush()`` in the training loop doesn't stall for the retry timeout.
        """
        deadline = None if timeout is None else time.monotonic() + timeout
        with self._cond:
            while self._queue or self._in_flight:
                if self._failing:
                    return False
                remaining = None if deadline is None else deadline - time.monotonic()
                if remaining is not None and remaining <= 0:
                    return False
                self._cond.wait(remaining)
            return True

    def close(self, timeout: float) -> bool:
        """Deliver what is queued (up to ``timeout`` seconds), then stop the thread.

        Returns ``False`` if batches had to be abandoned.
        """
        deadline = time.monotonic() + timeout
        with self._cond:
            if self._closing and not self._thread.is_alive():
                return not self._queue
            self._closing = True
            self._cond.notify_all()
            while self._queue or self._in_flight:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                self._cond.wait(remaining)
            delivered = not self._queue and not self._in_flight
            if not delivered:
                self._abandon = True
                abandoned = sum(item.batch.num_rows for item in self._queue)
                self._queue.clear()
                self._queued_bytes = 0
                self._abandon_event.set()
                self._cond.notify_all()
        if not delivered:
            self._on_error(
                f"[Aeroboard] Gave up on {abandoned} queued row(s) at shutdown: the server did not "
                f"accept them within {timeout:.0f}s."
            )
        self._thread.join(timeout=max(0.0, deadline - time.monotonic()) + 1.0)
        return delivered

    @property
    def dropped_rows(self) -> int:
        """Rows discarded because the queue budget was exceeded or the server rejected them."""
        with self._cond:
            return self._dropped_rows

    @property
    def pending_batches(self) -> int:
        with self._cond:
            return len(self._queue) + int(self._in_flight)

    # -- worker side ---------------------------------------------------------

    def _enforce_budget(self) -> None:
        """Drop the oldest queued batches beyond the budget (caller holds the lock)."""
        dropped = 0
        while len(self._queue) > 1 and (
            len(self._queue) > self._max_queue_batches or self._queued_bytes > self._max_queue_bytes
        ):
            oldest = self._queue.popleft()
            self._queued_bytes -= oldest.size
            dropped += oldest.batch.num_rows
        if dropped:
            first = self._dropped_rows == 0
            self._dropped_rows += dropped
            if first:
                self._on_error(
                    "[Aeroboard] Telemetry queue is full (server unreachable or too slow); dropping the "
                    "oldest data. Further drops are counted silently."
                )

    def _run(self) -> None:
        delay = self._initial_retry_delay
        while True:
            with self._cond:
                while not self._queue and not self._closing:
                    self._cond.wait()
                if not self._queue or self._abandon:
                    return
                item = self._queue.popleft()
                self._queued_bytes -= item.size
                items=[item]
                if self._put_many is not None:
                    while self._queue and len(items)<16 and sum(entry.size for entry in items)<8*1024*1024:
                        candidate=self._queue[0]
                        if candidate.path!=item.path or not candidate.schema.equals(item.schema):break
                        items.append(self._queue.popleft());self._queued_bytes-=candidate.size
                self._in_flight = True

            try:
                from ._protocol import prepare_batch
                for entry in items:
                    entry.batch=prepare_batch(entry.batch);entry.schema=entry.batch.schema
                if len(items)>1:self._put_many([(entry.batch,entry.schema,entry.path) for entry in items])
                else:self._put(item.batch, item.schema, item.path)
            except _PERMANENT_ERRORS as exc:
                with self._cond:
                    self._in_flight = False
                    self._dropped_rows += sum(entry.batch.num_rows for entry in items)
                    self._cond.notify_all()
                self._on_error(f"[Aeroboard] Server rejected a {item.path.decode()} batch; dropping it: {exc}")
                continue
            except Exception as exc:  # noqa: BLE001 - any transport error is retried
                with self._cond:
                    self._in_flight = False
                    newly_failing = not self._failing
                    self._failing = True
                    if not self._abandon:
                        # Keep ordering: the failed batch goes back to the front.
                        for entry in reversed(items):self._queue.appendleft(entry);self._queued_bytes+=entry.size
                        self._enforce_budget()
                    self._cond.notify_all()
                if newly_failing:
                    self._on_error(f"[Aeroboard] Cannot reach the server; will keep retrying in the background: {exc}")
                if self._abandon_event.wait(delay):
                    return
                delay = min(delay * 2, self._max_retry_delay)
                continue

            with self._cond:
                self._in_flight = False
                recovered = self._failing
                self._failing = False
                self._cond.notify_all()
            if recovered:
                logging.getLogger(__name__).info("Connection to the server restored; resuming delivery.")
            delay = self._initial_retry_delay
