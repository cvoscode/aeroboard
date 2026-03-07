import time
import pyarrow as pa
import pyarrow.flight as fl
from .schema import scalar_schema

class AeroboardClient:
    """
    Python SDK client for sending telemetry data to the Aeroboard Rust backend.
    """
    def __init__(self, run_id="default_run", url="grpc://localhost:50051", batch_size=100):
        self.base_run_id = run_id
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
        try:
            # Use GreptimeDB's SQL API to check for existing run_ids
            # We look for exact match or versioned matches.
            sql = f"SELECT DISTINCT run_id FROM scalars WHERE run_id = '{base}' OR run_id LIKE '{base}-v%'"
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
                if not existing or base not in existing:
                    return base

                # Find the highest version number
                version = 2
                while f"{base}-v{version}" in existing:
                    version += 1
                resolved = f"{base}-v{version}"
                print(f"[Aeroboard] Run '{base}' already exists → using '{resolved}'")
                return resolved
        except Exception:
            # GreptimeDB not reachable or table doesn't exist yet
            pass
        return base

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
