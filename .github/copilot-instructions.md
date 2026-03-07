# Aeroboard AI Coding Instructions

Expert guidance for the Aeroboard "Arrow-Native" telemetry platform.

## Architecture Overview
Aeroboard uses a **zero-copy/minimal-copy** telemetry path from Python to the Browser.
1. **Ingestion:** Python (`pyarrow`) → gRPC Arrow Flight → Rust (`arrow-rs`).
2. **The "T-Junction" (`aeroboard-server/src/t_junction.rs`):** A `tokio::sync::broadcast` hub that splits the stream.
3. **Path A (Persistence):** Rust formats Arrow batches into Influx Line Protocol → Async POST to **GreptimeDB**.
4. **Path B (Live Stream):** Rust encodes batches to Arrow IPC → WebSockets (`/ws/live`).
5. **Frontend:** React + `apache-arrow` (JS) → Extract `Float64Array` → `uPlot` (Canvas).

## Component Specifics

### Rust Backend (`aeroboard-server`)
- **Web Framework:** `axum` for HTTP/WS, `tonic` for gRPC/Flight.
- **Concurrency:** Uses `tokio::spawn` for the persistence loop and individual WS connections.
- **Arrow Handling:** Always use `Arc<RecordBatch>` to keep clones cheap.
- **Persistence:** Metadata and scalars are stored in GreptimeDB. Images/Distributions are currently live-only.

### Python Client (`aeroboard-client`)
- **Core Loop:** Buffer metrics in `self._buffer`, then `flush()` as a `pa.RecordBatch`.
- **Run ID Resolution:** The client auto-versions `run_id` (e.g., `experiment` -> `experiment-v2`) by querying GreptimeDB on `init()`.
- **Schema:** Defined in `src/aeroboard_client/schema.py`. Any new metric type needs a schema update here and in the Rust `persistence.rs`.

### Web Dashboard (`aeroboard-web`)
- **Data Hook:** `useAeroboard.ts` is the heart. It manages a `storeRef` (Record of typed arrays) to avoid React state overhead for high-frequency updates.
- **History:** Fetch from `GET /api/history` on mount.
- **Rendering:** `uPlot` components should receive the pre-allocated `Float64Array` from the store.

## Developer Workflows
- **Setup Everything:** `bash setup.sh`
- **Run Dev Environment:** `uv run aeroboard start` (starts both server and dashboard).
- **Format/Lint:** 
  - Rust: `cargo fmt` inside `aeroboard-server`.
  - Python: `ruff check` in root.
  - Web: `npm run lint` in `aeroboard-web`.

## Conventions
- **Naming:** Metrics named `step` or ending in `/step` are treated as the x-axis index for alignment in the frontend.
- **Data Types:** Prefer `Float64` for all scalars to match the `uPlot` expectations.
- **Errors:** Don't let a failed GreptimeDB connection crash the server; log it and continue Path B (Live Stream).
