"""Run control operations and heartbeats, isolated from bulk Flight uploads."""
from __future__ import annotations

from collections import deque
import importlib.metadata
import json
import math
import subprocess
import threading
import time
import uuid

import requests


class TrackingError(RuntimeError):
    pass


def provenance() -> dict:
    result = {}
    try:
        commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, timeout=1, check=True)
        dirty = subprocess.run(["git", "status", "--porcelain"], capture_output=True, timeout=1, check=True)
        result.update(git_commit=commit.stdout.strip(), git_dirty=bool(dirty.stdout))
    except (OSError, subprocess.SubprocessError):
        pass
    for package in ("torch", "lightning", "aeroboard"):
        try:
            result[package + "_version"] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            pass
    return result


class RunTracker:
    def __init__(self, base_url, *, project=None, resume=False, verify=True,
                 capture_provenance=False, metadata=None, tags=None, interval=15.0, on_error=print):
        if project is not None and (not isinstance(project, str) or not project or len(project)>128):
            raise ValueError("project must be a nonempty project ID")
        self.base_url = base_url
        self.project = project or "default"
        self.resume = resume
        self.required = project is not None or resume
        self.verify = verify
        self.run_uid = str(uuid.uuid4())
        self.session_id = str(uuid.uuid4())
        self.server_id = None
        self.capabilities = {}
        self.run_id = None
        self.enabled = False
        self.attempted = False
        self.sequence = 0
        self.interval = interval
        self.on_error = on_error
        self.metadata = {**(provenance() if capture_provenance else {}), **(metadata or {})}
        self.tags = tags or []
        if not isinstance(self.metadata,dict) or not isinstance(self.tags,list) or len(self.tags)>50 or any(not isinstance(tag,str) or len(tag)>128 for tag in self.tags):
            raise ValueError("metadata must be an object; tags must contain at most 50 strings of 128 characters")
        json.dumps(self.metadata, allow_nan=False)
        self._queue = deque()
        self._condition = threading.Condition()
        self._stop = False
        self._finished = False
        self._busy = False
        self._thread = None
        self._session = requests.Session()
        self._session.verify = verify
        self.definitions = {}
        self.segment = None
        self.durable_enqueue = None
        self.started_at_ms = int(time.time()*1000)
        self.replay = False
        self._rejected = 0

    def request(self, method, path, payload=None):
        if self._stop:
            raise TrackingError("run tracker is closed")
        response = self._session.request(method, self.base_url + path, json=payload, timeout=3)
        if not response.ok:
            try:
                message = response.json().get("error", response.text)
            except ValueError:
                message = response.text
            error = TrackingError(f"HTTP {response.status_code}: {message}")
            error.status = response.status_code
            raise error
        return response.json()

    def activate(self, name):
        if self.durable_enqueue is not None:
            if not self.attempted:
                if self.resume:
                    # Offline resume uses a stable UID, never a display name.
                    self.run_uid = str(uuid.UUID(name))
                self.run_id = name
                self.enabled = self.attempted = True
                self.capabilities = {"features": ["assets", "batch_envelopes"]}
                self.durable_enqueue("BOOTSTRAP", "", {"name": name})
            return self.run_id
        if self.attempted:
            return self.run_id
        try:
            capabilities = self.request("GET", "/api/capabilities")
            if "lifecycle" not in capabilities.get("features", []):
                raise TrackingError("server does not support run lifecycle")
        except (requests.RequestException, TrackingError, ValueError) as exc:
            if self.required:
                raise TrackingError(f"Requested project/resume features are unavailable: {exc}") from exc
            self.attempted = True
            return None
        self.connect(name, capabilities)
        self.attempted = True
        return self.run_id

    def connect(self, name, capabilities=None, *, persistent=False, bind=None):
        capabilities = capabilities or self.request("GET", "/api/capabilities")
        if "lifecycle" not in capabilities.get("features", []):
            error = TrackingError("server does not support run lifecycle")
            error.status = 422
            raise error
        if persistent and not capabilities.get("durable_replay"):
            error = TrackingError("disk outbox requires a persistent server with durable replay")
            error.status = 422
            raise error
        if bind is not None:
            bind(capabilities["server_id"])
        self.server_id = capabilities["server_id"]
        self.capabilities = capabilities
        if self.resume:
            run = self.request("GET", "/api/run-records/" + requests.utils.quote(name, safe=""))
            self.run_uid = run["id"]
        else:
            try:
                self.request("GET", "/api/projects/" + requests.utils.quote(self.project, safe=""))
            except TrackingError as exc:
                if getattr(exc, "status", None) != 404:
                    raise
                try:
                    self.request("POST", "/api/projects", {"id": self.project, "name": self.project})
                except TrackingError as create_error:
                    if getattr(create_error, "status", None) != 409:
                        raise
            run = self.request("POST", "/api/runs", {"run_uid": self.run_uid, "name": name,
                "project_id": self.project, "tags": self.tags, "provenance": self.metadata})
        session = self.request("POST", f"/api/runs/{self.run_uid}/sessions", {
            "session_id": self.session_id, "resume": bool(self.resume),
            "captured_at_ms": self.started_at_ms, "historical": self.replay, "segment": self.segment})
        self.project = run["data"]["project_id"]
        self.segment = session["data"].get("segment")
        self.run_id = run["data"]["run_id"]
        self.enabled = True
        if not self.replay and self._thread is None:
            self._thread = threading.Thread(target=self._work, name="aeroboard-heartbeat", daemon=True)
            self._thread.start()

    def enqueue(self, method, path, payload):
        if not self.enabled:
            raise TrackingError("server does not support this operation")
        if self.durable_enqueue is not None:
            self.durable_enqueue(method, path, payload)
            return
        with self._condition:
            if self._stop:
                raise TrackingError("run tracker is closed")
            if len(self._queue)>=1000:
                raise TrackingError("control queue is full; cannot enqueue metadata")
            self._queue.append((method, path, payload))
            self._condition.notify_all()

    def define(self, name, *, goal=None, target=None, unit=None):
        if not isinstance(name, str) or not name.strip():
            raise ValueError("metric name must be nonempty")
        if goal not in (None, "minimize", "maximize"):
            raise ValueError("goal must be minimize or maximize")
        if target is not None and (isinstance(target,bool) or not isinstance(target,(int,float)) or not math.isfinite(target)):
            raise ValueError("target must be a finite number")
        self.enqueue("DEFINE", "/api/metric-definitions", {
            "parent": self.run_uid, "name": name, "goal": goal, "target": target, "unit": unit})

    def event(self, label, *, kind="note", step=None, details=None, media_ids=None, event_id=None):
        if not isinstance(label,str) or not label.strip():
            raise ValueError("event label must be nonempty")
        if step is not None and (isinstance(step,bool) or not isinstance(step,(int,float)) or not math.isfinite(step)):
            raise ValueError("event step must be finite")
        if kind not in ("note","checkpoint","learning_rate","warning"):
            raise ValueError("event kind must be note, checkpoint, learning_rate or warning")
        identifier = event_id or str(uuid.uuid4())
        payload = {"id": identifier, "run_uid": self.run_uid, "session_id": self.session_id,
            "timestamp_ms": int(time.time()*1000), "step": step, "kind": kind,
            "label": label, "details": details or {}, "media_ids": media_ids or []}
        json.dumps(payload, allow_nan=False)
        self.enqueue("POST", "/api/events", payload)
        return identifier

    def finish(self, status="completed", error=None, final_sequence=0, dropped_rows=0):
        if status not in ("completed", "failed", "cancelled"):
            raise ValueError("status must be completed, failed, or cancelled")
        if self._finished:
            return
        self._finished = True
        with self._condition:
            self.sequence += 1
            self.enqueue("POST", f"/api/runs/{self.run_uid}/sessions/{self.session_id}/finish", {
                "sequence": self.sequence, "status": status, "error": error,
                "final_sequence": final_sequence, "dropped_rows": dropped_rows, "captured_at_ms": int(time.time()*1000)})

    def flush(self, timeout=3):
        deadline = time.monotonic()+timeout
        with self._condition:
            while self._queue or self._busy:
                remaining = deadline-time.monotonic()
                if remaining<=0:
                    return False
                self._condition.wait(remaining)
            return self._rejected == 0

    def deliver(self, method, path, payload):
        if method == "DEFINE":
            definitions = self.request("GET", "/api/metric-definitions?parent="+self.run_uid)
            existing = next((d for d in definitions if d["data"]["name"]==payload["name"]), None)
            body = {**payload, **({"revision": existing["revision"]} if existing else {})}
            result = self.request("POST", path, body)
            self.definitions[payload["name"]] = result
            return result
        return self.request(method, path, payload)

    def close(self):
        with self._condition:
            self._stop = True
            self._condition.notify_all()
        if self._thread:
            self._thread.join(timeout=4)
        if not self._thread or not self._thread.is_alive():
            self._session.close()

    def _work(self):
        next_heartbeat = time.monotonic()+self.interval
        delay = 0.5
        while True:
            with self._condition:
                while not self._stop and not self._queue and time.monotonic()<next_heartbeat:
                    self._condition.wait(max(0, next_heartbeat-time.monotonic()))
                if self._stop:
                    return
                if self._queue:
                    method, path, payload = self._queue[0]
                    queued = True
                elif not self._finished:
                    self.sequence += 1
                    method, path, payload = "POST", f"/api/runs/{self.run_uid}/sessions/{self.session_id}/heartbeat", {"sequence": self.sequence}
                    queued = False
                else:
                    next_heartbeat = time.monotonic()+self.interval
                    continue
                self._busy = True
            success = False
            try:
                self.deliver(method, path, payload)
                success = True
                delay = 0.5
            except (requests.RequestException, TrackingError, ValueError) as exc:
                self.on_error(f"[Aeroboard] Run metadata delivery failed: {exc}")
                success = isinstance(exc, TrackingError) and getattr(exc,"status",500)<500 and getattr(exc,"status",500)!=429
                if success and queued:
                    self._rejected += 1
            with self._condition:
                self._busy = False
                if queued and success:
                    self._queue.popleft()
                next_heartbeat = time.monotonic()+self.interval
                self._condition.notify_all()
                if not success:
                    self._condition.wait(delay)
                    delay = min(10.0,delay*2)
