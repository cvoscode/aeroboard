# Storage Model

Aeroboard routes all persistence and history queries through the Rust `StorageEngine` trait.

## Why the storage abstraction matters

The live telemetry path and the persistence path are intentionally separated. That design only stays clean if storage-specific behavior is isolated behind a narrow server-side interface.

The storage engine currently exposes operations for:

- appending telemetry batches
- querying scalar history
- querying hyperparameters
- querying image history
- listing runs
- clearing history

## Supported storage modes

## Filesystem

Filesystem storage is the default mode.

The server writes run-scoped data under the configured data directory, currently defaulting to `.aeroboard`.

The layout:

```text
.aeroboard/
  runs/
    <encoded-run-id>/
      manifest.json                     {"run_id": "..."}
      scalars/
        index.jsonl                     {"t": <ms>, "n": "<metric>", "v": <value>, "s": <step>}
      hyperparameters/
        index.jsonl                     one entry per logged key; the last one wins
      images/ | audio/ | video/
        <encoded-stream-name>/
          index.jsonl                   {"ts_ms", "run_id", "name", "format", "file_name"}
          <timestamp>-<uuid>.<format>   the raw payload
```

Important details:

- indexes are append-only [JSON Lines](https://jsonlines.org/): persisting a batch costs time proportional to the batch, not to the length of the run
- a manifest is written per run so the run can be enumerated later
- media payloads are written before their index entry, so an index never points at a missing file
- non-finite scalar values are stored as the strings `"NaN"`, `"inf"` and `"-inf"`
- all file I/O runs on tokio's blocking thread pool, never on the async workers
- after a crash, a torn last line is skipped with a warning; the rest of the run stays readable
- older layouts are still read: `index.json` arrays and per-batch `*.arrow` files written by Aeroboard < 0.2

Filesystem mode is usually the best place to validate new telemetry behavior because it keeps the persistence path transparent.

## GreptimeDB

Greptime mode writes through the InfluxDB line-protocol endpoint and reads with SQL over HTTP.

Before the first write the store creates its tables with `run_id` plus the metric (or hyperparameter key) as the primary key and `append_mode = 'true'`. Rows from different runs therefore never collide, and several steps logged within one millisecond are all kept. **Delete All Runs** drops the tables; they are recreated on the next write. Writes are retried with jittered exponential backoff on transport errors, 429 and 5xx responses.

Use it when you specifically want database-backed persistence or need to validate the Greptime-specific code path.

## Live-only

Live-only mode intentionally accepts batches without persisting them.

Expected semantics:

- ingestion succeeds
- live streaming still works
- history queries return empty collections
- delete-style persistence operations should not pretend they succeeded against nonexistent stored data

## Query semantics shared across backends

Every backend implements the same filter semantics:

- `run_id` accepts a comma-separated list
- `metric` accepts a comma-separated list
- `start_ms` and `end_ms` are inclusive
- `cursor` and `limit` implement position-based pagination

These semantics need to remain backend-agnostic.

## Adding a new storage backend

If you add another storage engine:

1. Implement the `StorageEngine` trait.
2. Wire it into backend selection in `build_storage`.
3. Preserve the empty-history contract where appropriate.
4. Make sure `list_runs()` supports run resolution and UI enumeration.
5. Return errors instead of panicking: the persistence loop reports each batch's outcome back to the uploading client, which retries failures.
6. Batches arrive already validated against `schema.rs`, so positional, typed column access is safe.

## Adding a new telemetry type

A new telemetry type usually requires more than a storage change.

Checklist:

1. Add or extend the Arrow schema in the Python client.
2. Teach ingestion to classify the new batch kind.
3. Decide whether the storage engine persists it, streams it, or both.
4. Extend query APIs if the frontend needs historical access.
5. Add examples and tests.

If you skip the query or storage contract, the UI may appear partially broken even when ingestion itself succeeds.