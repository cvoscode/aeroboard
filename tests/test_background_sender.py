"""Background delivery: training code never blocks on, or crashes from, the network."""

import threading
import time

import pyarrow as pa
import pyarrow.flight as fl
import pytest
from unittest.mock import patch

from aeroboard_client._sender import BatchSender
from aeroboard_client.client import AeroboardClient

from test_logging_payloads import DummyFlightClient

pytestmark = pytest.mark.legacy_server


def _batch(rows=1):
    return pa.RecordBatch.from_pydict({"x": list(range(rows))})


class FlakyPut:
    """Fails the first `failures` calls with a transport error, then succeeds."""

    def __init__(self, failures=0, error=None):
        self.failures = failures
        self.error = error or fl.FlightUnavailableError("server down")
        self.delivered = []
        self.attempts = 0
        self.lock = threading.Lock()

    def __call__(self, batch, schema, path):
        with self.lock:
            self.attempts += 1
            if self.attempts <= self.failures:
                raise self.error
            self.delivered.append((path, batch.num_rows))


def test_transient_failures_are_retried_in_order():
    put = FlakyPut(failures=2)
    errors = []
    sender = BatchSender(put, on_error=errors.append, initial_retry_delay=0.01, max_retry_delay=0.02)
    sender.submit(_batch(1), None, b"scalars")
    sender.submit(_batch(2), None, b"images")

    deadline = time.monotonic() + 5
    while sender.pending_batches and time.monotonic() < deadline:
        time.sleep(0.01)

    assert put.delivered == [(b"scalars", 1), (b"images", 2)]
    assert len(errors) == 1, "an outage is reported once, not once per retry"
    assert sender.close(timeout=1)


def test_rejected_batches_are_dropped_not_retried():
    put = FlakyPut(failures=1, error=pa.ArrowInvalid("value has type Utf8"))
    errors = []
    sender = BatchSender(put, on_error=errors.append, initial_retry_delay=0.01)
    sender.submit(_batch(3), None, b"scalars")
    sender.submit(_batch(1), None, b"scalars")
    assert sender.wait_idle(timeout=5)

    assert put.attempts == 2
    assert put.delivered == [(b"scalars", 1)]
    assert sender.dropped_rows == 3
    assert "rejected" in errors[0]
    sender.close(timeout=1)


def test_queue_is_bounded_while_server_is_down_and_flush_does_not_block():
    put = FlakyPut(failures=10**9)
    errors = []
    sender = BatchSender(put, on_error=errors.append, max_queue_batches=3, initial_retry_delay=0.05, max_retry_delay=0.05)
    for _ in range(10):
        sender.submit(_batch(1), None, b"scalars")

    deadline = time.monotonic() + 2
    while not errors and time.monotonic() < deadline:
        time.sleep(0.01)
    started = time.monotonic()
    assert sender.wait_idle(timeout=5) is False
    assert time.monotonic() - started < 1, "flush must not stall training while the server is down"
    assert sender.pending_batches <= 3
    assert sender.dropped_rows >= 7

    started = time.monotonic()
    assert sender.close(timeout=0.2) is False
    assert time.monotonic() - started < 2
    assert any("Gave up" in message for message in errors)


def test_client_logging_survives_server_outage():
    class DownFlightClient(DummyFlightClient):
        def do_put(self, descriptor, schema, options=None):
            raise fl.FlightUnavailableError("connection refused")

    with patch("aeroboard_client.client.fl.FlightClient", DownFlightClient), \
            patch.object(AeroboardClient, "_resolve_run_id", lambda self, base: base):
        client = AeroboardClient(run_id="offline", batch_size=2, flush_timeout=0.5)
        started = time.monotonic()
        for step in range(200):
            client.log({"loss": 1.0 / (step + 1)}, step=step)
        client.flush()
        assert time.monotonic() - started < 2, "logging must not wait on the network"
        assert client._scalar_buffer == []
        client.close(timeout=0.2)
        assert client.had_error()


def test_image_encoding_runs_on_sender_and_snapshots_mutable_inputs(monkeypatch):
    import threading
    import aeroboard_client.client as module
    from test_logging_payloads import DummyFlightClient
    entered,release=threading.Event(),threading.Event()
    caller=threading.get_ident();workers=[]
    def encode(image,**kwargs):
        workers.append(threading.get_ident());entered.set();assert release.wait(2)
        assert image==b'original'
        return image,'png'
    monkeypatch.setattr(module.fl,'FlightClient',DummyFlightClient)
    monkeypatch.setattr(module.AeroboardClient,'_resolve_run_id',lambda self,base:base)
    monkeypatch.setattr(module,'_coerce_image_bytes',encode)
    client=module.AeroboardClient('image-thread',batch_size=1)
    try:
        value=bytearray(b'original');client.log_image('image',value);value[:]=b'modified'
        assert entered.wait(1);assert workers==[workers[0]] and workers[0]!=caller
        release.set();assert client.flush()
    finally:release.set();client.close()
