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

import atexit
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
import pyarrow as pa
import pyarrow.flight as fl
from typing import Optional, Dict, Union, List, Any
from .schema import scalar_schema, image_schema, audio_schema, video_schema, hyperparameter_schema
from ._sender import BatchSender, DEFAULT_MAX_QUEUE_BYTES

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


def _resolve_lightning_integration() -> tuple[type[Any], type[Any], Any, bool]:
    try:
        from lightning.pytorch import Callback as lightning_callback_base
        from lightning.pytorch.loggers.logger import Logger as lightning_logger_base
        from lightning.pytorch.utilities.rank_zero import rank_zero_only
        return lightning_callback_base, lightning_logger_base, rank_zero_only, True
    except ImportError:
        try:
            from pytorch_lightning import Callback as lightning_callback_base
            from pytorch_lightning.loggers.logger import Logger as lightning_logger_base
            from pytorch_lightning.utilities.rank_zero import rank_zero_only
            return lightning_callback_base, lightning_logger_base, rank_zero_only, True
        except ImportError:
            def passthrough(func):
                return func

            return object, object, passthrough, False


_LightningCallbackBase, _LightningLoggerBase, _rank_zero_only, _LIGHTNING_AVAILABLE = _resolve_lightning_integration()


def _resolve_ray_tune_integration() -> tuple[type[Any], bool]:
    try:
        from ray.tune.logger import LoggerCallback as ray_logger_callback_base
        return ray_logger_callback_base, True
    except ImportError:
        return object, False


_RayTuneLoggerCallbackBase, _RAY_TUNE_AVAILABLE = _resolve_ray_tune_integration()


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


class AeroboardClientError(Exception):
    """Base exception for Aeroboard client errors."""

    pass


class FlightConnectionError(AeroboardClientError):
    """Raised when unable to connect to the Flight server."""

    pass


class AeroboardClient:
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
        self._scalar_buffer: List[Dict[str, Any]] = []
        self._hyperparameter_buffer: List[Dict[str, Any]] = []
        self._image_buffer: List[Dict[str, Any]] = []
        self._media_buffers: Dict[str, List[Dict[str, Any]]] = {"audio": [], "video": []}
        self._media_buffer_bytes: int = 0
        self._resolved_run_id: Optional[str] = None  # Resolved after first connection
        self._http_client: Optional[Any] = None  # For HTTP API queries
        self._had_flush_error: bool = False  # Track if flush encountered an error
        self._lock = threading.RLock()  # guards the buffers; log() may be called from several threads
        self._closed = False
        self.flush_timeout = flush_timeout

        # Initialize Flight client (connects lazily on the first upload)
        try:
            self.client = fl.FlightClient(self.url)
        except Exception as e:
            error_msg = f"[Aeroboard] Failed to connect to Flight server at {url}: {e}"
            print(error_msg)
            self.client = None

        self._sender: Optional[BatchSender] = None
        if background and self.client is not None:
            self._sender = BatchSender(self._do_put, on_error=self._report_error, max_queue_bytes=max_queue_bytes)
        _OPEN_CLIENTS.add(self)

    @property
    def run_id(self) -> str:
        """
        Get the resolved run ID.

        Returns:
            The canonical run ID (with version suffix if necessary).
        """
        if self._resolved_run_id is None:
            self._resolved_run_id = self._resolve_run_id(self.base_run_id)
        return self._resolved_run_id

    def _ensure_http_client(self) -> None:
        """Lazily create the HTTP session used by the query_* helpers."""
        if self._http_client is None:
            import requests

            session = requests.Session()
            session.verify = self.ssl_verify
            self._http_client = session

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
        import requests

        normalized_base = _normalize_run_id(base)

        try:
            response = requests.post(
                f"{self.api_base_url}/api/runs/resolve",
                json={"base_run_id": normalized_base},
                timeout=5,
                verify=self.ssl_verify,
            )
            if response.status_code == 200:
                resolved = response.json().get("run_id", normalized_base)
            else:
                existing = _fetch_existing_run_ids(normalized_base, api_base_url=self.api_base_url)
                resolved = resolve_run_id(normalized_base, existing)
        except Exception:
            existing = _fetch_existing_run_ids(normalized_base, api_base_url=self.api_base_url)
            resolved = resolve_run_id(normalized_base, existing)

        if resolved != normalized_base:
            print(f"[Aeroboard] Run '{normalized_base}' already exists → using '{resolved}'")
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
                    self._scalar_buffer.append({
                        "timestamp": ts,
                        "run_id": run_id,
                        "name": k,
                        "value": normalized,
                        "step": resolved_step,
                    })

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
                self._hyperparameter_buffer.append({
                    "timestamp": ts,
                    "run_id": run_id,
                    "key": str(key),
                    "value": encoded_value,
                    "value_type": value_type,
                })

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
            self._hyperparameter_buffer.append({
                "timestamp": int(time.time() * 1000),
                "run_id": run_id,
                "key": DASHBOARD_CONFIG_KEY,
                "value": json.dumps(config, sort_keys=True),
                "value_type": DASHBOARD_CONFIG_VALUE_TYPE,
            })
        # Send right away so the dashboard picks up the view before the first metrics.
        self.flush()

    def log_image(
        self,
        name: str,
        image: Any,
        *,
        format: Optional[str] = None,
        timestamp_ms: Optional[int] = None,
    ) -> None:
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

        encoded, inferred_format = _coerce_image_bytes(image, format=format)
        run_id = self.run_id
        with self._lock:
            self._image_buffer.append({
                "timestamp": timestamp_ms or int(time.time() * 1000),
                "run_id": run_id,
                "name": name,
                "image_data": encoded,
                "format": inferred_format,
            })

            # Images have smaller batch size (max 4 per batch due to size)
            if len(self._image_buffer) >= max(1, math.ceil(self.batch_size / 4)):
                self._flush_buffers()

    def log_audio(
        self,
        name: str,
        audio: Any,
        *,
        sample_rate: Optional[int] = None,
        format: Optional[str] = None,
        timestamp_ms: Optional[int] = None,
    ) -> None:
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
        self._append_media("audio", name, payload, media_format, timestamp_ms)

    def log_video(
        self,
        name: str,
        video: Any,
        *,
        format: Optional[str] = None,
        timestamp_ms: Optional[int] = None,
    ) -> None:
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
        self._append_media("video", name, payload, media_format, timestamp_ms)

    def _append_media(
        self,
        kind: str,
        name: str,
        payload: bytes,
        media_format: str,
        timestamp_ms: Optional[int],
    ) -> None:
        if self.client is None:
            raise FlightConnectionError("No Flight connection available.")
        if not payload:
            raise ValueError(f"Cannot log an empty {kind} payload.")
        if len(payload) > MAX_MEDIA_BYTES:
            raise ValueError(
                f"{kind} payload for '{name}' is {len(payload) / 1e6:.1f} MB; the limit is "
                f"{MAX_MEDIA_BYTES / 1e6:.0f} MB. Log a shorter or more compressed clip."
            )

        run_id = self.run_id
        with self._lock:
            self._media_buffers[kind].append({
                "timestamp": timestamp_ms or int(time.time() * 1000),
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
        return AeroboardLightningLogger(
            client=self,
            run_id=run_name or self.base_run_id,
            url=self.url,
            batch_size=batch_size or self.batch_size,
        )

    def flush(self) -> None:
        """
        Send all buffered data and wait (up to ``flush_timeout``) until the server
        has acknowledged it.

        If the server is currently unreachable this returns right away; the data
        stays queued and is retried in the background.
        """
        if self.client is None:
            return

        with self._lock:
            self._flush_buffers()
        if self._sender is not None:
            self._sender.wait_idle(self.flush_timeout)

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
        print(message)
        self._had_flush_error = True

    def _send(self, batch: pa.RecordBatch, schema: pa.Schema, descriptor_path: bytes) -> None:
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

        timestamps = [x["timestamp"] for x in self._scalar_buffer]
        run_ids = [x["run_id"] for x in self._scalar_buffer]
        names = [x["name"] for x in self._scalar_buffer]
        values = [x["value"] for x in self._scalar_buffer]
        steps = [x["step"] for x in self._scalar_buffer]

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

        batch = pa.RecordBatch.from_arrays(
            [
                pa.array([x["timestamp"] for x in self._hyperparameter_buffer], type=pa.timestamp('ms')),
                pa.array([x["run_id"] for x in self._hyperparameter_buffer], type=pa.string()),
                pa.array([x["key"] for x in self._hyperparameter_buffer], type=pa.string()),
                pa.array([x["value"] for x in self._hyperparameter_buffer], type=pa.string()),
                pa.array([x["value_type"] for x in self._hyperparameter_buffer], type=pa.string()),
            ],
            schema=hyperparameter_schema,
        )

        self._send(batch, hyperparameter_schema, b"hyperparameters")
        self._hyperparameter_buffer.clear()

    def _flush_image_buffer(self) -> None:
        """Flush images buffer to the backend."""
        if not self._image_buffer:
            return

        batch = pa.RecordBatch.from_arrays(
            [
                pa.array([x["timestamp"] for x in self._image_buffer], type=pa.timestamp('ms')),
                pa.array([x["run_id"] for x in self._image_buffer], type=pa.string()),
                pa.array([x["name"] for x in self._image_buffer], type=pa.string()),
                pa.array([x["image_data"] for x in self._image_buffer], type=pa.binary()),
                pa.array([x["format"] for x in self._image_buffer], type=pa.string()),
            ],
            schema=image_schema,
        )

        self._send(batch, image_schema, b"images")
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

        self._send(batch, schema, kind.encode())
        self._media_buffer_bytes -= sum(len(x["data"]) for x in buffer)
        buffer.clear()

    def _do_put(
        self,
        batch: pa.RecordBatch,
        schema: pa.Schema,
        descriptor_path: bytes,
    ) -> None:
        """Send a RecordBatch to the Flight server and wait for its acknowledgement.

        Raises on failure; the background sender decides whether to retry.
        """
        if self.client is None:
            return

        desc = fl.FlightDescriptor.for_path(descriptor_path)
        options = fl.FlightCallOptions(timeout=FLIGHT_CALL_TIMEOUT)
        writer, _reader = self.client.do_put(desc, schema, options=options)
        writer.write_batch(batch)
        writer.done_writing()
        # close() completes the RPC and raises if the server rejected the batch.
        writer.close()

    def query_history(
        self,
        *,
        run_id: Optional[str] = None,
        metric: Optional[str] = None,
        from_timestamp: Optional[float] = None,
        to_timestamp: Optional[float] = None,
        limit: Optional[int] = None,
        cursor: Optional[int] = None,
    ) -> Dict[str, Any]:
        """
        Query historical metrics data from the backend.

        Args:
            run_id: Filter by run ID. Uses client's run_id if not provided.
            metric: Filter by metric name.
            from_timestamp: Start timestamp in Unix seconds (float with decimals for ms).
            to_timestamp: End timestamp in Unix seconds.
            limit: Maximum number of results to return.
            cursor: Pagination cursor from previous query.

        Returns:
            Dictionary containing:
                - "rows": List of metric data points.
                - "run_summaries": Aggregated per-run summaries.
                - "next_cursor": Cursor for next page (None if last page).

        Example:
            >>> response = client.query_history(run_id="my-run", metric="loss")
            >>> for point in response["rows"]:
            ...     print(point["name"], point["value"])

        Raises:
            AeroboardClientError: If HTTP query fails.
        """
        self._ensure_http_client()
        url = f"{self.api_base_url}/api/history"

        params = {}
        if run_id is None:
            run_id = self.run_id
        if run_id:
            params["run_id"] = run_id
        if metric:
            params["metric"] = metric
        if from_timestamp is not None:
            params["start_ms"] = str(int(from_timestamp * 1000))
        if to_timestamp is not None:
            params["end_ms"] = str(int(to_timestamp * 1000))
        if limit is not None:
            params["limit"] = str(limit)
        if cursor is not None:
            params["cursor"] = str(cursor)

        try:
            resp = self._http_client.get(url, params=params, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            return resp.json()
        except Exception as e:
            raise AeroboardClientError(f"Failed to query history: {e}")

    def query_hyperparameters(
        self,
        *,
        run_id: Optional[str] = None,
        key: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[Dict[str, Any]]:
        """
        Query hyperparameter settings for a run.

        Args:
            run_id: Filter by run ID. Uses client's run_id if not provided.
            key: Filter by hyperparameter key.
            limit: Maximum number of results.

        Returns:
            List of hyperparameter entries.
        """
        self._ensure_http_client()

        url = f"{self.api_base_url}/api/history"

        params = {}
        if run_id is None:
            run_id = self.run_id
        if run_id:
            params["run_id"] = run_id
        if limit is not None:
            params["limit"] = str(limit)

        try:
            resp = self._http_client.get(url, params=params, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            data = resp.json()
            summaries = data.get("run_summaries", [])
            entries: List[Dict[str, Any]] = []
            for summary in summaries:
                summary_run_id = summary.get("run_id")
                for entry in summary.get("hyperparameters", []):
                    if key is not None and entry.get("key") != key:
                        continue
                    if key is None and entry.get("key") == DASHBOARD_CONFIG_KEY:
                        continue  # internal dashboard settings, not a hyperparameter
                    entries.append({
                        "run_id": summary_run_id,
                        "key": entry.get("key"),
                        "value": entry.get("value"),
                        "value_type": entry.get("value_type"),
                    })
            return entries
        except Exception as e:
            raise AeroboardClientError(f"Failed to query hyperparameters: {e}")

    def query_images(
        self,
        *,
        run_id: Optional[str] = None,
        name: Optional[str] = None,
        format: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[Dict[str, Any]]:
        """
        Query image history for a run.

        Args:
            run_id: Filter by run ID. Uses client's run_id if not provided.
            name: Filter by image name.
            format: Filter by image format (webp, jpeg, png).
            limit: Maximum number of images to return.

        Returns:
            List of image data points with base64-encoded image data.
        """
        self._ensure_http_client()

        url = f"{self.api_base_url}/api/history/images"

        params = {}
        if run_id is None:
            run_id = self.run_id
        if run_id:
            params["run_id"] = run_id
        if name:
            params["metric"] = name
        if limit is not None:
            params["limit"] = str(limit)

        try:
            resp = self._http_client.get(url, params=params, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            data = resp.json()
            rows = data.get("rows", [])
            if format is not None:
                rows = [row for row in rows if row.get("format") == format]
            return rows
        except Exception as e:
            raise AeroboardClientError(f"Failed to query images: {e}")

    def list_runs(self) -> List[str]:
        """
        List all run IDs from the backend.

        Returns:
            List of run ID strings.

        Raises:
            AeroboardClientError: If unable to connect or query.
        """
        self._ensure_http_client()

        url = f"{self.api_base_url}/api/runs"

        try:
            resp = self._http_client.get(url, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            data = resp.json()
            return data.get("runs", [])
        except Exception as e:
            raise AeroboardClientError(f"Failed to list runs: {e}")

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
        if self.client:
            try:
                self.client.close()
            except Exception as e:
                print(f"[Aeroboard] Warning: error closing Flight client: {e}")


class AeroboardLightningCallback:
    """
    PyTorch Lightning callback for automatic telemetry logging.

    This callback automatically logs:
    - Hyperparameters on fit start
    - Training metrics (loss, accuracy) on each batch end
    - Validation metrics on validation epoch end

    Example:
        >>> from lightning.pytorch import LightningModule, Trainer
        >>> callback = AeroboardLightningCallback()
        >>> trainer = Trainer(callbacks=[callback])
        >>> trainer.fit(model)
    """

    def __init__(
        self,
        run_id: Optional[str] = None,
        url: str = "grpc://localhost:50051",
        batch_size: int = 100,
        client: Optional[AeroboardClient] = None,
    ):
        """
        Initialize the Lightning callback.

        Args:
            run_id: Run name for telemetry.
            url: Flight server URL.
            batch_size: Telemetry batch size.
            client: Pre-created AeroboardClient instance.
        """
        if not _LIGHTNING_AVAILABLE:
            raise RuntimeError("lightning is required for AeroboardLightningCallback")

        # Create client if not provided
        self.client = client or AeroboardClient(run_id=run_id, url=url, batch_size=batch_size)

        # Create internal callback class
        class _Callback(_LightningCallbackBase):
            def __init__(self, outer: "AeroboardLightningCallback"):
                super().__init__()
                self._outer = outer
                self._hparams_logged = False

            def _client(self) -> AeroboardClient:
                return self._outer.client

            def _maybe_log_hparams(self, trainer, pl_module):
                """Log hyperparameters if not already logged."""
                if self._hparams_logged:
                    return
                combined = {}

                # Get module hyperparameters
                module_hparams = getattr(pl_module, "hparams", None)
                if module_hparams is not None:
                    combined.update(_mapping_like(module_hparams))

                # Get trainer logger hyperparameters
                if getattr(trainer, "logger", None) is not None:
                    maybe_hparams = getattr(trainer.logger, "hparams", None)
                    if maybe_hparams:
                        combined.update(_mapping_like(maybe_hparams))

                if combined:
                    self._client().log_hyperparameters(combined)
                    self._hparams_logged = True

            def on_fit_start(self, trainer, pl_module):
                """Log hyperparameters at fit start."""
                self._maybe_log_hparams(trainer, pl_module)

            def on_train_batch_end(self, trainer, pl_module, outputs, batch, batch_idx):
                """Log metrics after each training batch."""
                self._maybe_log_hparams(trainer, pl_module)
                payload: Dict[str, float] = {"step": float(trainer.global_step)}
                loss = outputs.get("loss") if isinstance(outputs, dict) else None
                if loss is not None:
                    payload["train/loss"] = float(loss.detach().item() if hasattr(loss, "detach") else loss)
                train_acc = trainer.callback_metrics.get("train_acc")
                if train_acc is not None:
                    payload["train/accuracy"] = float(train_acc.detach().item() if hasattr(train_acc, "detach") else train_acc)
                self._client().log(payload)

            def on_validation_epoch_end(self, trainer, pl_module):
                """Log validation metrics at epoch end."""
                self._maybe_log_hparams(trainer, pl_module)
                metrics = trainer.callback_metrics
                payload: Dict[str, float] = {"step": float(trainer.global_step)}
                if "val_loss" in metrics:
                    value = metrics["val_loss"]
                    payload["val/loss"] = float(value.detach().item() if hasattr(value, "detach") else value)
                if "val_acc" in metrics:
                    value = metrics["val_acc"]
                    payload["val/accuracy"] = float(value.detach().item() if hasattr(value, "detach") else value)
                if len(payload) > 1:
                    self._client().log(payload)
                    self._client().flush()

            def on_train_end(self, trainer, pl_module):
                """Flush remaining data at training end."""
                self._client().flush()

        self.callback = _Callback(self)

    def __getattr__(self, item: str) -> Any:
        """Delegate attribute access to the internal callback."""
        return getattr(self.callback, item)


class AeroboardLightningLogger(_LightningLoggerBase):
    """
    Lightning-compatible logger for Aeroboard telemetry.

    This logger forwards metrics emitted by `LightningModule.log(...)` and
    hyperparameters provided through Lightning's logger interface.

    Example:
        >>> from lightning.pytorch import Trainer
        >>> logger = AeroboardLightningLogger(run_id="lightning-demo")
        >>> trainer = Trainer(logger=logger)
        >>> trainer.fit(model)
    """

    def __init__(
        self,
        run_id: Optional[str] = None,
        url: str = "grpc://localhost:50051",
        batch_size: int = 100,
        client: Optional[AeroboardClient] = None,
    ):
        if not _LIGHTNING_AVAILABLE:
            raise RuntimeError("lightning is required for AeroboardLightningLogger")

        super().__init__()
        self.client = client or AeroboardClient(run_id=run_id, url=url, batch_size=batch_size)
        self._name = "aeroboard"
        self._version = self.client.run_id
        self._hparams: Dict[str, Any] = {}
        self._last_step: Optional[float] = None

    @property
    def name(self) -> str:
        return self._name

    @property
    def version(self) -> str:
        return self._version

    @property
    def experiment(self) -> AeroboardClient:
        return self.client

    @property
    def hparams(self) -> Dict[str, Any]:
        return dict(self._hparams)

    @_rank_zero_only
    def log_hyperparams(self, params: Any) -> None:
        values = _mapping_like(params)
        if not values:
            return
        self._hparams.update(values)
        self.client.log_hyperparameters(values)

    @_rank_zero_only
    def log_metrics(self, metrics: Dict[str, Any], step: Optional[int] = None) -> None:
        payload: Dict[str, Any] = {key: value for key, value in metrics.items() if key != "step"}

        resolved_step = _coerce_scalar_metric_value(step)
        if resolved_step is None:
            resolved_step = self._last_step
        if resolved_step is None and "epoch" in payload:
            resolved_step = _coerce_scalar_metric_value(payload["epoch"])

        if resolved_step is not None:
            self._last_step = resolved_step

        self.client.log_metrics(payload, step=resolved_step)

    @_rank_zero_only
    def save(self) -> None:
        self.client.flush()

    @_rank_zero_only
    def finalize(self, status: str) -> None:
        del status
        self.client.flush()


class AeroboardRayTuneLoggerCallback(_RayTuneLoggerCallbackBase):
    """
    Ray Tune callback that creates one Aeroboard run per trial.

    Trial configuration is logged as hyperparameters, and numeric trial results are
    streamed as scalar metrics to the matching Aeroboard run.
    """

    def __init__(
        self,
        *,
        url: str = "grpc://localhost:50051",
        api_base_url: Optional[str] = None,
        batch_size: int = 100,
        run_prefix: Optional[str] = None,
        metric_prefix: Optional[str] = None,
    ):
        if not _RAY_TUNE_AVAILABLE:
            raise RuntimeError("ray[tune] is required for AeroboardRayTuneLoggerCallback")

        super().__init__()
        self.url = url
        self.api_base_url = api_base_url
        self.batch_size = batch_size
        self.run_prefix = run_prefix
        self.metric_prefix = metric_prefix
        self._clients: Dict[str, AeroboardClient] = {}

    def _trial_key(self, trial: Any) -> str:
        return str(getattr(trial, "trial_id", id(trial)))

    def _trial_run_id(self, trial: Any) -> str:
        trial_name = (
            getattr(trial, "trial_name", None)
            or getattr(trial, "experiment_tag", None)
            or getattr(trial, "trial_id", None)
            or "trial"
        )
        if self.run_prefix:
            return f"{self.run_prefix}-{trial_name}"
        return str(trial_name)

    def _build_trial_hparams(self, trial: Any) -> Dict[str, Any]:
        payload = _mapping_like(getattr(trial, "config", {}) or {})
        trial_id = getattr(trial, "trial_id", None)
        trial_name = getattr(trial, "trial_name", None)
        experiment_tag = getattr(trial, "experiment_tag", None)
        if trial_id is not None:
            payload["ray/trial_id"] = str(trial_id)
        if trial_name is not None:
            payload["ray/trial_name"] = str(trial_name)
        if experiment_tag is not None:
            payload["ray/experiment_tag"] = str(experiment_tag)
        return payload

    def _get_client(self, trial: Any) -> AeroboardClient:
        trial_key = self._trial_key(trial)
        if trial_key in self._clients:
            return self._clients[trial_key]

        client = AeroboardClient(
            run_id=self._trial_run_id(trial),
            url=self.url,
            api_base_url=self.api_base_url,
            batch_size=self.batch_size,
        )
        hparams = self._build_trial_hparams(trial)
        if hparams:
            client.log_hyperparameters(hparams)
        self._clients[trial_key] = client
        return client

    def log_trial_start(self, trial: Any) -> None:
        self._get_client(trial)

    def log_trial_result(self, iteration: int, trial: Any, result: Dict[str, Any]) -> None:
        client = self._get_client(trial)
        step = _coerce_scalar_metric_value(result.get("training_iteration"))
        if step is None:
            step = _coerce_scalar_metric_value(iteration)

        filtered = {
            key: value
            for key, value in result.items()
            if key not in {
                "config",
                "date",
                "done",
                "experiment_id",
                "hostname",
                "node_ip",
                "pid",
                "timestamp",
                "time_since_restore",
                "timesteps_since_restore",
                "iterations_since_restore",
                "training_iteration",
                "trial_id",
                "should_checkpoint",
            }
        }
        client.log_metrics(filtered, step=step, prefix=self.metric_prefix)

    def log_trial_end(self, trial: Any, failed: bool = False) -> None:
        client = self._clients.pop(self._trial_key(trial), None)
        if client is None:
            return
        client.log_hyperparameters({"ray/failed": failed})
        client.flush()
        client.close()

    def on_experiment_end(self, trials: List[Any], **info: Any) -> None:
        del trials, info
        for client in list(self._clients.values()):
            client.flush()
            client.close()
        self._clients.clear()


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


def _coerce_image_bytes(
    image: Any,
    *,
    format: Optional[str] = None,
    max_bytes: int = MAX_IMAGE_BYTES,
) -> tuple[bytes, str]:
    """
    Coerce image data to bytes and infer format.

    Args:
        image: Image in any supported format (bytes, PIL, numpy, torch).
        format: Target format (default: inferred).
        max_bytes: Maximum allowed size in bytes.

    Returns:
        Tuple of (encoded_bytes, format_string).
    """
    inferred_format = (format or "png").lower()
    if isinstance(image, (bytes, bytearray, memoryview)):
        raw = bytes(image)
        return _maybe_compress_encoded_image(raw, inferred_format, max_bytes=max_bytes)

    try:
        from PIL import Image
    except ImportError:
        Image = None

    if Image is not None and isinstance(image, Image.Image):
        return _encode_pil_image(image, inferred_format, max_bytes=max_bytes)

    try:
        import numpy as np
    except ImportError:
        np = None

    if np is not None and isinstance(image, np.ndarray):
        if Image is None:
            raise RuntimeError("Pillow is required to encode numpy array images")
        pil_image = Image.fromarray(image)
        return _coerce_image_bytes(pil_image, format=inferred_format, max_bytes=max_bytes)

    try:
        import torch
    except ImportError:
        torch = None

    if torch is not None and isinstance(image, torch.Tensor):
        if np is None:
            raise RuntimeError("numpy is required to encode torch tensor images")
        arr = image.detach().cpu().numpy()
        if arr.ndim == 3 and arr.shape[0] in (1, 3, 4):
            arr = arr.transpose(1, 2, 0)
        return _coerce_image_bytes(arr, format=inferred_format, max_bytes=max_bytes)

    raise TypeError("Unsupported image type. Use bytes, PIL Image, numpy array, or torch tensor.")


def _normalize_media_format(value: str) -> str:
    return value.strip().lstrip(".").lower()


def _coerce_encoded_media(
    media: Any,
    *,
    kind: str,
    format: Optional[str] = None,
) -> tuple[bytes, str]:
    """
    Accept already-encoded media as bytes or a file path. No transcoding is done.

    Returns:
        Tuple of (payload, format).
    """
    if isinstance(media, (bytes, bytearray, memoryview)):
        media_format = _normalize_media_format(format or _MEDIA_DEFAULT_FORMATS[kind])
        return bytes(media), media_format

    if isinstance(media, (str, os.PathLike)):
        path = os.fspath(media)
        suffix = os.path.splitext(path)[1]
        media_format = _normalize_media_format(format or suffix or _MEDIA_DEFAULT_FORMATS[kind])
        with open(path, "rb") as handle:
            return handle.read(), media_format

    hint = (
        " Encode frames to MP4/WebM first (e.g. with ffmpeg) and pass the bytes or file path."
        if kind == "video"
        else ""
    )
    raise TypeError(f"Unsupported {kind} type {type(media).__name__}. Use bytes or a file path.{hint}")


def _coerce_audio_bytes(
    audio: Any,
    *,
    sample_rate: Optional[int] = None,
    format: Optional[str] = None,
) -> tuple[bytes, str]:
    """
    Coerce audio to encoded bytes.

    Encoded bytes and file paths pass through unchanged. numpy arrays and torch
    tensors are packed as 16-bit PCM WAV (vectorised numpy + stdlib ``wave``).
    """
    if isinstance(audio, (bytes, bytearray, memoryview, str, os.PathLike)):
        return _coerce_encoded_media(audio, kind="audio", format=format)

    if hasattr(audio, "detach") and hasattr(audio, "cpu"):  # torch.Tensor without importing torch
        audio = audio.detach().cpu().numpy()

    try:
        import numpy as np
    except ImportError:
        np = None

    if np is None or not isinstance(audio, np.ndarray):
        raise TypeError(
            f"Unsupported audio type {type(audio).__name__}. Use bytes, a file path, a numpy array or a torch tensor."
        )

    if format is not None and _normalize_media_format(format) != "wav":
        raise ValueError("Waveform arrays are encoded as WAV; pass encoded bytes to log other formats.")
    if not sample_rate or sample_rate <= 0:
        raise ValueError("sample_rate is required when logging a waveform array.")

    return _encode_wav(audio, int(sample_rate)), "wav"


def _encode_wav(samples: Any, sample_rate: int) -> bytes:
    import numpy as np

    arr = np.asarray(samples)
    if arr.ndim == 2 and arr.shape[0] < arr.shape[1]:
        arr = arr.T  # (channels, samples) -> (samples, channels)
    if arr.ndim == 1:
        arr = arr[:, None]
    if arr.ndim != 2 or arr.shape[0] == 0:
        raise ValueError(f"Expected a 1D or 2D waveform, got shape {np.shape(samples)}.")

    if np.issubdtype(arr.dtype, np.floating):
        pcm = (np.clip(np.nan_to_num(arr), -1.0, 1.0) * 32767.0).astype("<i2")
    elif arr.dtype == np.int16:
        pcm = arr.astype("<i2", copy=False)
    else:
        raise TypeError(f"Unsupported waveform dtype {arr.dtype}; use float in [-1, 1] or int16.")

    with io.BytesIO() as buffer:
        with wave.open(buffer, "wb") as writer:
            writer.setnchannels(pcm.shape[1])
            writer.setsampwidth(2)
            writer.setframerate(sample_rate)
            writer.writeframes(np.ascontiguousarray(pcm).tobytes())
        return buffer.getvalue()


def _maybe_compress_encoded_image(
    payload: bytes,
    image_format: str,
    *,
    max_bytes: int = MAX_IMAGE_BYTES,
) -> tuple[bytes, str]:
    """
    Compress large images to stay within size limits.

    Args:
        payload: Raw image bytes.
        image_format: Image format.
        max_bytes: Maximum allowed size.

    Returns:
        Compressed payload and format (may be converted to webp).
    """
    if len(payload) <= max_bytes:
        return payload, image_format

    try:
        from PIL import Image
    except ImportError:
        return payload, image_format

    try:
        with Image.open(io.BytesIO(payload)) as image:
            converted = (
                image.convert('RGB')
                if image.mode not in ('RGB', 'L')
                else image.copy()
            )
            return _encode_pil_image(converted, 'webp', max_bytes=max_bytes)
    except Exception:
        return payload, image_format


def _encode_pil_image(
    image: Any,  # PIL Image
    image_format: str,
    *,
    max_bytes: int = MAX_IMAGE_BYTES,
) -> tuple[bytes, str]:
    """
    Encode a PIL image to bytes.

    Args:
        image: PIL Image object.
        image_format: Target format.
        max_bytes: Maximum allowed size.

    Returns:
        Encoded bytes and format.
    """
    # Check if image is actually a PIL Image
    if not hasattr(image, 'save'):
        raise TypeError(f"Expected PIL Image, got {type(image)}")

    normalized = image_format.lower()
    save_format = 'JPEG' if normalized in {'jpg', 'jpeg'} else normalized.upper()

    if save_format == 'WEBP':
        for quality in (85, 75, 60):
            with io.BytesIO() as buffer:
                image.save(buffer, format='WEBP', quality=quality, method=6)
                payload = buffer.getvalue()
            if len(payload) <= max_bytes:
                return payload, 'webp'
        return payload, 'webp'

    with io.BytesIO() as buffer:
        image.save(buffer, format=save_format)
        payload = buffer.getvalue()

    if len(payload) > max_bytes:
        rgb_image = (
            image.convert('RGB')
            if getattr(image, 'mode', '') not in ('RGB', 'L')
            else image
        )
        return _encode_pil_image(rgb_image, 'webp', max_bytes=max_bytes)

    return payload, normalized


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


def _normalize_run_id(run_id: Optional[str]) -> str:
    """Normalize a run ID by stripping whitespace."""
    cleaned = (run_id or '').strip()
    return cleaned or f"run-{int(time.time())}"


def _default_api_base_url(flight_url: Optional[str] = None) -> str:
    if flight_url is None:
        return "http://localhost:3000"

    if flight_url.startswith("grpc://"):
        return flight_url.replace("grpc://", "http://").replace(":50051", ":3000")
    if flight_url.startswith("grpcs://"):
        return flight_url.replace("grpcs://", "https://").replace(":50051", ":3000")
    return "http://localhost:3000"


def _fetch_existing_run_ids(base: str, api_base_url: Optional[str] = None) -> set[str]:
    """Fetch run IDs matching ``base`` (or its versions) from the server's run list.

    Only used when the server's ``/api/runs/resolve`` endpoint is unavailable. The
    client never talks to the storage backend directly.
    """
    import requests

    normalized_base = _normalize_run_id(base)
    api_base_url = (api_base_url or "http://localhost:3000").rstrip("/")
    try:
        resp = requests.get(f"{api_base_url}/api/runs", timeout=2)
        if resp.status_code != 200:
            return set()
        runs = resp.json().get("runs", [])
    except Exception:
        return set()
    return {
        run_id for run_id in runs
        if run_id == normalized_base or run_id.startswith(f"{normalized_base}-v")
    }


def resolve_run_id(base: str, existing: set[str], include_base: bool = True) -> str:
    """
    Resolve a run ID with versioning.

    Args:
        base: Base run name.
        existing: Set of existing run IDs.
        include_base: Whether to include the base run if version 1 doesn't exist.

    Returns:
        The resolved run ID with version suffix if needed.
    """
    normalized = _normalize_run_id(base)
    base_without_suffix, framework_suffix = _split_framework_suffix(normalized)

    existing_versions = {
        version
        for candidate in existing
        if (version := _extract_version(candidate, normalized)) is not None
    }

    if include_base and 1 not in existing_versions:
        return normalized

    next_version = max(existing_versions, default=1) + 1
    return f"{base_without_suffix}-v{next_version}{framework_suffix}"


def _split_framework_suffix(candidate: str) -> tuple[str, str]:
    """
    Split a run ID into base name and framework suffix.

    Args:
        candidate: The run ID to split.

    Returns:
        Tuple of (base_name, framework_suffix).
    """
    # A Lightning suffix is "-pl" plus a dotted version ("exp-pl2.5.1"); names that
    # merely contain "-pl" ("sample-placement") have none.
    match = re.fullmatch(r"(.+?)(-pl\d+(?:\.\d+)*)?", candidate)
    if match is None:
        return candidate, ""
    return match.group(1), match.group(2) or ""


def _extract_version(candidate: str, base: str) -> Optional[int]:
    """
    Extract version number from a candidate run ID relative to base.

    Args:
        candidate: The candidate run ID.
        base: The base run name.

    Returns:
        Version number or None if no match.
    """
    candidate_base, candidate_suffix = _split_framework_suffix(candidate)
    base_without_suffix, base_suffix = _split_framework_suffix(base)

    if candidate_suffix != base_suffix:
        return None

    if candidate_base == base_without_suffix:
        return 1

    match = re.fullmatch(rf"{re.escape(base_without_suffix)}-v(\d+)", candidate_base)
    if match is None:
        return None
    return int(match.group(1))