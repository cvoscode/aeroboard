import re
import time
import io
import math
import pyarrow as pa
import pyarrow.flight as fl
from .schema import scalar_schema, image_schema, hyperparameter_schema

MAX_IMAGE_BYTES = 256 * 1024


def _quote_sql_string(value: str) -> str:
    return value.replace("'", "''")


def _extract_greptime_run_ids(payload: dict) -> set[str]:
    output = payload.get("output", [])
    if not output or "records" not in output[0]:
        return set()
    rows = output[0]["records"].get("rows", [])
    return {row[0] for row in rows if row and row[0]}


def _fetch_existing_run_ids(base: str) -> set[str]:
    import requests

    existing: set[str] = set()
    normalized_base = _normalize_run_id(base)
    quoted = _quote_sql_string(normalized_base)

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

    try:
        resp = requests.get("http://localhost:3000/api/runs", timeout=1)
        if resp.status_code == 200:
            runs = resp.json().get("runs", [])
            existing.update(run_id for run_id in runs if run_id == normalized_base or run_id.startswith(f"{normalized_base}-v"))
    except Exception:
        pass

    return existing


def _normalize_run_id(run_id: str | None) -> str:
    cleaned = (run_id or '').strip()
    return cleaned or f"run-{int(time.time())}"


def _with_lightning_suffix(run_id: str) -> str:
    try:
        import pytorch_lightning as pl
    except ImportError:
        return run_id

    suffix = f"-pl{pl.__version__}"
    return run_id if run_id.endswith(suffix) else f"{run_id}{suffix}"


def _split_framework_suffix(candidate: str) -> tuple[str, str]:
    match = re.fullmatch(r"(.*?)(-pl[^-]+)?", candidate)
    if match is None:
        return candidate, ""
    return match.group(1), match.group(2) or ""


def _extract_version(candidate: str, base: str) -> int | None:
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


def resolve_run_id(base: str, existing: set[str], include_base: bool = True) -> str:
    normalized = _normalize_run_id(base)
    base_without_suffix, framework_suffix = _split_framework_suffix(normalized)
    existing_versions = {
        version for candidate in existing
        if (version := _extract_version(candidate, normalized)) is not None
    }

    if include_base and 1 not in existing_versions:
        return normalized

    next_version = max(existing_versions, default=1) + 1
    return f"{base_without_suffix}-v{next_version}{framework_suffix}"

class AeroboardClient:
    """
    Python SDK client for sending telemetry data to the Aeroboard Rust backend.
    """
    def __init__(self, run_id=None, url="grpc://localhost:50051", batch_size=100):
        self.base_run_id = _normalize_run_id(run_id)
        self.url = url
        self.batch_size = batch_size
        self._scalar_buffer = []
        self._hyperparameter_buffer = []
        self._image_buffer = []
        self._resolved_run_id: str | None = None  # resolved after first connection
        try:
            self.client = fl.FlightClient(self.url)
        except Exception as e:
            print(f"[Aeroboard] Failed to connect to Flight server at {url}: {e}")
            self.client = None

    @property
    def run_id(self) -> str:
        if self._resolved_run_id is None:
            self._resolved_run_id = self._resolve_run_id(self.base_run_id)
        return self._resolved_run_id

    def _resolve_run_id(self, base: str) -> str:
        """
        Query GreptimeDB to find if a run with this base name already exists.
        If it does, auto-increment a version suffix: resnet -> resnet-v2 -> resnet-v3 …
        """
        normalized_base = _normalize_run_id(base)
        existing = _fetch_existing_run_ids(normalized_base)
        resolved = resolve_run_id(normalized_base, existing)
        if resolved != normalized_base:
            print(f"[Aeroboard] Run '{normalized_base}' already exists → using '{resolved}'")
        return resolved

    def log(self, metrics: dict):
        """
        Log scalar metrics.
        e.g. aeroboard.log({"loss": 0.5, "epoch": 1})
        """
        if self.client is None:
            return

        ts = int(time.time() * 1000)
        for k, v in metrics.items():
            if isinstance(v, (int, float)):
                self._scalar_buffer.append({
                    "timestamp": ts,
                    "run_id": self.run_id,
                    "name": k,
                    "value": float(v)
                })

        if len(self._scalar_buffer) >= self.batch_size:
            self.flush()

    def log_hyperparameters(self, hyperparameters: dict, step: int | float | None = None):
        if self.client is None:
            return

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

        if step is not None:
            self.log({"hyperparameters/step": float(step)})

        if len(self._hyperparameter_buffer) >= self.batch_size:
            self.flush()

    def log_image(self, name: str, image, *, format: str | None = None, timestamp_ms: int | None = None):
        if self.client is None:
            return

        encoded, inferred_format = _coerce_image_bytes(image, format=format)
        self._image_buffer.append({
            "timestamp": timestamp_ms or int(time.time() * 1000),
            "run_id": self.run_id,
            "name": name,
            "image_data": encoded,
            "format": inferred_format,
        })

        if len(self._image_buffer) >= max(1, math.ceil(self.batch_size / 4)):
            self.flush()

    def attach_lightning(self, run_name: str | None = None, *, batch_size: int | None = None):
        return AeroboardLightningCallback(
            client=self,
            run_id=run_name or self.base_run_id,
            url=self.url,
            batch_size=batch_size or self.batch_size,
        )

    def flush(self):
        """
        Force flush the currently buffered logs via Arrow Flight to the backend.
        """
        if self.client is None:
            return

        try:
            self._flush_scalar_buffer()
            self._flush_hyperparameter_buffer()
            self._flush_image_buffer()
        except Exception as e:
            print(f"[Aeroboard] Error flushing data via Flight: {e}")

    def _flush_scalar_buffer(self):
        if not self._scalar_buffer:
            return

        timestamps = [x["timestamp"] for x in self._scalar_buffer]
        run_ids    = [x["run_id"]    for x in self._scalar_buffer]
        names      = [x["name"]      for x in self._scalar_buffer]
        values     = [x["value"]     for x in self._scalar_buffer]

        batch = pa.RecordBatch.from_arrays(
            [
                pa.array(timestamps, type=pa.timestamp('ms')),
                pa.array(run_ids,    type=pa.string()),
                pa.array(names,      type=pa.string()),
                pa.array(values,     type=pa.float64())
            ],
            schema=scalar_schema
        )

        self._do_put(batch, scalar_schema, b"scalars")
        self._scalar_buffer.clear()

    def _flush_hyperparameter_buffer(self):
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

    def _flush_image_buffer(self):
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

    def _do_put(self, batch: pa.RecordBatch, schema: pa.Schema, descriptor_path: bytes):
        if self.client is None:
            return

        try:
            desc = fl.FlightDescriptor.for_path(descriptor_path)
            writer, _reader = self.client.do_put(desc, schema)
            writer.write_batch(batch)
            writer.done_writing()
            writer.close()
        except Exception:
            raise

    def close(self):
        self.flush()
        if self.client:
            self.client.close()


class AeroboardLightningCallback:
    def __init__(
        self,
        run_id: str | None = None,
        url: str = "grpc://localhost:50051",
        batch_size: int = 100,
        client: AeroboardClient | None = None,
    ):
        try:
            import pytorch_lightning as pl
        except ImportError as exc:
            raise RuntimeError("pytorch-lightning is required for AeroboardLightningCallback") from exc

        class _Callback(pl.Callback):
            def __init__(self, outer: AeroboardLightningCallback):
                super().__init__()
                self._outer = outer
                self._hparams_logged = False

            def _client(self) -> AeroboardClient:
                return self._outer.client

            def _maybe_log_hparams(self, trainer, pl_module):
                if self._hparams_logged:
                    return
                combined = {}
                module_hparams = getattr(pl_module, "hparams", None)
                if module_hparams is not None:
                    combined.update(_mapping_like(module_hparams))
                if getattr(trainer, "logger", None) is not None:
                    maybe_hparams = getattr(trainer.logger, "hparams", None)
                    if maybe_hparams:
                        combined.update(_mapping_like(maybe_hparams))
                if combined:
                    self._client().log_hyperparameters(combined)
                    self._hparams_logged = True

            def on_fit_start(self, trainer, pl_module):
                self._maybe_log_hparams(trainer, pl_module)

            def on_train_batch_end(self, trainer, pl_module, outputs, batch, batch_idx):
                self._maybe_log_hparams(trainer, pl_module)
                payload = {"step": float(trainer.global_step)}
                loss = outputs.get("loss") if isinstance(outputs, dict) else None
                if loss is not None:
                    payload["train/loss"] = float(loss.detach().item() if hasattr(loss, "detach") else loss)
                train_acc = trainer.callback_metrics.get("train_acc")
                if train_acc is not None:
                    payload["train/accuracy"] = float(train_acc.detach().item() if hasattr(train_acc, "detach") else train_acc)
                self._client().log(payload)

            def on_validation_epoch_end(self, trainer, pl_module):
                self._maybe_log_hparams(trainer, pl_module)
                metrics = trainer.callback_metrics
                payload = {"step": float(trainer.global_step)}
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
                self._client().flush()

        self.client = client or AeroboardClient(run_id=run_id, url=url, batch_size=batch_size)
        self.callback = _Callback(self)

    def __getattr__(self, item):
        return getattr(self.callback, item)


def _normalize_hparam_value(value) -> tuple[str, str] | None:
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
    return (str(value), value.__class__.__name__)


def _coerce_image_bytes(image, *, format: str | None = None, max_bytes: int = MAX_IMAGE_BYTES) -> tuple[bytes, str]:
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

    raise TypeError("Unsupported image type. Use encoded bytes, PIL image, numpy array, or torch tensor.")


def _maybe_compress_encoded_image(payload: bytes, image_format: str, *, max_bytes: int = MAX_IMAGE_BYTES) -> tuple[bytes, str]:
    if len(payload) <= max_bytes:
        return payload, image_format

    try:
        from PIL import Image
    except ImportError:
        return payload, image_format

    try:
        with Image.open(io.BytesIO(payload)) as image:
            converted = image.convert('RGB') if image.mode not in ('RGB', 'L') else image.copy()
            return _encode_pil_image(converted, 'webp', max_bytes=max_bytes)
    except Exception:
        return payload, image_format


def _encode_pil_image(image, image_format: str, *, max_bytes: int = MAX_IMAGE_BYTES) -> tuple[bytes, str]:
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
        rgb_image = image.convert('RGB') if getattr(image, 'mode', '') not in ('RGB', 'L') else image
        return _encode_pil_image(rgb_image, 'webp', max_bytes=max_bytes)

    return payload, normalized


def _mapping_like(value) -> dict:
    if isinstance(value, dict):
        return value
    if hasattr(value, "items"):
        return dict(value.items())
    return {k: getattr(value, k) for k in dir(value) if not k.startswith('_') and not callable(getattr(value, k))}
