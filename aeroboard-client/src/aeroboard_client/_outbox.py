"""Crash-recoverable local delivery queue. SQLite FULL commits define durability.

A local writer and a separate network worker keep disk capture progressing while
the server is offline. Acknowledged items are removed; rejected items remain for
inspection. An OS lock prevents two processes from draining one outbox.
"""
from __future__ import annotations

from collections import deque
import hashlib
import json
from pathlib import Path
import sqlite3
import logging
import threading
import time
import uuid

import pyarrow as pa
import pyarrow.flight as fl


class OutboxError(RuntimeError):
    pass


class DiskOutbox:
    def __init__(self, directory, config=None, max_bytes=1024**3):
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self._file = open(self.directory / "owner.lock", "a+b")
        try:
            try:
                import fcntl
                fcntl.flock(self._file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except ImportError:
                import msvcrt
                self._file.write(b"\0"); self._file.flush(); self._file.seek(0)
                msvcrt.locking(self._file.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError as exc:
            self._file.close()
            raise OutboxError("outbox is owned by another process") from exc
        self._lock = threading.RLock()
        self._closed = False
        self.max_bytes = max_bytes
        self.db = sqlite3.connect(self.directory / "outbox.sqlite", check_same_thread=False)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS items(
                ordinal INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,
                kind TEXT NOT NULL,path TEXT NOT NULL,body BLOB NOT NULL,
                rows INTEGER NOT NULL,checksum TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',error TEXT);
        """)
        if config is not None:
            old = self.setting("config")
            if old is not None and old != config:
                self.close()
                raise OutboxError("outbox configuration does not match")
            self.set_setting("config", config)
        self.config = self.setting("config")
        if not isinstance(self.config, dict):
            self.close()
            raise OutboxError("outbox has no valid configuration")
        self.bytes = self.db.execute("SELECT COALESCE(SUM(length(body)),0) FROM items").fetchone()[0]

    def setting(self, key):
        with self._lock:
            row = self.db.execute("SELECT value FROM settings WHERE key=?", (key,)).fetchone()
            return json.loads(row[0]) if row else None

    def set_setting(self, key, value):
        with self._lock, self.db:
            self.db.execute("INSERT INTO settings VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (key, json.dumps(value, allow_nan=False)))

    def bind(self, identity):
        current = self.setting("server_id")
        if current is not None and current != identity:
            raise OutboxError("server-store identity changed; this outbox cannot be replayed here")
        self.set_setting("server_id", identity)

    def append(self, kind, path, body, rows=0):
        with self._lock, self.db:
            if self.bytes + len(body) > self.max_bytes:
                raise OutboxError("outbox quota exceeded; committed unacknowledged data was retained")
            self.db.execute("INSERT INTO items(id,kind,path,body,rows,checksum) VALUES(?,?,?,?,?,?)", (str(uuid.uuid4()),kind,path,body,rows,hashlib.sha256(body).hexdigest()))
            self.bytes += len(body)

    def next(self):
        with self._lock:
            head=self.db.execute("SELECT ordinal,status FROM items ORDER BY ordinal LIMIT 1").fetchone()
            if head is None:return None
            row = self.db.execute("SELECT ordinal,kind,path,body,rows,checksum,status FROM items WHERE ordinal=?",(head[0],)).fetchone()
            if row is None:
                return None
            if row[6] == "rejected":
                raise OutboxError("outbox contains a rejected entry; inspect delivery status before recovery")
            if hashlib.sha256(row[3]).hexdigest() != row[5]:
                self.reject(row[0], "stored payload checksum mismatch")
                raise OutboxError("stored payload checksum mismatch; entry was quarantined")
            return row[:5]

    def next_group(self):
        first=self.next()
        if first is None:return []
        if first[1]!="flight":return [first]
        result=[first];size=len(first[3])
        with self._lock:
            for row in self.db.execute("SELECT ordinal,kind,path,body,rows,checksum,status FROM items WHERE ordinal>? ORDER BY ordinal LIMIT 15",(first[0],)):
                if row[1]!="flight" or row[2]!=first[2] or row[6]!="pending" or size+len(row[3])>8*1024*1024:break
                if hashlib.sha256(row[3]).hexdigest()!=row[5]:break
                result.append(row[:5]);size+=len(row[3])
        return result

    def acknowledge(self, ordinal):
        with self._lock, self.db:
            row = self.db.execute("SELECT length(body) FROM items WHERE ordinal=?", (ordinal,)).fetchone()
            self.db.execute("DELETE FROM items WHERE ordinal=?", (ordinal,))
            if row: self.bytes -= row[0]

    def reject(self, ordinal, error):
        with self._lock, self.db:
            self.db.execute("UPDATE items SET status='rejected',error=? WHERE ordinal=?", (str(error),ordinal))

    def status(self):
        with self._lock:
            rows = self.db.execute("SELECT status,COUNT(*),COALESCE(SUM(rows),0) FROM items GROUP BY status").fetchall()
            return {"directory":str(self.directory),"bytes":self.bytes,"pending_batches":sum(count for state,count,_ in rows if state=="pending"),
                "rejected_batches":sum(count for state,count,_ in rows if state=="rejected"),"pending_rows":sum(n for state,_,n in rows if state=="pending"),
                "errors":[r[0] for r in self.db.execute("SELECT error FROM items WHERE status='rejected' ORDER BY ordinal LIMIT 20")]}

    def close(self):
        with self._lock:
            if self._closed: return
            self._closed = True
            self.db.close()
            self._file.close()


class OutboxSender:
    def __init__(self, outbox, deliver, *, on_error=None, max_queue_bytes=256*1024**2):
        self.outbox = outbox
        self.deliver = deliver
        self.on_error = on_error or logging.getLogger(__name__).warning
        self.max_queue_bytes = max_queue_bytes
        self._queue = deque()
        self._bytes = 0
        self._condition = threading.Condition()
        self._stop = False
        self._closing = False
        self._writing = False
        self._sending = False
        self._failing = False
        self._dropped_rows = 0
        self._local_failures = 0
        self._workers = 2
        self._final_status = None
        initial=self.outbox.status()
        self._pending=initial["pending_batches"]
        self._rejected=initial["rejected_batches"]
        self._disk_revision=0
        self._writer = threading.Thread(target=self._write, name="aeroboard-outbox-writer", daemon=True)
        self._sender = threading.Thread(target=self._send, name="aeroboard-outbox-sync", daemon=True)
        self._writer.start(); self._sender.start()

    def _submit(self, kind, path, payload, rows, size):
        with self._condition:
            if self._closing: raise OutboxError("outbox sender is closed")
            if self._bytes + size > self.max_queue_bytes or len(self._queue)>=1000:
                self._dropped_rows += rows; self._local_failures += 1
                self.on_error("[Aeroboard] Outbox memory queue is full; new telemetry was not accepted.")
                return
            self._queue.append((kind,path,payload,rows,size)); self._bytes += size
            self._condition.notify_all()

    def submit(self, batch, schema, path):
        self._submit("flight",path.decode(),batch,batch.num_rows,batch.nbytes)

    def submit_control(self, method, path, body):
        payload = json.dumps({"method":method,"body":body},allow_nan=False).encode()
        self._submit("control",path,payload,0,len(payload))

    def _done(self):
        with self._condition:
            self._workers -= 1
            if self._workers == 0:
                self._final_status = self.outbox.status()
                self.outbox.close()
            self._condition.notify_all()

    def _write(self):
        try:
            while True:
                with self._condition:
                    while not self._queue and not self._stop: self._condition.wait()
                    if self._stop: return
                    kind,path,payload,rows,size = self._queue.popleft()
                    self._writing = True
                try:
                    if kind == "flight":
                        from ._protocol import prepare_batch
                        payload=prepare_batch(payload)
                        stream = pa.BufferOutputStream()
                        with pa.ipc.new_stream(stream,payload.schema) as writer: writer.write_batch(payload)
                        body = stream.getvalue().to_pybytes()
                    else: body = payload
                    self.outbox.append(kind,path,body,rows)
                    with self._condition:
                        self._pending+=1
                        self._disk_revision+=1
                        self._condition.notify_all()
                except (OSError,sqlite3.Error,OutboxError,pa.ArrowException,TypeError,ValueError) as exc:
                    self.on_error(f"[Aeroboard] Local telemetry capture failed: {exc}")
                    with self._condition:
                        self._dropped_rows += rows; self._local_failures += 1
                finally:
                    with self._condition:
                        self._writing = False; self._bytes -= size; self._condition.notify_all()
        finally: self._done()

    def _send(self):
        delay = 0.5
        try:
            while True:
                with self._condition:
                    if self._stop: return
                    observed=self._disk_revision
                try:
                    group=self.outbox.next_group() if hasattr(self.deliver,"send_many") else ([item] if (item:=self.outbox.next()) is not None else [])
                    item=group[0] if group else None
                except (OutboxError, sqlite3.Error) as exc:
                    self.on_error(f"[Aeroboard] Outbox recovery stopped: {exc}")
                    with self._condition:
                        self._failing = True
                        while not self._stop: self._condition.wait(1)
                    return
                if item is None:
                    with self._condition:
                        while not self._stop and self._disk_revision==observed:self._condition.wait()
                    continue
                ordinal,kind,path,body,_rows = item
                with self._condition: self._sending = True
                delivered = False
                try:
                    if len(group)>1:self.deliver.send_many(group)
                    else:self.deliver(kind,path,body)
                    for entry in group:self.outbox.acknowledge(entry[0])
                    with self._condition:self._pending-=len(group)
                    delivered = True; delay = 0.5
                except Exception as exc:
                    status = getattr(exc,"status",500)
                    permanent = isinstance(exc,(OutboxError,pa.ArrowInvalid,pa.ArrowTypeError,fl.FlightUnauthenticatedError,fl.FlightUnauthorizedError)) or 400<=status<500 and status != 429
                    if permanent:
                        self.outbox.reject(ordinal,exc)
                        with self._condition:self._pending-=1;self._rejected+=1
                        self.on_error(f"[Aeroboard] Outbox entry quarantined: {exc}")
                        # Dependent operations must not pass a rejected registration/definition.
                        with self._condition:
                            self._failing = True
                            while not self._stop: self._condition.wait(1)
                        return
                    if not self._failing: self.on_error(f"[Aeroboard] Outbox retained offline: {exc}")
                finally:
                    with self._condition:
                        self._sending = False; self._failing = not delivered; self._condition.notify_all()
                if not delivered:
                    with self._condition:
                        if not self._stop: self._condition.wait(delay)
                    delay = min(10.0,delay*2)
        finally: self._done()

    def wait_local(self, timeout=10):
        deadline = time.monotonic()+timeout
        with self._condition:
            while self._queue or self._writing:
                remaining = deadline-time.monotonic()
                if remaining<=0: return False
                self._condition.wait(remaining)
            return self._local_failures == 0

    def wait_idle(self, timeout=10):
        deadline = time.monotonic()+(timeout or 10)
        if not self.wait_local(max(0,deadline-time.monotonic())): return False
        with self._condition:
            while self._pending or self._sending:
                if self._failing: return False
                remaining = deadline-time.monotonic()
                if remaining<=0: return False
                self._condition.wait(remaining)
            return self._rejected == 0

    @property
    def dropped_rows(self):
        with self._condition: return self._dropped_rows

    @property
    def pending_batches(self):
        with self._condition: return len(self._queue)+self._pending

    def status(self):
        with self._condition:
            return {**(self._final_status or self.outbox.status()),"memory_bytes":self._bytes,"dropped_rows":self._dropped_rows,"local_failures":self._local_failures,"offline":self._failing}

    def close(self, timeout=10):
        with self._condition: self._closing = True
        delivered = self.wait_idle(timeout)
        if not self.wait_local(timeout):
            self.on_error("[Aeroboard] Local flush did not complete; inspect delivery_status() for uncaptured data.")
        with self._condition: self._stop = True; self._condition.notify_all()
        cancel = getattr(self.deliver,"cancel",None)
        if cancel is not None: cancel()
        self._writer.join(timeout=2)
        # Keep file ownership until the bounded active operation ends. Requests
        # have a three-second timeout; Flight uploads allow sixty seconds.
        self._sender.join(timeout=61)
        return delivered
