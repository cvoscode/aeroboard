# Architecture

This page describes the runtime architecture of Aeroboard as implemented today.

## High-level topology

1. **Python SDK**: buffers telemetry and sends Arrow `RecordBatch`es over Arrow Flight from a background thread.
2. **Rust server**: one binary for Flight ingestion, live WebSocket streaming, persistence, the HTTP history API and the dashboard bundle.
3. **React dashboard**: loads history as Arrow IPC, then merges live Arrow batches from the WebSocket.
4. **Storage backend**: filesystem (default), GreptimeDB, or none (live-only).

## Data path

### Ingestion

`do_put` decodes the Flight stream with `arrow_flight`'s decoder (schema and
dictionary messages are handled correctly). The telemetry kind comes from the Flight
descriptor path, or from the column names when there is none. Every batch then goes
through `schema::normalize_batch`, which checks column names, casts compatible types
(e.g. `int64` values, `us` timestamps, `large_string`) to the canonical schema, and
rejects anything else with `INVALID_ARGUMENT`. Code after this point can rely on the
canonical layout.

### T-junction

`TJunction` splits each validated batch into two paths with different guarantees:

- **Persistence**: a bounded `mpsc` queue drained by one persistence loop. When it is
  full, ingestion waits, so a slow backend slows uploads down (backpressure) rather
  than losing data. The loop reports each batch's outcome back to the request that
  sent it, and `do_put` acknowledges an upload only once all of its batches are
  stored. A client's `flush()` therefore means "durably stored", and storage errors
  reach the client, which retries.
- **Live**: a `broadcast` channel for WebSocket subscribers. A slow browser lags,
  loses batches, and is told so with a `{"type": "lagged"}` message so it can
  re-fetch history. Each batch is IPC-encoded once and shared by all subscribers.

### Summaries

`SummaryCache` holds per-run last/min/max per metric plus hyperparameters. A run is
loaded from storage the first time it is requested; after that, the persistence loop
folds every stored batch into it, so `/api/runs/summary` doesn't rescan history.

### Shutdown

On Ctrl-C both servers stop accepting work and finish in-flight requests, the
persistence queue is closed and drained (up to 30 s), and the process exits.

## Frontend data model

High-frequency series live in typed-array stores held in refs, not React state.
`useAeroboard` exposes a `tick` counter, bumped at most once per animation frame,
that memos reading the stores depend on.

- History is fetched as one Arrow IPC response per run selection.
- `mergeHistoryRows` merges fetched history with live points that are already
  present, sorted by (timestamp, step) and de-duplicated, so loading history mid-run
  doesn't duplicate or reorder points.
- The step for each point comes from the `step` column. Data from older clients,
  which logged a separate `step` metric, is matched by timestamp.
- The WebSocket reconnects with exponential backoff. After a reconnect or a `lagged`
  message, the dashboard re-fetches what it shows.
- Images and other media are shown through URLs: Blob URLs for frames pushed live,
  `/api/history/media/content` for persisted ones.

## Security model

There is no authentication. The server listens on `127.0.0.1` and sends no CORS
headers by default; `AEROBOARD_HOST` and `AEROBOARD_CORS_ORIGINS` opt in to wider
access.

## When you change the architecture

A new telemetry type needs coordinated changes in:

1. Python schema and client buffering (`aeroboard-client/src/aeroboard_client/schema.py`).
2. The canonical schema in `aeroboard-server/src/schema.rs` and kind detection in `telemetry.rs`.
3. Storage serialization and queries in each backend.
4. HTTP response shaping if the UI reads it.
5. Frontend decoding and rendering.

Changing only one layer is the fastest way to create bugs in Aeroboard.
