# Aeroboard improvement review

Reviewed at commit `e690ce6` by reading the source: the Rust server, the Python SDK/CLI, then the React dashboard. Nothing here was benchmarked or profiled; cost claims are derived from the code paths, so measure before and after any change you make for speed.

Each finding is tagged with a rough impact (**High / Medium / Low**) and effort (**S / M / L**).

## Top 10, in the order I would do them

| # | Change | Area | Impact | Effort |
|---|--------|------|--------|--------|
| 1 | Index `receipts` and stop scanning it on every batch and every query (B1) | server | High | S |
| 2 | Stop broadcasting `catalog_changed` for heartbeats; stop full workspace reloads (B6, F6) | server + web | High | S |
| 3 | Group-commit the persistence loop instead of 3 fsyncs per 100-row batch (B2) | server | High | M |
| 4 | Pick one storage format; compact per-batch files into per-run segments (B3) | server | High | L |
| 5 | Columnar history path end to end: no `Vec<HistoryPoint>` on the server, no row objects in the browser (B4, F2) | server + web | High | M |
| 6 | Downsample for charts (server `max_points`, client min/max decimation) (F3) | server + web | High | M |
| 7 | Replace the global `tick` with per-series subscriptions; memoize chart cards (F1) | web | High | M |
| 8 | Filter the live WebSocket by subscribed runs; strip image payloads (B8, F5) | server + web | Medium | M |
| 9 | Persist run summaries/scores instead of rebuilding from full history (B5) | server | Medium | M |
| 10 | Split `client.py`, lazy-load Lightning/Ray, move image encoding off the training thread (P1, P2) | SDK | Medium | S |

---

## Backend: Rust server

### B1. `receipts` has no usable index and is scanned constantly — High / S

The `receipts` table only has its primary key (`catalog.rs:101`). Three hot paths scan it in full:

- **Per ingested batch, twice.** `ingestion.rs:147` and `durable.rs:316` both run `WHERE run_uid=?1 AND sequence=?2 AND json_extract(CAST(body AS TEXT),'$.envelope.session_id')=?3`. With no index on `run_uid`, each is a table scan that also casts and JSON-parses bodies. Ingestion cost grows with the total number of batches ever stored, so ingest is quadratic over the life of the store.
- **Per history / hyperparameter query.** `DurableStore::receipts` (`durable.rs:203`) selects *every* committed receipt, parses each JSON body, and filters by run and kind in Rust.
- **Per run in the run list.** `with_delivery` (`catalog.rs:360`) runs a `COUNT(*)` with `json_extract` for each run, so `GET /api/run-records` is O(runs × receipts).

Fix: add real columns `session_id` and `kind`, plus `CREATE INDEX receipts_run ON receipts(run_uid, kind, committed)` and a `UNIQUE(run_uid, session_id, sequence)` constraint. The uniqueness constraint replaces both "sequence was reused" queries with a constraint violation, and the validation in `ingestion.rs` and `durable.rs` (currently duplicated almost line for line) collapses into one function. Bump `user_version` to 2 with a migration.

### B2. Persistence is one batch at a time with three fsyncs each — High / M

`persistence.rs` takes one job, then for an enveloped batch `DurableStore::append_batch` does:

1. a SQLite transaction (`synchronous=FULL`) to insert the receipt,
2. write file, `sync_all`, rename, then fsync the directory (`durable.rs:106-126`),
3. a second SQLite transaction to mark it committed.

The SDK's default `batch_size` is 100 rows and it sends one batch per `do_put` RPC, waiting for the ack. Throughput is therefore bounded by roughly three fsyncs per 100 points, serialized across *all* runs by the single loop, the `writer` mutex and the `analysis.gate` mutex.

Fix: drain up to N queued jobs (or a few milliseconds' worth) per iteration, write their payloads, then commit all receipts in one transaction and ack them together. Per-run parallelism becomes possible once the catalog work is one transaction per group. Also let the SDK send several enveloped batches per RPC; `ingestion.rs:67` currently rejects that.

### B3. Two storage formats are live at once; the newer one does not scale — High / L

`DurableStore` wraps the base engine, but an enveloped batch never reaches `base.append_batch` (`durable.rs:289-363`). Every current SDK sends envelopes, so:

- **Filesystem mode:** the JSONL layout documented in `ARCHITECTURE.md` and `filesystem.rs` is only written by legacy clients. New data lands as one Arrow IPC file per batch in a single flat `batches/` directory shared by all runs. A 1M-point run at the default batch size is 10,000 files; reading its history opens each one (the 128 MiB cache is FIFO and `get` never refreshes it).
- **GreptimeDB mode:** the `scalars`/`images`/… tables are no longer written. Whole Arrow batches are hex-encoded (2× size) into a `payload STRING` field of `aeroboard_batches`. GreptimeDB is being used as a blob store, so its SQL filtering, time index and compression are unused, and the data is no longer queryable from outside Aeroboard.
- Every query still reads **both** formats and merges them, and `DurableStore` matches on `storage_mode` in five places plus owns its own `reqwest::Client` and its own retry-less `greptime_sql` (duplicating `GreptimeStore::sql`). `put_bytes` also sends `CREATE TABLE IF NOT EXISTS` before every single batch write (`durable.rs:130`).

Recommended direction:

1. Make the idempotency layer (receipts) independent of the payload layout. Dedupe by `batch_id` in the catalog, then hand the batch to the base engine's native append.
2. Filesystem: append to per-run, per-kind segment files (Arrow IPC or Parquet), rolled by size. Keep the batch file only as a short-lived write-ahead record, or drop it.
3. Greptime: write rows into the native tables again; put media bytes somewhere other than hex strings.
4. Replace the `match storage_mode` blocks with a small trait so `DurableStore` holds no backend-specific code.
5. Provide a one-off migration for legacy `index.json`, raw `*.arrow` and JSONL layouts, then delete the three legacy read paths in `filesystem.rs`.

If a full rewrite is too much now, a background compactor that merges a finished session's batch files into one per-run file gets most of the read-side benefit.

### B4. History is materialized as row structs and re-read for every page — High / M

- `HistoryPoint` owns two `String`s per point. A 1M-point query allocates 2M strings, sorts the vector, then `history_rows_to_arrow` re-encodes it into dictionary columns. The data starts as Arrow batches on disk; it should stay columnar (concat + filter + sort indices with `arrow::compute`) and be written straight to the response.
- Pagination is offset-based on top of a full read. `filesystem.rs` parses the whole JSONL index and `DurableStore::query_history` re-reads every receipt and batch for each page. `export_csv` (`analytics.rs:187`) pages 5,000 rows at a time, so exporting N rows costs O(N²/5000) I/O. Stream the export from one pass instead.
- A query for one metric still reads every metric of the run. Per-metric (or at least per-run) segments from B3 fix that.
- The Arrow response is built fully in memory. Stream record batches with `Body::from_stream`.
- `apply_history_query` and `apply_image_query` are the same function over two types; a small trait with `ts_ms()/run_id()/name()` removes the copy. The same goes for `HistoryQueryOptions` vs `ImageQueryOptions`.

### B5. Summaries and scorecards are rebuilt from full history — Medium / M

- `SummaryCache::summaries` loads the complete history of every requested run on a cold cache, with no limit, as one `Vec<HistoryPoint>`. The dashboard asks for summaries of **all** runs on load (`useAeroboard.ts:346`), so the first page view after a server restart scans the entire store.
- `delete_run` calls `summaries.clear()` and `analysis.clear()` (`workspace.rs:427`), throwing away every run's cache to remove one.
- `scorecards` holds `analysis.gate` while it loads up to 2M points (`analytics.rs:464-491`). The persistence loop takes the same mutex for every batch, so a cold scorecard request stalls all ingestion.
- The `entries` cache key embeds `revision`, which `invalidate()` bumps on every persisted batch. During training the cache never hits, and the `at_step`/`segment` path recomputes from full history on every poll (the dashboard polls every 3 s).
- Cache keys are built with `{definitions:?}:{selected:?}`, i.e. a Debug dump of whole entities.

Fix: persist per-run metric stats (last/min/max/best/first-ts) in the catalog, updated in the same transaction that commits the receipt. Summaries and the default scorecards then become lookups, survive restarts, and need no gate. Invalidate per run, not globally. Key the remaining caches on `(run uid, run data revision, definition revisions)`.

### B6. Catalog access patterns — High / S–M

- **Heartbeats wake every browser.** `heartbeat` uses `catalog.write`, which broadcasts `catalog_changed` (`catalog.rs:197`). Each open tab responds by reloading capabilities, projects, all run records, views and metric definitions. With R active runs that is a full reload in every tab every 15/R seconds. Use `quiet` for heartbeats, or include the changed kind/id in the message so clients can patch.
- **One connection behind one mutex.** Reads (`catalog.read`) block on writes and on each other. WAL mode allows concurrent readers; use a small read pool (or `r2d2`/`deadpool`) and keep one writer.
- **Everything is a full list.** `deleted()`, `run_by_id`'s fallback, `known_runs`, `import_runs`, `runs_handler` and the legacy-ingest check all call `catalog::list(db, "run", None)`, which deserializes every run's JSON body. `DurableStore::query_history` calls `deleted()` twice per query; `runs_handler` lists all runs twice. Add an indexed `run_id` column (or a `runs` table with `run_id UNIQUE`, `deleted`, `project`) and query for what is needed.
- **`import_runs` on read paths.** `/run-records`, `/run-records/{id}`, `/scorecards` and `/comparison/aggregate` each call `known_runs`, which lists the storage backend (a `readdir` plus one `manifest.json` read per run on filesystem, five `SELECT DISTINCT` queries on Greptime). Import once at startup and when a legacy batch for an unknown run arrives.
- `stale_interval(db)` runs a settings query per run inside `with_delivery`; the value is already on `Catalog`.
- Text search does `e.data.to_string().to_lowercase()` per run and lowercases the needle per row.
- The legacy ingest path checks for deleted runs in `ingestion.rs:181` and again in `durable.rs:290`.

### B7. Stringly-typed JSON entities — Medium / M (maintainability)

Runs, sessions, assets and events are `serde_json::Value` blobs addressed by string keys (`data["deleted"] == true`, `data["run_id"].as_str().unwrap_or("")`). Typos compile, missing fields silently become `""`/`0`, and nothing documents the shape. Define `RunRecord`, `Session`, `Asset`, `Event` structs with serde and keep `Entity<T>` generic. This pairs naturally with the column changes in B1/B6.

Related: `CatalogError` is used as the general HTTP error in `analytics.rs` and `workspace.rs`, and storage errors are `Box<dyn Error>` flattened to strings and reported as 500 or `UNAVAILABLE`. A single `AppError` enum (`thiserror`) with `IntoResponse` and `Into<tonic::Status>` would remove the repeated `map_err(|e| CatalogError(INTERNAL_SERVER_ERROR, e.to_string()))` and the four copies of the empty-`HistoryResponse` error body in `history.rs`.

### B8. Live stream — Medium / M

- Every WebSocket client receives every batch of every run. Add a subscribe message (`{"type":"subscribe","runs":[...]}`) and filter server-side; send only a lightweight "run X has activity" notice for the rest.
- Only audio/video payloads are stripped (`streaming.rs:44`). Image bytes are pushed to all tabs even though they are persisted and fetchable by URL. Strip them too in persistent modes.
- Batches are broadcast before they are persisted or deduplicated, so a client retry is pushed twice (the browser dedupes by `batch_id`) and a batch that fails persistence was already shown. Broadcasting from the persistence loop after commit gives exactly-once live delivery and lets the server stop relying on client-side dedupe.
- `handle_socket` spawns two tasks and aborts one when the other ends; a single `select!` loop over the socket and the two receivers is simpler.

### B9. Smaller server items — Low–Medium / S

- **No SIGTERM handling.** `main.rs` only listens for Ctrl-C, so `docker stop`, systemd and `kill` skip the 30 s drain. Add `tokio::signal::unix::signal(SignalKind::terminate())`.
- **No response compression.** `tower-http` is built without `compression-*`. JSON listings and the JS bundles (ag-grid, arrow) go out uncompressed. Add `CompressionLayer` for the API and `ServeDir::precompressed_gzip()/br()` for assets.
- **No `[profile.release]`.** Add `lto = "thin"`, `codegen-units = 1`, `strip = true` for the shipped binary.
- **Likely unused dependencies:** I found no direct use of `axum-extra` or `prost` in `src/`. Confirm with `cargo machete` and drop them. `tokio = { features = ["full"] }` can be narrowed.
- **Formatting is not enforced.** Parts of `ingestion.rs`, `durable.rs`, `workspace.rs`, `catalog.rs` and `analytics.rs` are dense single-line blocks that `cargo fmt` would reflow; CI runs clippy but not `cargo fmt --check`.
- `ingestion.rs::ingest` is a 210-line function mixing decoding, envelope validation, catalog checks and publishing. Split into `validate_envelope`, `check_identity` (one catalog call), and `publish`.
- `GreptimeStore::list_runs` issues five sequential `SELECT DISTINCT` queries; run them concurrently or keep the run list in the catalog.
- `TJunction::register_batch_runs` takes the mutex and walks every row of every batch. A batch nearly always has one run; check the first value and fall back to a scan only if the column is not constant. `live_run_ids` also never shrinks except on delete-all.
- `/api/config` re-reads and re-parses the TOML on every request; cache on mtime.
- `retry.rs` derives randomness from `RandomState`; fine, but `fastrand` is one tiny dependency and clearer.
- `tests.rs` is 1,428 lines in one module inside `src/`. Move HTTP-level tests to `tests/` and keep unit tests next to the code they cover.

---

## Backend: Python SDK and CLI

### P1. `client.py` is 1,891 lines and imports heavy frameworks eagerly — Medium / S

- `_resolve_lightning_integration()` and `_resolve_ray_tune_integration()` run at import time, so `import aeroboard_client` imports Lightning (and therefore torch) and Ray whenever they are installed. Move the integrations to `aeroboard_client/integrations/{lightning,ray}.py` and resolve them lazily (module `__getattr__`).
- Split the rest: `client.py` (the client), `_media.py` (image/audio/video coercion), `_naming.py` (run-id versioning), `_query.py` (HTTP read helpers).
- `AeroboardLightningCallback` hardcodes `train_acc`/`val_loss`/`val_acc` and wraps an inner class via `__getattr__`; the logger already covers this use and the callback could be deprecated.
- Run-id versioning (`resolve_run_id`, `_split_framework_suffix`, `_extract_version`) is implemented in both Python and Rust. The server is authoritative; the client copy only serves the fallback for servers without `/api/runs/resolve`. Decide whether those servers are still supported and delete one copy if not.

### P2. Work done on the training thread — Medium / S–M

- `_encode_pil_image` encodes WebP with `method=6` (the slowest setting) and may try three quality levels; numpy/tensor inputs default to PNG and are then re-encoded to WebP if over 256 KB. All of this runs synchronously inside `log_image`. Use `method=4`, encode straight to WebP for arrays, and do the encoding on the sender thread.
- `batch_checksum` (`_protocol.py`) walks every value through `to_pylist()` and `struct.pack` in Python. For scalar batches that is 5 Python-level hash updates per row on the logging thread. Hash the Arrow buffers (or the IPC body bytes) instead; that needs the matching change in `telemetry.rs::batch_checksum`, which would also get faster.
- Buffers are lists of per-row dicts, later unzipped with five list comprehensions per flush. Keeping five parallel lists (or `array.array`) avoids the dict churn.
- `configure_dashboard` calls `flush()`, which blocks up to `flush_timeout` waiting for the server ack.

### P3. Delivery — Medium / S–M

- One `do_put` RPC per batch, strictly sequential, each waiting for the server's fsyncs (see B2). Reusing one stream for several batches, or letting the sender coalesce queued scalar batches, raises the ceiling substantially.
- `OutboxSender._send` polls `outbox.next()` every 100 ms when idle, and `next()` selects the whole body blob and re-hashes it each time. `wait_idle` runs `outbox.status()` (three SQL queries) every 100 ms while holding the condition. Have the writer notify the sender, and select only the ordinal until an item is actually sent.
- Timeouts disagree: the direct path allows 60 s per Flight call, `OutboxTransport` allows 5 s. A 32 MB clip over a slow link can exceed 5 s and be retried forever.
- `_resolve_run_id` and `_fetch_existing_run_ids` use bare `requests` calls; the latter ignores `ssl_verify`. Route everything through the tracker's session.
- `RunTracker.deliver("DEFINE")` fetches all metric definitions before each define.
- `query_hyperparameters` requests `/api/history` (all scalar rows for the run) only to read `run_summaries`; call `/api/runs/summary`.
- Errors are reported with `print`. Use `logging`/`warnings` so users can route or silence them.

### P4. CLI/runtime and repo hygiene — Low / S

- `runtime.py` downloads the server binary and web bundle without verifying a checksum. Publish `SHA256SUMS` with releases and check it.
- `aeroboard start` runs the server with `subprocess.run`; combined with B9's missing SIGTERM handler, stopping the CLI by anything other than Ctrl-C skips the drain.
- Leftovers to delete or move: `aeroboard-client/torch_example.py` (diverged copy of `examples/torch_example.py`), `aeroboard-client/lightning_example.py`, `aeroboard-client/test.py`, `examples/test.py`, `aeroboard-client/uv.lock` (the root `pyproject.toml` packages the client), empty `src/` and `.aws/`.
- `Issues.md` lists three open "critical" UI issues from 2026-05-08 that predate the current dashboard; confirm and close or remove.
- `ARCHITECTURE.md` still describes the JSONL layout as *the* filesystem format and does not mention the catalog, receipts or batch files.
- `.claude/CLAUDE.md` describes the stack as "Python / PyTorch" and was indexed at an older commit.

---

## Frontend: React dashboard

### F1. One `tick` re-renders the whole app every frame — High / M

`useAeroboard` keeps data in refs and bumps a single `tick` at most once per animation frame. `App` depends on it, so while any run is logging:

- `App` (849 lines) re-renders, and every `useMemo` keyed on `tick` recomputes.
- `getGlobalXBounds` (`scalarStore.ts`) loops over **every point of every series** of the selected runs on each tick. The arrays are sorted by time, so wall/rel bounds are the first and last element; keep a running min/max for steps.
- `MetricChartGrid` and `SortableMetricChart` are not memoized and receive fresh closures, so every chart card re-renders. `LineChart` avoids reprocessing via `dataSignature`, but still rebuilds several joined-string keys per chart per frame, and `markers` is recomputed with `events.flatMap` for each chart.

Fix: turn the scalar store into an external store with per-`(run, metric)` version counters and subscribe with `useSyncExternalStore` inside the chart. Only charts whose series changed re-render; `App` re-renders only when the run or metric list changes. Wrap `SortableMetricChart` in `memo` and pass stable callbacks that take `groupKey`.

### F2. History loading converts columns to objects to columns — High / M

`fetchScalarHistory` reads the Arrow table and builds one JS object per point, calling `String(runs.get(i))` and `String(names.get(i))` on dictionary columns for every row. `mergeHistoryRows` then converts the existing typed arrays back into row objects, concatenates, sorts, builds a `Set` of string keys (one template string per point) and re-inserts each point through `ingestPoint`, which calls `updateMetricSummary` (a linear `find` over the metric list) per point.

For a 1M-point run that is several million short-lived objects and strings. Instead: read the dictionary indices, bucket row indices per `(run, name)`, and copy slices of the timestamp/value/step columns into the store's typed arrays. Dedupe against live points by comparing the tail of each series (live points are newer than history except for the overlap window). Update summaries once per series from the column min/max.

Also: `resync()` re-downloads the full history of every loaded run after any reconnect or `lagged` notice. Pass `start_ms` set to the last timestamp held per run.

### F3. Charts process and draw every point — High / M

`buildProcessedData` runs whenever a series gains a point. Each time it copies all points into plain arrays, unions all x values into a `Set`, sorts them, builds a `Map` per series and maps the aligned arrays. `getFocusedRanges` then builds a `{x, y}` object per point. That is O(N log N) work and O(N) allocation per chart per frame for a live run, and uPlot draws all N points.

- Single-series charts (the common single-run case) need no alignment: hand uPlot `subarray(0, count)` views of the store's typed arrays.
- Decimate to about two points per pixel column (min/max per bucket) before giving data to uPlot; recompute on zoom.
- Add a `max_points` parameter to `/api/history` so long finished runs arrive pre-reduced, with full resolution fetched for a zoomed window.
- Mount uPlot only for charts in or near the viewport (`IntersectionObserver`); off-screen expanded charts currently update on every batch.
- `LineChart` is remounted by `key={revisionKey}` from the parent and its wrapper by `key={axisRevisionKey}`, while Effect 1 also rebuilds on the same inputs. One mechanism is enough.

### F4. Per-series memory — Medium / S

`makeEmptySeries` preallocates five `Float64Array(4096)`, about 164 KB per `(run, metric)` before the first point. A 50-trial sweep with 20 metrics visible over the live socket is roughly 160 MB of mostly empty buffers. Start at 64–256 and grow. `relTimes` and `plotX` are derivable (`wall − start`, `step ?? rel`), so three arrays suffice. Combined with F5 the store would hold only runs the user is looking at.

### F5. The browser keeps everything the socket sends — Medium / M

Live batches for all runs are ingested into the stores whether or not the run is selected, and live image frames become Blob URLs that are only revoked on delete or unmount. A tab left open during a sweep grows without bound. With the subscription filter from B8, only selected runs are streamed; additionally cap live image blobs per series (keep the newest N, fall back to the content URL for older frames).

### F6. Polling and reload fan-out — High / S

- `useWorkspace` reloads capabilities, projects, **all** run records (paged, including archived), views and definitions every 15 s and on every `aeroboard:catalog` event, which heartbeats trigger (B6).
- `useScores` polls `/scorecards` every 3 s for as long as runs are selected, live or not; each call does the server-side work described in B5/B6.
- `WorkspacePanel` runs its own `/run-records` query in addition to the one in `useWorkspace`.

Fix: have the server send typed change events (`{"type":"run_changed","id":…,"revision":…}`), patch the affected record, and refetch scorecards when the server's `data_revision` changes instead of on a timer. Capabilities need loading once.

### F7. `App.tsx` structure — Medium / M (maintainability)

- `controls` is a `Record<string, unknown>` validated inline in about fifteen places (`controls.xAxisType === "wall" || …`). Move this to one `parseControls(raw): Controls` in `lib/` and a `useControls(scope, viewControls)` hook that returns typed values and setters.
- The four-layer preference resolution (server TOML → run config → saved view → localStorage) is spread across six memos. It is pure logic; extract it to `lib/dashboardDefaults.ts` beside the existing helpers, where it can be unit-tested.
- `MetricChartGrid` takes about 35 props, mostly passed straight through to each chart. A context (or the store from F1) for chart settings removes most of the drilling.
- Delete handling (three handlers and the confirm state) and the tab renderers are self-contained and can move to `useRunDeletion` and per-tab components.
- Dead or duplicate bits: `effectiveActiveTab` is an alias of `activeTab`; `useDashboardPreferences` still persists `widthMode`, which `App` now reads from `controls`; `asObject`/`names`/`flags` in `App.tsx` duplicate `asRecord`/`asStringArray` in `useDashboardPreferences.ts`.
- `useAeroboard` (671 lines) mixes the socket, four stores, load-state tracking and delete logic. Split into `useLiveSocket`, a store module (F1) and loaders.
- `updateMetricSummary` and `setHyperparameter` use `Array.find` per update; keep `Map`s in the store and produce arrays on read.
- `App.css` is a single 1,783-line file. Co-locate styles with components (CSS modules work with Vite out of the box) so unused rules become visible.

### F8. Tooling — Low / S

- The `web` CI job runs `npm install`; use `npm ci` so the lockfile is honored.
- Only `test:browser` (workspace) runs in CI. The other four Playwright suites (`runs`, `groups`, `analysis`, `grid`) do not.
- `apache-arrow` is imported as `import * as arrow` in `useAeroboard.ts`; the named `tableFromIPC` import (already used in `api.ts`) tree-shakes better.

---

## Cross-cutting

- **One protocol, defined once.** Column layouts live in `schema.rs`, `schema.py` and implicitly in the frontend's `getChild` calls; the checksum algorithm is hand-mirrored in `_protocol.py` and `telemetry.rs`. A short protocol spec with shared test vectors (a few batches with expected checksums, checked by both test suites) would catch drift.
- **Legacy surface.** The server supports pre-`step` scalars, three filesystem layouts, pre-0.2 Greptime tables, the `CORS_ORIGIN` variable, envelope-less ingestion and `/api/runs/resolve`; the SDK supports servers without lifecycle or resolve. The project is at 0.1.0. Dropping what no deployed client needs would remove a meaningful share of the branching in `ingestion.rs`, `durable.rs`, `filesystem.rs`, `client.py` and `scalarStore.ts`.
- **Measure.** `benchmarks/workspace.py` exists; add an ingest benchmark (points/s at batch sizes 100 and 5,000 with an existing store of 10k and 1M receipts) and a history-load benchmark (1M-point run, server time and browser time-to-chart). B1, B2, B4, F2 and F3 should all move those numbers visibly, and they give a regression guard afterwards.
