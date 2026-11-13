"""
Aeroboard Python Client SDK.

This module provides a Python client for sending telemetry data to the Aeroboard
Rust backend via gRPC Flight protocol, and for querying historical data.

Classes:
    AeroboardClient: Main client for telemetry ingestion and data queries.
    AeroboardLightningLogger: Logger for PyTorch Lightning integration.
    AeroboardRayTuneLoggerCallback: Callback for Ray Tune integration.
    AeroboardLightningCallback: Callback for PyTorch Lightning integration.

Example:
    >>> from aeroboard_client import AeroboardClient
    >>> client = AeroboardClient(run_id="my-experiment")
    >>> client.log({"loss": 0.5, "accuracy": 0.8})
    >>> client.log_hyperparameters({"learning_rate": 0.001, "batch_size": 32})
    >>> client.flush()
    >>> client.close()
"""

from __future__ import annotations

import atexit
import logging
import json
import re
import os
import threading
import time
import io
import math
import wave
import warnings
import weakref
import uuid
from pathlib import Path
import pyarrow as pa
import pyarrow.flight as fl
from typing import Optional, Dict, Union, List, Any
from .schema import scalar_schema, image_schema, audio_schema, video_schema, hyperparameter_schema
from ._sender import BatchSender, DEFAULT_MAX_QUEUE_BYTES
from ._tracking import RunTracker
from ._protocol import batch_checksum, media_metadata, ENVELOPE_KEY


from ._errors import AeroboardClientError, FlightConnectionError
from ._query import QueryClient
from ._buffers import ColumnBuffer, ScalarBuffer
from ._media import _coerce_image_bytes, _normalize_media_format, _coerce_encoded_media, _coerce_audio_bytes, _encode_wav, _maybe_compress_encoded_image, _encode_pil_image
from ._naming import _normalize_run_id, _default_api_base_url, _fetch_existing_run_ids, resolve_run_id, _split_framework_suffix, _extract_version
from ._protocol import DeferredBatch, prepare_batch

MAX_IMAGE_BYTES = 256 * 1024  # 256 KB
# Audio/video are passed through as already-encoded bytes; these limits keep a
# single Flight message well under the server's AEROBOARD_MAX_MESSAGE_MB (64 MB).
MAX_MEDIA_BYTES = 32 * 1024 * 1024  # 32 MB per clip
MEDIA_FLUSH_BYTES = 8 * 1024 * 1024  # flush once this much media is buffered

_MEDIA_SCHEMAS = {"audio": audio_schema, "video": video_schema}
# Seconds a Flight upload may take before it is treated as failed and retried.
FLIGHT_CALL_TIMEOUT = 60.0
# Metric names whose value is the training step rather than a metric of their own.
_STEP_KEYS = ("step",)

# Clients still open at interpreter exit get flushed so trailing metrics aren't lost.
_OPEN_CLIENTS: "weakref.WeakSet[AeroboardClient]" = weakref.WeakSet()


@atexit.register
def _close_open_clients() -> None:
    for client in list(_OPEN_CLIENTS):
        try:
            client.close()
        except Exception:  # noqa: BLE001 - best effort during shutdown
            pass
_MEDIA_DEFAULT_FORMATS = {"audio": "wav", "video": "mp4"}


# Per-run dashboard defaults travel on the hyperparameter channel under a reserved
# key, so the server needs no extra storage. The dashboard hides this entry from
# hyperparameter views and uses it to pre-star charts and draw target lines.
DASHBOARD_CONFIG_KEY = "__aeroboard_dashboard__"
DASHBOARD_CONFIG_VALUE_TYPE = "aeroboard_dashboard"
_DASHBOARD_FIELDS = ("starred", "order", "targets")


def _normalize_dashboard_config(config: Dict[str, Any]) -> Dict[str, Any]:
    """Validate a dashboard config dict and return its JSON-ready form.

    Raises ValueError on unknown keys (catches typos such as "stared") or bad types.
    """
    if not isinstance(config, dict):
        raise ValueError("dashboard config must be a dict")
    unknown = sorted(set(config) - set(_DASHBOARD_FIELDS))
    if unknown:
        raise ValueError(f"unknown dashboard option(s): {', '.join(unknown)}; expected {', '.join(_DASHBOARD_FIELDS)}")

    normalized: Dict[str, Any] = {}
    for field in ("starred", "order"):
        names = config.get(field)
        if names is None:
            continue
        if isinstance(names, str) or not all(isinstance(name, str) for name in names):
            raise ValueError(f"dashboard {field} must be a list of metric names")
        normalized[field] = [name for name in (n.strip() for n in names) if name]

    targets = config.get("targets")
    if targets is not None:
        if not isinstance(targets, dict):
            raise ValueError("dashboard targets must be a dict of metric name -> number")
        clean: Dict[str, float] = {}
        for name, value in targets.items():
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                raise ValueError(f"dashboard target for {name!r} must be a finite number")
            clean[str(name)] = float(value)
        normalized["targets"] = clean
    return normalized


class AeroboardClient(QueryClient):
    """
    Python SDK client for sending telemetry data to the Aeroboard Rust backend.

    This client provides two main interfaces:
    1. **Telemetry Ingestion**: Log metrics, hyperparameters, and images via gRPC Flight
    2. **Data Querying**: Query historical data using HTTP API endpoints

    Args:
        run_id: Base run identifier. Auto-versioning is applied if a run with this
            name already exists (e.g., "resnet" -> "resnet-v2" -> "resnet-v3").
        url: gRPC Flight server URL (default: "grpc://localhost:50051")
        batch_size: Number of metrics to buffer before flushing (default: 100)
        background: Send batches from a background thread (default). Logging calls
            then never wait on the network; ``flush()`` waits for delivery.
        flush_timeout: Seconds ``flush()`` waits for queued batches to be delivered.
        max_queue_bytes: Memory budget for batches waiting to be sent while the
            server is unreachable; the oldest are dropped beyond it.

    Example:
        >>> client = AeroboardClient(run_id="my-model", url="grpc://server:50051")
        >>> client.log({"loss": 0.5})
        >>> client.flush()

    Attributes:
        run_id: The resolved run ID (after auto-versioning).
        client: The underlying FlightClient (None if connection failed).
        url: The Flight server URL.
        batch_size: Buffer size for batching.
    """

    def __init__(
        self,
        run_id: Optional[str] = None,
        url: str = "grpc://localhost:50051",
        batch_size: int = 100,
        api_base_url: Optional[str] = None,
        ssl_verify: Union[bool, str] = True,
        *,
        background: bool = True,
        flush_timeout: float = 10.0,
        max_queue_bytes: int = DEFAULT_MAX_QUEUE_BYTES,
        project: Optional[str] = None,
        resume: bool = False,
        tags: Optional[List[str]] = None,
        metadata: Optional[Dict[str, Any]] = None,
        capture_provenance: bool = False,
        spool_dir: Optional[Union[str, Path]] = None,
        max_spool_bytes: int = 1024**3,
        segment: Optional[str] = None,
    ):
        """
        Initialize the Aeroboard client.

        Args:
            run_id: Base run identifier for telemetry.
            url: gRPC Flight server address.
            batch_size: Batch size for telemetry buffering.
            api_base_url: HTTP API base URL (defaults to converting gRPC URL).
            ssl_verify: Whether to verify SSL certificates. Can also be a path
                to a CA bundle. Set to False for self-signed certificates.
        """
        self.base_run_id = _normalize_run_id(run_id)
        self.url = url
        self.batch_size = batch_size
        self.api_base_url = (api_base_url or _default_api_base_url(url)).rstrip("/")
        self.ssl_verify = ssl_verify
        self._scalar_buffer = ScalarBuffer()
        self._hyperparameter_buffer = ColumnBuffer()
        self._image_buffer: List[Dict[str, Any]] = []
        self._media_buffers: Dict[str, List[Dict[str, Any]]] = {"audio": [], "video": []}
        self._media_buffer_bytes: int = 0
        self._resolved_run_id: Optional[str] = None  # Resolved after first connection
        self._http_client: Optional[Any] = None  # For HTTP API queries
        self._had_flush_error: bool = False  # Track if flush encountered an error
        self._lock = threading.RLock()  # guards the buffers; log() may be called from several threads
        self._closed = False
        self.flush_timeout = flush_timeout
        self._batch_sequence = 0
        self._tracking = RunTracker(self.api_base_url, project=project, resume=resume, tags=tags,
            metadata=metadata, capture_provenance=capture_provenance, verify=ssl_verify, on_error=self._report_error)
        if segment is not None and (not isinstance(segment,str) or not segment.strip() or len(segment)>128):
            raise ValueError("segment must be a nonempty string up to 128 characters")
        self._tracking.segment = segment
        if resume:
            self._tracking.run_uid = str(uuid.UUID(self.base_run_id))
        self._transport = None

        # Initialize Flight client (connects lazily on the first upload)
        try:
            self.client = fl.FlightClient(self.url)
        except Exception as e:
            error_msg = f"[Aeroboard] Failed to connect to Flight server at {url}: {e}"
            logging.getLogger(__name__).warning(error_msg)
            self.client = None

        self._sender: Optional[BatchSender] = None
        if spool_dir is not None:
            from ._outbox import DiskOutbox, OutboxSender
            from ._delivery import OutboxTransport
            config = {"version": 1, "name": self.base_run_id, "url": self.url,
                "api_base_url": self.api_base_url, "ssl_verify": ssl_verify,
                "project": self._tracking.project, "resume": resume,
                "run_uid": self.run_uid, "session_id": self._tracking.session_id,
                "started_at_ms": self._tracking.started_at_ms,
                "tags": self._tracking.tags, "metadata": self._tracking.metadata, "segment": segment}
            outbox = DiskOutbox(Path(spool_dir)/self._tracking.session_id, config, max_spool_bytes)
            self._transport = OutboxTransport(outbox, tracker=self._tracking, flight=self.client)
            self._sender = OutboxSender(outbox, self._transport, on_error=self._report_error, max_queue_bytes=max_queue_bytes)
            self._tracking.durable_enqueue = self._sender.submit_control
        elif background and self.client is not None:
            self._sender = BatchSender(self._do_put, put_many=self._do_put_many, on_error=self._report_error, max_queue_bytes=max_queue_bytes)
        _OPEN_CLIENTS.add(self)

    @property
    def run_id(self) -> str:
        """
        Get the resolved run ID.

        Returns:
            The canonical run ID (with version suffix if necessary).
        """
        if self._closed:
            raise RuntimeError("Aeroboard client is closed")
        if self._resolved_run_id is None:
            tracked = self._tracking.activate(self.base_run_id)
            self._resolved_run_id = tracked or self._resolve_run_id(self.base_run_id)
        return self._tracking.run_id or self._resolved_run_id

    @property
    def run_uid(self) -> str:
        """Stable identity used for resume and metadata operations."""
        return self._tracking.run_uid

    def define_metric(self, name: str, *, goal=None, target=None, unit=None) -> None:
        """Define a ranking objective independently of chart display preferences."""
        _ = self.run_id
        self._tracking.define(name, goal=goal, target=target, unit=unit)

    def log_event(self, label: str, *, kind="note", step=None, details=None, media_ids=None, event_id=None) -> str:
        """Record an annotation or checkpoint reference at a training step."""
        _ = self.run_id
        return self._tracking.event(label, kind=kind, step=step, details=details, media_ids=media_ids, event_id=event_id)

    def finish(self, status: str = "completed", *, error: Optional[str] = None) -> None:
        """Record a terminal execution outcome and flush pending telemetry."""
        _ = self.run_id
        self.flush()
        if self._tracking.enabled:
            self._tracking.finish(status, error=error, final_sequence=self._batch_sequence,
                dropped_rows=self._sender.dropped_rows if self._sender else 0)
            if self._tracking.durable_enqueue is None:
                self._tracking.flush(self.flush_timeout)
        self.close()

    def __enter__(self):
        _ = self.run_id
        return self

    def __exit__(self, exc_type, exc, traceback):
        self.finish("cancelled" if exc_type and issubclass(exc_type, KeyboardInterrupt) else "failed" if exc_type else "completed", error=str(exc) if exc else None)
        return False

    def _ensure_http_client(self) -> None:
        """Lazily create the HTTP session used by the query_* helpers."""
        if self._http_client is None:
            self._http_client=self._tracking._session
        self._http_client.verify=self.ssl_verify

    def _resolve_run_id(self, base: str) -> str:
        """
        Query the backend to find existing runs with the given base name.

        If a run with the base name already exists, auto-increment a version suffix:
        "resnet" -> "resnet-v2" -> "resnet-v3" ...

        Args:
            base: The base run name to check.

        Returns:
            The resolved run ID.
        """
        self._ensure_http_client()
        normalized_base = _normalize_run_id(base)

        try:
            response = self._http_client.post(
                f"{self.api_base_url}/api/runs/resolve",
                json={"base_run_id": normalized_base},
                timeout=5,
                verify=self.ssl_verify,
            )
            if response.status_code == 200:
                resolved = response.json().get("run_id", normalized_base)
            else:
                existing = _fetch_existing_run_ids(normalized_base, api_base_url=self.api_base_url,session=self._http_client,verify=self.ssl_verify)
                resolved = resolve_run_id(normalized_base, existing)
        except Exception:
            existing = _fetch_existing_run_ids(normalized_base, api_base_url=self.api_base_url,session=self._http_client,verify=self.ssl_verify)
            resolved = resolve_run_id(normalized_base, existing)

        if resolved != normalized_base:
            logging.getLogger(__name__).warning(f"[Aeroboard] Run '{normalized_base}' already exists → using '{resolved}'")
        return resolved

    def log(self, metrics: Dict[str, float], step: Optional[Union[int, float]] = None) -> None:
        """
        Log scalar metrics to the current run.

        Metrics are buffered and flushed to the backend when the batch size is reached
        or when explicitly calling `flush()`. Only numeric values are logged.

        Args:
            metrics: Dictionary of metric names to values. A ``"step"`` entry is used
                as the step when ``step`` is not given (it is not logged as a metric).
            step: Training step these values belong to; the dashboard plots by it.

        Example:
            >>> client.log({"loss": 0.5, "accuracy": 0.8}, step=100)

        Raises:
            AeroboardClientError: If the Flight client is not connected.
        """
        if self.client is None:
            raise FlightConnectionError("No Flight connection available. Check server URL and connectivity.")

        values = dict(metrics)
        resolved_step = _coerce_scalar_metric_value(step)
        for key in _STEP_KEYS:
            if key in values:
                embedded = _coerce_scalar_metric_value(values.pop(key))
                if resolved_step is None:
                    resolved_step = embedded

        ts = int(time.time() * 1000)
        run_id = self.run_id
        with self._lock:
            for k, v in values.items():
                normalized = _coerce_scalar_metric_value(v)
                if normalized is not None:
                    self._scalar_buffer.append(ts,run_id,k,normalized,resolved_step)

            if len(self._scalar_buffer) >= self.batch_size:
                self._flush_buffers()

    def log_metrics(
        self,
        metrics: Dict[str, Any],
        *,
        step: Optional[Union[int, float]] = None,
        prefix: Optional[str] = None,
    ) -> None:
        """
        Log numeric metrics with optional metric prefixing and a shared step.

        This is convenient for vanilla training loops where you want to keep code like
        `client.log_metrics({"loss": loss, "accuracy": acc}, step=global_step, prefix="train")`.

        Args:
            metrics: Mapping of metric names to numeric values or scalar tensors.
            step: Optional shared step for all values.
            prefix: Optional prefix for metric names that do not already contain `/`.
        """
        payload: Dict[str, float] = {}
        for key, value in metrics.items():
            normalized = _coerce_scalar_metric_value(value)
            if normalized is None:
                continue

            metric_name = str(key)
            if prefix and metric_name not in {"step", "epoch"} and "/" not in metric_name:
                metric_name = f"{prefix}/{metric_name}"

            payload[metric_name] = normalized

        if payload:
            self.log(payload, step=step)

    def log_hyperparameters(
        self,
        hyperparameters: Dict[str, Any],
        step: Optional[Union[int, float]] = None,
    ) -> None:
        """
        Log hyperparameter settings for the current run.

        Hyperparameters are stored with metadata about their type (int, float, string, etc.).
        Optionally log the current training step.

        Args:
            hyperparameters: Dictionary of hyperparameter names to values.
            step: Training step number (optional).

        Example:
            >>> client.log_hyperparameters({
            ...     "learning_rate": 0.001,
            ...     "batch_size": 32,
            ...     "optimizer": "adam",
            ... }, step=10)

        Raises:
            AeroboardClientError: If the Flight client is not connected.
        """
        if self.client is None:
            raise FlightConnectionError("No Flight connection available.")

        ts = int(time.time() * 1000)
        run_id = self.run_id
        with self._lock:
            for key, value in hyperparameters.items():
                normalized = _normalize_hparam_value(value)
                if normalized is None:
                    continue
                encoded_value, value_type = normalized
                self._hyperparameter_buffer.append(ts,run_id,str(key),encoded_value,value_type)

            if len(self._hyperparameter_buffer) >= self.batch_size:
                self._flush_buffers()

        # Record the step at which the hyperparameters were set.
        if step is not None:
            self.log({"hyperparameters/step": float(step)})

    def configure_dashboard(
        self,
        *,
        starred: Optional[List[str]] = None,
        targets: Optional[Dict[str, float]] = None,
        order: Optional[List[str]] = None,
    ) -> None:
        """
        Set the dashboard's initial view for this run.

        Metric names may be full names ("train/loss") or chart names ("loss").
        These override the server's ``aeroboard.toml`` defaults; anything a user
        changes in the dashboard UI wins over both. Calling again replaces the
        previous settings for this run.

        Args:
            starred: Charts to star: expanded and pinned to the top.
            targets: Metric name -> target value, drawn as a dashed reference line.
            order: Initial chart order; unlisted charts follow alphabetically.

        Example:
            >>> client.configure_dashboard(
            ...     starred=["loss", "val/accuracy"],
            ...     targets={"val/accuracy": 0.95},
            ... )
        """
        if self.client is None:
            raise FlightConnectionError("No Flight connection available.")

        options = {"starred": starred, "targets": targets, "order": order}
        config = _normalize_dashboard_config({k: v for k, v in options.items() if v is not None})
        run_id = self.run_id
        with self._lock:
            self._hyperparameter_buffer.append(int(time.time()*1000),run_id,DASHBOARD_CONFIG_KEY,json.dumps(config,sort_keys=True),DASHBOARD_CONFIG_VALUE_TYPE)

        # Send right away so the dashboard picks up the view before the first metrics.
        with self._lock:
            self._flush_hyperparameter_buffer()

    def log_image(
        self,
        name: str,
        image: Any,
        *,
        format: Optional[str] = None,
        timestamp_ms: Optional[int] = None,
        step: Optional[float] = None,
        segment: Optional[str] = None,
    ) -> Optional[str]:
        """
        Log an image for the current run.

        Images are encoded to WebP or JPEG based on size, with automatic compression
        for large images. Supports bytes, PIL images, numpy arrays, and torch tensors.

        Args:
            name: Image name (e.g., "output", "input").
            image: Image data in any supported format (bytes, PIL, numpy, torch).
            format: Image format (default: inferred from extension or "webp")
            timestamp_ms: Optional timestamp in milliseconds (defaults to now).

        Example:
            >>> client.log_image("output", prediction_image)
            >>> client.log_image("input", input_image, format="png")

        Raises:
            AeroboardClientError: If the Flight client is not connected.
            TypeError: If the image format is not supported.
        """
        if self.client is None:
            raise FlightConnectionError("No Flight connection available.")

        # Snapshot mutable inputs before returning to training. Encoding stays off this thread.
        from ._media import snapshot_image
        image,estimated_bytes = snapshot_image(image)
        inferred_format = format
        _validate_media_position(step,segment)
        run_id = self.run_id
        asset_id = str(uuid.uuid4())
        with self._lock:
            self._image_buffer.append({
                "timestamp": int(time.time() * 1000) if timestamp_ms is None else timestamp_ms,
                "asset_id": asset_id, "step": step, "segment": segment,
                "run_id": run_id,
                "name": name,
                "image_data": image,
                "estimated_bytes": estimated_bytes,
                "format": inferred_format,
            })

            # Images have smaller batch size (max 4 per batch due to size)
            if len(self._image_buffer) >= max(1, math.ceil(self.batch_size / 4)):
                self._flush_buffers()
        return asset_id if "assets" in self._tracking.capabilities.get("features",[]) else None

    def log_audio(
        self,
        name: str,
        audio: Any,
        *,
        sample_rate: Optional[int] = None,
        format: Optional[str] = None,
        timestamp_ms: Optional[int] = None,
        step: Optional[float] = None,
        segment: Optional[str] = None,
    ) -> Optional[str]:
        """
        Log an audio clip for the current run.

        Encoded audio (bytes or a file path such as ``"sample.mp3"``) is sent as-is
        with no re-encoding. Raw waveforms (numpy arrays or torch tensors) are
        packed into 16-bit PCM WAV using numpy and the standard library only.

        Args:
            name: Stream name (e.g., "eval/speech").
            audio: Encoded bytes, a file path, or a waveform array of shape
                ``(samples,)``, ``(samples, channels)`` or ``(channels, samples)``.
                Float waveforms are expected in ``[-1, 1]``; int16 is used as-is.
            sample_rate: Sample rate in Hz (required for waveform arrays).
            format: Container format, e.g. "wav", "mp3", "ogg", "flac". Inferred
                from the file extension for paths; defaults to "wav".
            timestamp_ms: Optional timestamp in milliseconds (defaults to now).

        Example:
            >>> client.log_audio("eval/speech", waveform, sample_rate=16000)
            >>> client.log_audio("eval/tts", "outputs/sample.mp3")
        """
        payload, media_format = _coerce_audio_bytes(audio, sample_rate=sample_rate, format=format)
        return self._append_media("audio", name, payload, media_format, timestamp_ms, step=step, segment=segment)

    def log_video(
        self,
        name: str,
        video: Any,
        *,
        format: Optional[str] = None,
        timestamp_ms: Optional[int] = None,
        step: Optional[float] = None,
        segment: Optional[str] = None,
    ) -> Optional[str]:
        """
        Log an encoded video clip for the current run.

        The bytes are forwarded untouched; Aeroboard never transcodes on the
        Python side. Use a browser-playable container such as MP4 (H.264) or
        WebM (VP8/VP9/AV1). Animated GIF/WebP are also supported.

        Args:
            name: Stream name (e.g., "eval/rollout").
            video: Encoded bytes or a path to a video file.
            format: Container format, e.g. "mp4", "webm", "gif". Inferred from
                the file extension for paths; defaults to "mp4".
            timestamp_ms: Optional timestamp in milliseconds (defaults to now).

        Example:
            >>> client.log_video("eval/rollout", "videos/episode_10.mp4")
        """
        payload, media_format = _coerce_encoded_media(video, kind="video", format=format)
        return self._append_media("video", name, payload, media_format, timestamp_ms, step=step, segment=segment)

    def _append_media(
        self,
        kind: str,
        name: str,
        payload: bytes,
        media_format: str,
        timestamp_ms: Optional[int],
        *, step: Optional[float] = None, segment: Optional[str] = None,
    ) -> Optional[str]:
        if self.client is None:
            raise FlightConnectionError("No Flight connection available.")
        if not payload:
            raise ValueError(f"Cannot log an empty {kind} payload.")
        _validate_media_position(step,segment)
        if len(payload) > MAX_MEDIA_BYTES:
            raise ValueError(
                f"{kind} payload for '{name}' is {len(payload) / 1e6:.1f} MB; the limit is "
                f"{MAX_MEDIA_BYTES / 1e6:.0f} MB. Log a shorter or more compressed clip."
            )

        run_id = self.run_id
        asset_id = str(uuid.uuid4())
        with self._lock:
            self._media_buffers[kind].append({
                "timestamp": int(time.time() * 1000) if timestamp_ms is None else timestamp_ms,
                "asset_id": asset_id, "step": step, "segment": segment,
                "run_id": run_id,
                "name": name,
                "data": payload,
                "format": media_format,
            })
            self._media_buffer_bytes += len(payload)

            pending = sum(len(buffer) for buffer in self._media_buffers.values())
            if (
                self._media_buffer_bytes >= MEDIA_FLUSH_BYTES
                or pending >= max(1, math.ceil(self.batch_size / 4))
            ):
                self._flush_buffers()
        return asset_id if "assets" in self._tracking.capabilities.get("features",[]) else None

    def attach_lightning(
        self,
        run_name: Optional[str] = None,
        *,
        batch_size: Optional[int] = None,
    ) -> "AeroboardLightningLogger":
        """
        Compatibility alias for creating a PyTorch Lightning logger.

        Prefer `lightning_logger()` for new code and pass the returned object to
        `pl.Trainer(logger=...)`.

        Args:
            run_name: Run name (uses client's run_id if not provided).
            batch_size: Override the default batch size.

        Returns:
            AeroboardLightningLogger instance bound to this client.

        Raises:
            RuntimeError: If Lightning is not installed.
        """
        warnings.warn(
            "attach_lightning() is deprecated; use lightning_logger() and pass it to Trainer(logger=...) instead.",
            DeprecationWarning,
            stacklevel=2,
        )
        return self.lightning_logger(run_name=run_name, batch_size=batch_size)

    def lightning_logger(
        self,
        run_name: Optional[str] = None,
        *,
        batch_size: Optional[int] = None,
    ) -> "AeroboardLightningLogger":
        """
        Create a Lightning-compatible logger that streams metrics to Aeroboard.

        The logger captures Lightning metrics via `self.log(...)` and forwards
        hyperparameters logged by Lightning's logger interface.

        Args:
            run_name: Run name (uses client's run_id if not provided).
            batch_size: Override the default batch size.

        Returns:
            AeroboardLightningLogger instance to pass to Lightning Trainer.

        Example:
            >>> from lightning.pytorch import Trainer
            >>> client = AeroboardClient()
            >>> logger = client.lightning_logger()
            >>> trainer = Trainer(logger=logger)
            >>> trainer.fit(model)

        Raises:
            RuntimeError: If Lightning is not installed.
        """
        from .integrations.lightning import AeroboardLightningLogger
        return AeroboardLightningLogger(
            client=self,
            run_id=run_name or self.base_run_id,
            url=self.url,
            batch_size=batch_size or self.batch_size,
        )

    def flush(self, *, local_only: bool = False) -> bool:
        """
        Send all buffered data and wait (up to ``flush_timeout``) until the server
        has acknowledged it.

        If the server is currently unreachable this returns right away; the data
        stays queued and is retried in the background.
        """
        if self.client is None:
            return False

        with self._lock:
            self._flush_buffers()
        if self._sender is not None:
            if local_only:
                if self._tracking.durable_enqueue is None:
                    raise ValueError("local_only requires spool_dir")
                return self._sender.wait_local(self.flush_timeout)
            delivered = self._sender.wait_idle(self.flush_timeout)
        else:
            if local_only:
                raise ValueError("local_only requires spool_dir")
            delivered = not self._had_flush_error
        if self._tracking.enabled and self._tracking.durable_enqueue is None:
            delivered = self._tracking.flush(self.flush_timeout) and delivered
        return delivered

    def delivery_status(self) -> dict:
        """Inspect queued, rejected, dropped, and locally durable telemetry."""
        if self._tracking.durable_enqueue is not None:
            return self._sender.status()
        return {"pending_batches": self._sender.pending_batches if self._sender else 0,
            "dropped_rows": self._sender.dropped_rows if self._sender else 0,
            "persistent": False, "had_error": self._had_flush_error}

    def _flush_buffers(self) -> None:
        """Turn every non-empty buffer into a batch and hand it to the sender.

        Buffers are cleared as soon as their batch is queued; delivery, retries and
        the memory budget are the sender's job. Called with ``self._lock`` held.
        """
        try:
            self._flush_scalar_buffer()
            self._flush_hyperparameter_buffer()
            self._flush_image_buffer()
            for kind in self._media_buffers:
                self._flush_media_buffer(kind)
        except Exception as e:
            self._report_error(f"[Aeroboard] Error flushing data via Flight: {e}")

    def _report_error(self, message: str) -> None:
        logging.getLogger(__name__).warning(message)
        self._had_flush_error = True

    def _send(self, batch: pa.RecordBatch, schema: pa.Schema, descriptor_path: bytes) -> None:
        if self._tracking.enabled and "batch_envelopes" in self._tracking.capabilities.get("features", []):
            self._batch_sequence += 1
            envelope = {"version":2 if self._tracking.capabilities.get("protocol_version",1)>=2 else 1,"batch_id":str(uuid.uuid4()),"run_uid":self.run_uid,
                "session_id":self._tracking.session_id,"server_id":self._tracking.server_id,
                "sequence":self._batch_sequence,"checksum":"","captured_at_ms":int(time.time()*1000),"segment":self._tracking.segment}
            if isinstance(batch,DeferredBatch):
                original=batch.factory
                batch=DeferredBatch(lambda:original().replace_schema_metadata({ENVELOPE_KEY:json.dumps(envelope).encode()}),batch.num_rows,batch.nbytes,batch.schema)
            else:
                batch=batch.replace_schema_metadata({**(batch.schema.metadata or {}),ENVELOPE_KEY:json.dumps(envelope).encode()})
            schema=batch.schema
        if self._sender is not None:
            self._sender.submit(batch, schema, descriptor_path)
        else:
            self._do_put(batch, schema, descriptor_path)

    def had_error(self) -> bool:
        """
        Check whether sending telemetry failed since the last ``clear_error()``.

        Returns:
            True if a batch could not be delivered (it may still be retried).
        """
        return self._had_flush_error

    def clear_error(self) -> None:
        """Clear the flush error flag."""
        self._had_flush_error = False

    def _flush_scalar_buffer(self) -> None:
        """Flush scalar metrics buffer to the backend."""
        if not self._scalar_buffer:
            return

        timestamps,run_ids,names,values,steps = self._scalar_buffer.columns

        batch = pa.RecordBatch.from_arrays(
            [
                pa.array(timestamps, type=pa.timestamp('ms')),
                pa.array(run_ids, type=pa.string()),
                pa.array(names, type=pa.string()),
                pa.array(values, type=pa.float64()),
                pa.array(steps, type=pa.float64()),
            ],
            schema=scalar_schema,
        )

        self._send(batch, scalar_schema, b"scalars")
        self._scalar_buffer.clear()

    def _flush_hyperparameter_buffer(self) -> None:
        """Flush hyperparameters buffer to the backend."""
        if not self._hyperparameter_buffer:
            return

        batch=pa.RecordBatch.from_arrays(
            [pa.array(column,type=field.type) for column,field in zip(self._hyperparameter_buffer.columns,hyperparameter_schema)],
            schema=hyperparameter_schema,
        )

        self._send(batch, hyperparameter_schema, b"hyperparameters")
        self._hyperparameter_buffer.clear()

    def _flush_image_buffer(self) -> None:
        """Flush images buffer to the backend."""
        if not self._image_buffer:
            return

        rows = self._image_buffer[:]
        def encode():
            encoded = [_coerce_image_bytes(row["image_data"],format=row["format"]) for row in rows]
            batch = pa.RecordBatch.from_arrays(
                [
                    pa.array([x["timestamp"] for x in rows], type=pa.timestamp('ms')),
                    pa.array([x["run_id"] for x in rows], type=pa.string()),
                    pa.array([x["name"] for x in rows], type=pa.string()),
                    pa.array([item[0] for item in encoded], type=pa.binary()),
                    pa.array([item[1] for item in encoded], type=pa.string()),
                ],
                schema=image_schema,
            )

            if self._tracking.enabled and "assets" in self._tracking.capabilities.get("features",[]):
                batch = media_metadata(batch,rows)
            return batch
        deferred=DeferredBatch(encode,len(rows),sum(row["estimated_bytes"] for row in rows),image_schema)
        self._send(deferred,image_schema,b"images")
        self._image_buffer.clear()

    def _flush_media_buffer(self, kind: str) -> None:
        """Flush buffered audio or video clips to the backend."""
        buffer = self._media_buffers[kind]
        if not buffer:
            return

        schema = _MEDIA_SCHEMAS[kind]
        batch = pa.RecordBatch.from_arrays(
            [
                pa.array([x["timestamp"] for x in buffer], type=pa.timestamp('ms')),
                pa.array([x["run_id"] for x in buffer], type=pa.string()),
                pa.array([x["name"] for x in buffer], type=pa.string()),
                pa.array([x["data"] for x in buffer], type=pa.binary()),
                pa.array([x["format"] for x in buffer], type=pa.string()),
            ],
            schema=schema,
        )

        if self._tracking.enabled and "assets" in self._tracking.capabilities.get("features",[]):
            batch = media_metadata(batch,buffer)
        self._send(batch, batch.schema, kind.encode())
        self._media_buffer_bytes -= sum(len(x["data"]) for x in buffer)
        buffer.clear()

    def _do_put_many(self,items):
        if "batch_streams" not in self._tracking.capabilities.get("features",[]):
            for batch,schema,path in items:self._do_put(batch,schema,path)
            return
        batches=[prepare_batch(batch) for batch,_schema,_path in items]
        writer,reader=self.client.do_put(fl.FlightDescriptor.for_path(items[0][2]),batches[0].schema,options=fl.FlightCallOptions(timeout=FLIGHT_CALL_TIMEOUT))
        try:
            for batch in batches:writer.write_with_metadata(batch,(batch.schema.metadata or {}).get(ENVELOPE_KEY,b""))
            writer.done_writing()
            if reader.read() is None:raise FlightConnectionError("server did not acknowledge the batch stream")
        finally:writer.close()

    def _do_put(
        self,
        batch: pa.RecordBatch,
        schema: pa.Schema,
        descriptor_path: bytes,
    ) -> None:
        """Send a RecordBatch to the Flight server and wait for its acknowledgement.

        Raises on failure; the background sender decides whether to retry.
        """
        batch=prepare_batch(batch)
        schema=batch.schema
        if self.client is None:
            return

        desc = fl.FlightDescriptor.for_path(descriptor_path)
        options = fl.FlightCallOptions(timeout=FLIGHT_CALL_TIMEOUT)
        writer, _reader = self.client.do_put(desc, schema, options=options)
        writer.write_batch(batch)
        writer.done_writing()
        # close() completes the RPC and raises if the server rejected the batch.
        writer.close()


    def close(self, timeout: Optional[float] = None) -> None:
        """
        Flush remaining data, wait up to ``timeout`` seconds (default:
        ``flush_timeout``) for it to be delivered, and release the connection.
        """
        if self._closed:
            return
        self._closed = True
        _OPEN_CLIENTS.discard(self)
        if self.client is not None:
            with self._lock:
                self._flush_buffers()
        if self._sender is not None:
            self._sender.close(self.flush_timeout if timeout is None else timeout)
        if self._tracking.enabled and self._tracking.durable_enqueue is None:
            self._tracking.flush(self.flush_timeout if timeout is None else timeout)
        self._tracking.close()
        if self.client:
            try:
                self.client.close()
            except Exception as e:
                logging.getLogger(__name__).warning(f"[Aeroboard] Warning: error closing Flight client: {e}")


def _validate_media_position(step, segment):
    if step is not None and (isinstance(step,bool) or not isinstance(step,(int,float)) or not math.isfinite(step)):
        raise ValueError("media step must be finite")
    if segment is not None and (not isinstance(segment,str) or not segment.strip() or len(segment)>128):
        raise ValueError("media segment must be a nonempty string up to 128 characters")


def _normalize_hparam_value(value: Any) -> Optional[tuple[str, str]]:
    """
    Normalize a hyperparameter value for storage.

    Args:
        value: The value to normalize.

    Returns:
        Tuple of (encoded_string, type_name) or None if value should be skipped.
    """
    if value is None:
        return None
    if isinstance(value, bool):
        return ("true" if value else "false", "bool")
    if isinstance(value, int) and not isinstance(value, bool):
        return (str(value), "int")
    if isinstance(value, float):
        return (repr(value), "float")
    if isinstance(value, str):
        return (value, "string")
    if isinstance(value, (list, tuple, set)):
        return (", ".join(map(str, value)), "list")
    return (str(value), type(value).__name__)


def _mapping_like(value: Any) -> Dict[str, Any]:
    """
    Extract items from a mapping-like object.

    Args:
        value: Object with .items() or dict-like.

    Returns:
        Dictionary of items.
    """
    if isinstance(value, dict):
        return value
    if hasattr(value, "items"):
        return dict(value.items())
    return {k: getattr(value, k) for k in dir(value) if not k.startswith('_') and not callable(getattr(value, k))}


def _coerce_scalar_metric_value(value: Any) -> Optional[float]:
    """Convert Lightning metric values to floats when possible."""
    if value is None:
        return None
    if isinstance(value, bool):
        return float(value)
    if isinstance(value, (int, float)):
        return float(value)
    if hasattr(value, "detach"):
        value = value.detach()
    if hasattr(value, "item"):
        try:
            return float(value.item())
        except (TypeError, ValueError):
            return None
    return None


_INTEGRATIONS={"AeroboardLightningCallback":"lightning","AeroboardLightningLogger":"lightning","AeroboardRayTuneLoggerCallback":"ray"}
def __getattr__(name):
    if name in _INTEGRATIONS:
        import importlib
        value=getattr(importlib.import_module(f".integrations.{_INTEGRATIONS[name]}",__package__),name)
        globals()[name]=value
        return value
    if name=="_RAY_TUNE_AVAILABLE":
        from .integrations.ray import _RAY_TUNE_AVAILABLE
        return _RAY_TUNE_AVAILABLE
    raise AttributeError(name)
