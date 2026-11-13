# Aeroboard architecture

Aeroboard combines a Rust/Axum/Arrow Flight server, a Python SDK and CLI, and a
React/TypeScript dashboard. The root `pyproject.toml` packages the SDK from
`aeroboard-client/src/aeroboard_client`. Optional Lightning and Ray modules load
only when their integration classes are requested.

## Delivery and visibility

1. The SDK buffers scalar columns, then queues Arrow batches. Image inputs are
   snapshotted before returning to training; the sender encodes them as WebP.
   Checksums and IPC serialization run on the delivery worker. `flush()` waits
   for an acknowledgment; `configure_dashboard()` only queues its update.
2. Durable SDK mode journals operations in a SQLite outbox. Its writer notifies
   the sender when work becomes available. Consecutive compatible batches share
   a Flight stream, with an envelope in each batch's application metadata.
3. Flight validates schemas, checksums, store/run/session identity and assets,
   canonicalizes run names, and submits to a bounded persistence queue.
4. Persistence collects up to 64 jobs / 16 MiB in a 2 ms window. It reserves
   receipt identities in one transaction, writes the payload group, then commits
   locations, assets, summaries, scores and receipts together. A lost response
   can be retried without double-counting observations.
5. Only newly committed batches enter the live broadcast channel. WebSocket
   subscriptions select runs. Persistent images, audio and video carry metadata
   with empty binary payloads; clients retrieve media by asset ID. Other runs
   produce lightweight activity notices. Lagging subscribers resync history.

SIGINT and SIGTERM stop the servers and drain accepted writes. The CLI forwards
termination to its child. HTTP JSON and static responses can be compressed; binary IPC bypasses HTTP compression;
release assets also contain precompressed gzip files.

## Catalog

Each persistent data directory contains `catalog.sqlite` in WAL mode, with one
writer connection and four read connections. Read operations use snapshots.
Schema version 2 migrates existing receipts to indexed `session_id` and `kind`
columns and enforces `(run_uid, session_id, sequence)` uniqueness.

`run_lookup` indexes canonical names, project membership and tombstones.
`payload_locations` maps receipts to storage handles. `run_summaries`,
`run_scores`, `summary_batches` and `data_revisions` maintain durable reducers and
per-run revisions. Default score requests read snapshots; a changed objective
requires a history rebuild outside the write gate. The rebuilt snapshot is saved
only if the run revision has not advanced during the query.

Typed entity shapes document runs, sessions, assets and events. Catalog writes
emit entity kind, ID and revision; heartbeat writes are quiet. The dashboard loads
capabilities once, patches changed runs and reloads the affected metadata kinds.
Scores update from data notifications rather than periodic polling.

## Storage

The receipt layer delegates payload operations to `PayloadBackend`. It contains
no backend HTTP client or SQL implementation.

### Filesystem

Identified batches live in `segments/<run-uid>/<kind>/<number>.segment`, rolled at
64 MiB. Each record is a little-endian uint64 IPC-byte length followed by one IPC
stream. Catalog handles store file path, offset and length. Payload groups share
file fsyncs; directory entries are synced before the receipt commit. History
opens each segment once per query and reads its referenced ranges.

On startup, old committed `batches/<batch-id>.arrow` payloads are copied into
segments. Their handles commit before old files are removed, making interruption
safe to retry. Existing legacy JSONL, JSON-array and raw Arrow run directories
remain readable for envelope-less clients; these compatibility paths are covered
by tests. Newly identified SDK uploads always use segments.

### GreptimeDB

New scalars and hyperparameters use native `aeroboard_scalars` and
`aeroboard_hyperparameters` tables. Batch ID and row ordinal participate in their
keys, so retries overwrite the same rows while distinct same-millisecond steps
remain distinct. Numeric `value` supports external SQL queries; `value_bits`
retains exact non-finite diagnostics and signed zero. Batch reads share SQL
requests. Table creation and HTTP retries use the common Greptime client.

Media bytes use local segments, with catalog asset metadata. Keep the data
volume with the catalog when running Greptime mode. Startup migrates the old
hex-encoded `aeroboard_batches` blobs into native rows or media segments before
removing the old blobs. The older native tables remain readable through the
compatibility engine.

## History and charts

The identified scalar path filters, concatenates and sorts Arrow columns, with
an ordered-data fast path. Names are dictionary encoded for responses. IPC is
streamed in bounded chunks; CSV uses one storage snapshot rather than re-reading
history for each page. JSON and analytics consumers convert to rows only at
their API boundary. `max_points` reduces each run/metric series for overviews;
raw export remains full resolution.

Payload records and history snapshots each have a 128 MiB LRU cache. Snapshot
keys include the selected runs' data revisions and deletion state, plus the
metric/time filters and overview budget. Pages share one snapshot; cursor and
page size only slice its columns. Larger snapshots are served without retention.

The browser merges column vectors directly into per-series buffers. It starts
with 128 entries and grows geometrically. Wall time, value and step are stored;
relative/plot axes are derived. Sorted merge removes live/history overlaps, and
reconnect requests include a timestamp overlap window.

Charts use per-series subscriptions and versions. Ordinary scalar arrivals do
not tick the entire app. Chart cards mount canvases near the viewport, decimate
with min/max buckets and refine zoom windows from full-resolution history. Axis
bounds use timestamps at the ends of each series and maintained step extrema.
Chart settings context shares stable keyed actions and event markers. Preference
resolution is a pure function covering server, run, saved-view and local settings.
Controls, socket lifecycle, deletion and SDK integrations have separate modules;
component stylesheet files preserve the existing cascade.

## Verification and compatibility

The shared protocol and checksum vectors live in [protocol/](protocol/README.md).
The SDK negotiates protocol v2 and preserves v1 and older-server naming fallbacks.
Legacy storage and the `CORS_ORIGIN` alias remain supported intentionally.

Rust API fixtures live under `aeroboard-server/tests/`, unit tests beside modules,
Python tests under `tests/`, and frontend/browser tests under `aeroboard-web/tests/`.
CI runs formatting, lint, builds and all five browser suites. Release downloads
must match the published `SHA256SUMS` before extraction.

[benchmarks/ingest_history.py](benchmarks/ingest_history.py) measures both receipt
sizes (10k/1M), both batch sizes (100/5,000), a million-point history, and optional
browser navigation-to-chart. It creates disposable data directories and accepts
an original server binary for comparisons.
