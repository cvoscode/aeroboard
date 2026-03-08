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

Conceptually the layout looks like this:

```text
.aeroboard/
  runs/
    <encoded-run-id>/
      manifest.json
      scalars/
        <timestamp>-<uuid>.arrow
      hyperparameters/
        <timestamp>-<uuid>.arrow
      images/
        <encoded-metric-name>/
          index.json
          <timestamp>-<uuid>.<format>
```

Important details:

- a manifest is written per run so the run can be enumerated later
- scalar and hyperparameter data are stored as Arrow files
- images are extracted and stored as individual files plus an index

Filesystem mode is usually the best place to validate new telemetry behavior because it keeps the persistence path transparent.

## GreptimeDB

Greptime mode routes persistence and historical queries through the configured Greptime HTTP endpoint.

Use it when you specifically want database-backed persistence or need to validate the Greptime-specific code path.

## Live-only

Live-only mode intentionally accepts batches without persisting them.

Expected semantics:

- ingestion succeeds
- live streaming still works
- history queries return empty collections
- delete-style persistence operations should not pretend they succeeded against nonexistent stored data

## Query semantics shared across backends

The HTTP history layer applies consistent filters after retrieving rows from storage:

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
5. Verify that failures in persistence do not break the live path.

## Adding a new telemetry type

A new telemetry type usually requires more than a storage change.

Checklist:

1. Add or extend the Arrow schema in the Python client.
2. Teach ingestion to classify the new batch kind.
3. Decide whether the storage engine persists it, streams it, or both.
4. Extend query APIs if the frontend needs historical access.
5. Add examples and tests.

If you skip the query or storage contract, the UI may appear partially broken even when ingestion itself succeeds.