# Workspace verification

The implementation adds lifecycle, objectives, projects/views, comparisons, linked events/media, and durable recovery while retaining legacy ingestion and history readers. This record describes local verification on Linux with Python 3.12, a debug Rust build, and Greptime v0.12.0.

## Automated checks

- Rust: 45 tests passed; Clippy passed for all targets with warnings denied.
- Python: all 80 tests passed, including ten integration cases covering filesystem and real Greptime storage.
- Frontend: 20 tests, ESLint, and the production TypeScript/Vite build passed.
- Chromium: saved views opened in a fresh context with the same filter/log-axis settings; stale saves conflicted after live metadata refresh and reloaded safely. PNG/CSV downloads, comparison controls, media-to-chart selection, and desktop/mobile rendering passed. Light-theme contrast and responsive layout were checked.
- Documentation: the strict Sphinx/MyST directory-HTML build passed with warnings treated as errors.

The filesystem and real Greptime fixtures verify normal delivery, missing backend effects with pending receipts, durable backend effects with lost receipt commits, and lost acknowledgements. They restart the server to force cold reads, verify stable store identity, and check that each logical observation remains visible once. Additional cases cover abrupt SDK process exit followed by CLI recovery, same-millisecond media IDs and Range reads, edited annotation replay, saved-view conflicts, numeric filters, resumed segments, objective revision changes, hand-calculated mean/sample standard deviation, missing contributor coverage, and deletion tombstones.

Local outbox tests cover competing owners, quota exhaustion, checksum corruption, injected `ENOSPC`, permanent rejection ordering, local-flush boundaries, and shutdown ownership release. These tests simulate the relevant storage failure windows; they are not physical power-loss or filesystem-corruption certification.

## Workstation measurements

`benchmarks/workspace.py` logs two scalar metrics for 5,000 steps plus a warm-up sample: 10,002 observations, 100 scalar rows per batch. Setup and imports precede the logging timer. All three runs use the same current filesystem server; the baseline imports the SDK from commit `f700cda`, which writes the legacy storage representation. Each result is one local diagnostic run, not a statistical performance guarantee.

| SDK mode | Logging total | Median / p95 log call | Delivery total | Cold scorecards | Warm scorecards median | Peak process RSS |
| --- | --- | --- | --- | --- | --- | --- |
| Baseline SDK, memory queue | 15.9 ms | 0.85 / 1.45 µs | 190 ms | 39.4 ms | 2.90 ms | 775 MiB |
| Current SDK, memory queue | 34.5 ms | 0.82 / 1.32 µs | 1,533 ms | 21.2 ms | 2.94 ms | 777 MiB |
| Current SDK, disk outbox | 83.6 ms | 1.48 / 2.31 µs | 2,487 ms | 57.5 ms | 4.53 ms | 778 MiB |

The persistent run reached its local durable barrier in 521 ms after logging; 768,096 payload bytes remained queued at that snapshot. Aggregation took 83.3, 50.7, and 132.5 ms respectively. RSS includes the existing optional framework imports and does not represent queue size alone.

Versioned writes add immutable-file syncs and transactional receipt commits. The resulting delivery latency is higher in this small-batch debug workload. Logging remains buffered and fast; applications with higher sustained throughput should measure their own batch size, storage, and workload. A successful local flush establishes local capture, while remote delivery can continue afterward.

## Operational limits

- One server owns a catalog. Keep its data directory with backups even when telemetry lives in Greptime.
- Analysis accepts at most 50 runs and two million observations per request. The decoded batch cache is bounded to 128 MiB. At-step and aggregation queries still read raw observations; cold queries and legacy readers can cost more than warm scorecard lookups.
- CSV output streams to the browser using paginated storage reads. Pause writers when a fixed export snapshot is required.
- Delivery is at least once with idempotent visible effects. Live-only mode does not advertise durable replay. Ordinary logging retains an uncommitted memory window until a successful local flush.
- Cleanup waits for bounded in-flight transport operations before releasing outbox ownership. Hardware and operating-system failures beyond the injected cases require deployment-specific testing.
