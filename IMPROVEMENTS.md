# Improvement implementation and verification

Implemented against the review in `improvement.md`. The review itself is left
unchanged. Existing data and older-client compatibility remain supported.

| Review | Implementation |
| --- | --- |
| B1 | Catalog v2 migration, indexed receipt session/kind columns, unique run/session/sequence identity, indexed run lookup, shared identity validation. |
| B2 | Bounded group commits, shared segment durability and receipt transactions, multiple enveloped batches per Flight stream, SDK stream reuse. |
| B3 | Per-run/per-kind 64 MiB segments, backend payload trait, native Greptime scalar/hyperparameter rows, media segments, restart-safe flat-batch/blob migration, grouped segment/SQL reads. |
| B4 | Identified scalar history remains Arrow through filtering/concatenation/sort; ordered-input fast path; streamed IPC; CSV reads one snapshot; revision-keyed 128 MiB snapshot LRU reuses history across pages; shared query options and point filters. |
| B5 | Transactional persistent summaries/default scores and applied-batch markers; per-run data revisions; objective rebuilds outside ingestion gate; revision-scoped analysis caches; targeted deletion. |
| B6 | Quiet heartbeats, four WAL snapshot readers, indexed run identities/tombstones, targeted asset/tombstone queries, startup import, cached stale interval, normalized search, typed change events. |
| B7 | Generic entities, validated run/session/asset/event writes, typed identity and asset reads, shared thiserror application error enum with HTTP/Flight conversions. Extension metadata and flexible patches stay JSON. |
| B8 | Run subscriptions, metadata-only persistent media, committed-only live delivery, one WebSocket select loop. |
| B9 | SIGTERM drain and CLI forwarding, API/static compression (binary Arrow excluded), precompressed assets, release profile, unused direct dependencies removed, narrower Tokio features, fmt CI, ingestion validation helpers, concurrent Greptime run discovery, cached distinct batch run IDs and shrinking live-run registry, config mtime cache, fastrand jitter, API fixtures moved under tests. |
| P1 | Media/naming/query/error/buffer modules and lazy integration modules; deprecated legacy Lightning callback. |
| P2 | Background image snapshots/encoding and checksums, direct WebP arrays, method 4 encoding, scalar and hyperparameter column buffers, nonblocking dashboard configuration, negotiated bulk-buffer checksum v2. |
| P3 | Compatible Flight stream reuse, notification-based idle outbox, head-only idle lookup, consistent 60 s upload limits, shared TLS-aware HTTP session, cached metric definitions, summary endpoint for hyperparameters, logging. |
| P4 | Published SHA256SUMS and verified atomic downloads, signal forwarding, duplicate examples/lockfile removed, architecture/config/API docs refreshed, historical UI issues verified closed. |
| F1 | Per-series versions and subscriptions, memoized chart cards, stable keyed actions, shared event markers, memoized chart identities and numeric data versions, separate live axis/objective subscribers, O(series) bounds. Application tick is reserved for structure/media/settings changes. |
| F2 | Direct Arrow-vector grouping and sorted column merge; no point objects on the active history/live path; overlap recovery on reconnect. |
| F3 | Server overview limit preserving finite extrema and gaps, client extrema/gap decimation, single-series alignment shortcut and typed-array views for small wall-time traces, full-resolution zoom requests, viewport canvas mounting, duplicate remount keys removed. |
| F4 | 128-entry initial buffers, geometric growth, three stored columns with derived axes. |
| F5 | Selected-run socket delivery, bounded image Blob URLs, small inactive-run navigation cache and reloading on reselection. |
| F6 | Load capabilities once, typed metadata patches, event-driven scores, no periodic workspace/score polling. Advanced run filters still use their required server query. |
| F7 | Typed controls parser/hook, tested pure four-layer preference resolution, chart settings context, socket and deletion hooks, per-series store subscriptions, metric/hyperparameter summary indexes, separated component styles. Existing tab panels remain the presentation boundaries. |
| F8 | npm ci in CI/releases, all five browser suites in CI, named Arrow import. |
| Protocol / measurement | Shared specification and cross-language checksum vectors; isolated ingest/history/browser benchmark with optional original binary; recorded before/after samples. |

## Compatibility choices

Startup migrates the previous immutable flat-batch representation. The older
JSONL/JSON-array/raw-Arrow run readers and SDK naming fallback remain available
because compatibility tests exercise these contracts. They are isolated from the
new identified columnar path. Removing these readers, the older-server fallback
or the CORS alias would be a separate breaking change. Per-metric files and a new
schema-code generator were not required to obtain the measured improvements;
run/kind segments and shared protocol vectors cover those review concerns.
The workspace-managed `.aws/` directory is read-only and was left untouched.

The dynamic catalog extension fields and legacy compatibility queries still use
JSON/row conversion. Identified Arrow history avoids per-point Rust strings and
browser row objects. Full history is a columnar query snapshot; the network IPC
encoding and CSV output stream bounded chunks. Positional pagination remains
wire-compatible. Payload records and history snapshots have separate 128 MiB LRU
budgets. Snapshots larger than the budget are served without retention.

## Measurements

Single local Linux debug-build samples, both using the updated SDK/dashboard and
normal protocol negotiation. Receipt cases seed unrelated pending receipt
metadata; they measure table-growth sensitivity without generating a million
payload files. These are comparisons, not an SLA.

| Workload | Original server | Improved server |
| --- | ---: | ---: |
| 10k receipts, batch 100 | 7,640 points/s | 39,279 points/s |
| 1M receipts, batch 100 | 695 points/s | 42,088 points/s |
| 10k receipts, batch 5,000 | 105,076 points/s | 134,197 points/s |
| 1M receipts, batch 5,000 | 20,288 points/s | 93,722 points/s |
| 1M-point upload, batch 5,000 | 6.54 s | 3.55 s |
| Full 1M-point Arrow history | 1,169 ms | 705 ms |
| Chart overview HTTP | 1,105 ms / 36.8 MB | 369 ms / 370 KB |
| Navigation to chart | 2,050 ms | 896 ms |

The original server ignores `max_points`; its overview transfers all points.
All raw points remain available on the improved server. Samples and methodology
are in `benchmarks/results/improvements-2026-11-11.json`; reproduce with
`benchmarks/ingest_history.py`.

## Checks

- 50 Rust tests, Clippy with warnings denied, and rustfmt check.
- 87 Python tests, including all filesystem and Greptime v0.12 integration cases.
- 78 frontend store/chart tests, production build, and ESLint.
- All five Playwright suites, including the real workspace example workflow.
- Strict Sphinx documentation build.

Regression coverage includes v1 catalog migration/index selection, sequence
conflicts, grouped retries and restart, persistent counters, segment file count,
overview extrema/pagination, background image encoding, checksum verification,
protocol slices/nulls/nonfinite values, lazy imports, and live/history merges.
The follow-up audit also covers snapshot reuse without payload reads, revision
invalidation, gap-preserving overview budgets, all four preference layers, and
indexed hyperparameter updates. The measurement table records the original
implementation samples; the audit changes were verified by the regression suites.
