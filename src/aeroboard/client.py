import re
import time
import pyarrow as pa
import pyarrow.flight as fl
from .schema import scalar_schema


def _quote_sql_string(value: str) -> str:
    return value.replace("'", "''")


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
    existing_versions = {
        version for candidate in existing
        if (version := _extract_version(candidate, normalized)) is not None
    }

    if include_base and 1 not in existing_versions:
        return normalized

    next_version = max(existing_versions, default=1) + 1
    return f"{normalized}-v{next_version}"

class AeroboardClient:
    """
    Python SDK client for sending telemetry data to the Aeroboard Rust backend.
    """
    def __init__(self, run_id=None, url="grpc://localhost:50051", batch_size=100):
        self.base_run_id = _normalize_run_id(run_id)
        self.url = url
        self.batch_size = batch_size
        self._buffer = []
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
        import requests
        normalized_base = _normalize_run_id(base)
        try:
            # Use GreptimeDB's SQL API to check for existing run_ids
            # We look for exact match or versioned matches.
            quoted = _quote_sql_string(normalized_base)
            sql = f"SELECT DISTINCT run_id FROM scalars WHERE run_id = '{quoted}' OR run_id LIKE '{quoted}-v%'"
            resp = requests.get(
                "http://localhost:4000/v1/sql",
                params={"sql": sql, "db": "public"},
                timeout=1
            )
            if resp.status_code == 200:
                data = resp.json()
                # Handle different GreptimeDB response formats
                rows = []
                output = data.get("output", [])
                if output and "records" in output[0]:
                    rows = output[0]["records"].get("rows", [])
                
                existing = {r[0] for r in rows if r}
                resolved = resolve_run_id(normalized_base, existing)
                if resolved != normalized_base:
                    print(f"[Aeroboard] Run '{normalized_base}' already exists → using '{resolved}'")
                return resolved
        except Exception:
            # GreptimeDB not reachable or table doesn't exist yet
            pass
        return normalized_base

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
                self._buffer.append({
                    "timestamp": ts,
                    "run_id": self.run_id,
                    "name": k,
                    "value": float(v)
                })

        if len(self._buffer) >= self.batch_size:
            self.flush()

    def flush(self):
        """
        Force flush the currently buffered logs via Arrow Flight to the backend.
        """
        if not self._buffer or self.client is None:
            return

        timestamps = [x["timestamp"] for x in self._buffer]
        run_ids    = [x["run_id"]    for x in self._buffer]
        names      = [x["name"]      for x in self._buffer]
        values     = [x["value"]     for x in self._buffer]

        batch = pa.RecordBatch.from_arrays(
            [
                pa.array(timestamps, type=pa.timestamp('ms')),
                pa.array(run_ids,    type=pa.string()),
                pa.array(names,      type=pa.string()),
                pa.array(values,     type=pa.float64())
            ],
            schema=scalar_schema
        )

        try:
            desc = fl.FlightDescriptor.for_path(b"scalars")
            writer, reader = self.client.do_put(desc, scalar_schema)
            writer.write_batch(batch)
            writer.done_writing()
            writer.close()
        except Exception as e:
            print(f"[Aeroboard] Error flushing data via Flight: {e}")

        self._buffer.clear()

    def close(self):
        self.flush()
        if self.client:
            self.client.close()
