# Architecture

Aeroboard consists of a Python SDK/CLI, a Rust server (Axum, Tokio and Arrow
Flight), and a React dashboard (Vite, Arrow JS and uPlot).

## Data path

The SDK buffers scalar columns and queues Arrow batches. Image encoding,
checksums and serialization run on delivery workers. Optional Lightning and Ray
modules load lazily. Durable mode journals operations in a SQLite outbox and wakes
the sender through notifications. Compatible uploads share Flight streams.

The server normalizes schemas, validates envelopes and submits batches to a
bounded persistence queue. The persistence loop groups writes, reserves receipt
identities, writes payloads, then commits visibility and derived data. It
acknowledges only durable writes. Retries preserve idempotent visible effects.

Newly committed batches enter the live broadcast channel. Browsers subscribe to
selected run names. Images, audio and video are fetched by URL in persistent
modes; live messages carry metadata. Other runs produce lightweight activity
notices. Reconnects and lag notices trigger history recovery with a timestamp
overlap window.

SIGINT and SIGTERM stop accepting work and drain the queue. The CLI forwards
termination to the server. Local access and same-origin requests are the defaults;
network exposure still requires explicit host/CORS configuration.

## Catalog and storage

SQLite WAL stores the server identity, typed entities, indexed receipts, run
lookups, payload locations, summaries, scores and data revisions. A single writer
and four snapshot readers allow queries during commits. Heartbeats are quiet;
other entity changes identify the affected kind, ID and revision.

Filesystem payloads use per-run/per-kind segments rolled at 64 MiB. Greptime
scalars and hyperparameters use native rows; media uses local segments. Startup
migrates the old immutable batch representation. See the [storage model](storage-model.md)
for recovery, migration and compatibility contracts.

Default scorecards and summaries read persisted reducers. Objective edits can
rebuild from raw history without holding the ingestion gate. Remaining caches
use selected run and metric-definition revisions.

## Dashboard

History remains columnar into per-series typed buffers. Each series has its own
version and subscription. Scalar updates redraw the affected chart cards and
axis controls; run and metric-list changes update the application structure.
Buffers begin at 128 entries, grow geometrically, and derive relative/plot axes.

Charts mount canvases near the viewport, decimate with min/max buckets, and fetch
full-resolution zoom windows. History and live overlaps merge in sorted order.
Persistent images load by URL; live image Blob URLs are capped. A small inactive
run cache bounds navigation memory.

Socket lifecycle, controls, deletion, data stores, media panels and comparison
panels are separate modules. Styles are split by component area. Capabilities
load once; typed metadata events patch records, and data events refresh scores.

## Verification

Rust unit tests live beside modules; API fixtures are in `aeroboard-server/tests`.
Python tests cover delivery and shared checksum vectors; frontend tests cover
stores, reducers and chart logic. CI runs all five browser suites, formatting,
lint and builds. The isolated ingestion/history benchmark supports baseline
binaries and optional browser navigation-to-chart measurement.
