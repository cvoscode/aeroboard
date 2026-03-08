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

import re
import time
import io
import math
import warnings
import pyarrow as pa
import pyarrow.flight as fl
from typing import Optional, Dict, Union, List, Any
from .schema import scalar_schema, image_schema, hyperparameter_schema

MAX_IMAGE_BYTES = 256 * 1024  # 256 KB


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
    ):
        """
        Initialize the Aeroboard client.

        Args:
            run_id: Base run identifier for telemetry.
            url: gRPC Flight server address.
            batch_size: Batch size for telemetry buffering.
        """
        self.base_run_id = _normalize_run_id(run_id)
        self.url = url
        self.batch_size = batch_size
        self.api_base_url = (api_base_url or _default_api_base_url(url)).rstrip("/")
        self._scalar_buffer: List[Dict[str, Any]] = []
        self._hyperparameter_buffer: List[Dict[str, Any]] = []
        self._image_buffer: List[Dict[str, Any]] = []
        self._resolved_run_id: Optional[str] = None  # Resolved after first connection
        self._http_client: Optional[Any] = None  # For HTTP API queries

        # Initialize Flight client
        try:
            self.client = fl.FlightClient(self.url)
        except Exception as e:
            error_msg = f"[Aeroboard] Failed to connect to Flight server at {url}: {e}"
            print(error_msg)
            self.client = None

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
                timeout=2,
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

    def log(self, metrics: Dict[str, float]) -> None:
        """
        Log scalar metrics to the current run.

        Metrics are buffered and flushed to the backend when the batch size is reached
        or when explicitly calling `flush()`. Only numeric values are logged.

        Args:
            metrics: Dictionary of metric names to values.

        Example:
            >>> client.log({"loss": 0.5, "accuracy": 0.8, "epoch": 1.0})

        Raises:
            AeroboardClientError: If the Flight client is not connected.
        """
        if self.client is None:
            raise FlightConnectionError("No Flight connection available. Check server URL and connectivity.")

        ts = int(time.time() * 1000)
        for k, v in metrics.items():
            normalized = _coerce_scalar_metric_value(v)
            if normalized is not None:
                self._scalar_buffer.append({
                    "timestamp": ts,
                    "run_id": self.run_id,
                    "name": k,
                    "value": normalized,
                })

        if len(self._scalar_buffer) >= self.batch_size:
            self.flush()

    def log_metrics(
        self,
        metrics: Dict[str, Any],
        *,
        step: Optional[Union[int, float]] = None,
        prefix: Optional[str] = None,
    ) -> None:
        """
        Log numeric metrics with optional metric prefixing and shared step injection.

        This is convenient for vanilla training loops where you want to keep code like
        `client.log_metrics({"loss": loss, "accuracy": acc}, step=global_step, prefix="train")`.

        Args:
            metrics: Mapping of metric names to numeric values or scalar tensors.
            step: Optional shared step to attach as `step`.
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

        resolved_step = _coerce_scalar_metric_value(step)
        if resolved_step is not None:
            payload["step"] = resolved_step

        if payload:
            self.log(payload)

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
        for key, value in hyperparameters.items():
            normalized = _normalize_hparam_value(value)
            if normalized is None:
                continue
            encoded_value, value_type = normalized
            self._hyperparameter_buffer.append({
                "timestamp": ts,
                "run_id": self.run_id,
                "key": str(key),
                "value": encoded_value,
                "value_type": value_type,
            })

        # Log the step if provided
        if step is not None:
            self.log({"hyperparameters/step": float(step)})

        if len(self._hyperparameter_buffer) >= self.batch_size:
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
        self._image_buffer.append({
            "timestamp": timestamp_ms or int(time.time() * 1000),
            "run_id": self.run_id,
            "name": name,
            "image_data": encoded,
            "format": inferred_format,
        })

        # Images have smaller batch size (max 4 per batch due to size)
        if len(self._image_buffer) >= max(1, math.ceil(self.batch_size / 4)):
            self.flush()

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
        Force flush all buffered data to the backend.

        This sends all buffered metrics, hyperparameters, and images via Arrow Flight.
        Call this explicitly to ensure data is sent even if batch size isn't reached.
        """
        if self.client is None:
            return

        try:
            self._flush_scalar_buffer()
            self._flush_hyperparameter_buffer()
            self._flush_image_buffer()
        except Exception as e:
            error_msg = f"[Aeroboard] Error flushing data via Flight: {e}"
            print(error_msg)

    def _flush_scalar_buffer(self) -> None:
        """Flush scalar metrics buffer to the backend."""
        if not self._scalar_buffer:
            return

        timestamps = [x["timestamp"] for x in self._scalar_buffer]
        run_ids = [x["run_id"] for x in self._scalar_buffer]
        names = [x["name"] for x in self._scalar_buffer]
        values = [x["value"] for x in self._scalar_buffer]

        batch = pa.RecordBatch.from_arrays(
            [
                pa.array(timestamps, type=pa.timestamp('ms')),
                pa.array(run_ids, type=pa.string()),
                pa.array(names, type=pa.string()),
                pa.array(values, type=pa.float64()),
            ],
            schema=scalar_schema,
        )

        self._do_put(batch, scalar_schema, b"scalars")
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

        self._do_put(batch, hyperparameter_schema, b"hyperparameters")
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

        self._do_put(batch, image_schema, b"images")
        self._image_buffer.clear()

    def _do_put(
        self,
        batch: pa.RecordBatch,
        schema: pa.Schema,
        descriptor_path: bytes,
    ) -> None:
        """Send a RecordBatch to the Flight server."""
        if self.client is None:
            return

        try:
            desc = fl.FlightDescriptor.for_path(descriptor_path)
            writer, _reader = self.client.do_put(desc, schema)
            writer.write_batch(batch)
            writer.done_writing()
            writer.close()
        except Exception as e:
            error_msg = f"[Aeroboard] Error sending data to Flight: {e}"
            print(error_msg)
            raise

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
        self._http_client = __import__('requests')
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
        self._http_client = __import__('requests')

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
        self._http_client = __import__('requests')

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
        self._http_client = __import__('requests')

        url = f"{self.api_base_url}/api/runs"

        try:
            resp = self._http_client.get(url, timeout=10)
            if resp.status_code != 200:
                raise AeroboardClientError(f"HTTP {resp.status_code}: {resp.text}")
            data = resp.json()
            return data.get("runs", [])
        except Exception as e:
            raise AeroboardClientError(f"Failed to list runs: {e}")

    def close(self) -> None:
        """
        Close the client and flush any remaining data.
        """
        self.flush()
        if self.client:
            self.client.close()


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


def _quote_sql_string(value: str) -> str:
    """Escape single quotes for SQL."""
    return value.replace("'", "''")


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
    """Fetch existing run IDs from GreptimeDB or HTTP API."""
    import requests

    existing: set[str] = set()
    normalized_base = _normalize_run_id(base)
    quoted = _quote_sql_string(normalized_base)
    api_base_url = (api_base_url or "http://localhost:3000").rstrip("/")

    # Try GreptimeDB SQL query first
    try:
        sql = f"SELECT DISTINCT run_id FROM scalars WHERE run_id = '{quoted}' OR run_id LIKE '{quoted}-v%'"
        resp = requests.get(
            "http://localhost:4000/v1/sql",
            params={"sql": sql, "db": "public"},
            timeout=1,
        )
        if resp.status_code == 200:
            existing.update(_extract_greptime_run_ids(resp.json()))
    except Exception:
        pass

    # Try HTTP API as fallback
    try:
        resp = requests.get(f"{api_base_url}/api/runs", timeout=1)
        if resp.status_code == 200:
            runs = resp.json().get("runs", [])
            existing.update(
                run_id for run_id in runs
                if run_id == normalized_base or run_id.startswith(f"{normalized_base}-v")
            )
    except Exception:
        pass

    return existing


def _extract_greptime_run_ids(payload: dict) -> set[str]:
    """Extract run IDs from GreptimeDB query result."""
    output = payload.get("output", [])
    if not output or "records" not in output[0]:
        return set()
    rows = output[0]["records"].get("rows", [])
    return {row[0] for row in rows if row and row[0]}


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
    match = re.fullmatch(r"(.*?)(-pl[^-]+)?", candidate)
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