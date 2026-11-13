# Storage model

`StorageEngine` supplies history, media, export and summary queries.
`DurableStore` handles identities and commit visibility; `PayloadBackend` owns the
physical payload layout. The catalog uses SQLite WAL, one writer, four readers,
and synchronous FULL commits. One server owns a data directory.

## Filesystem

New SDK uploads use this layout:

```text
.aeroboard/
  catalog.sqlite
  segments/
    <run-uid>/
      scalars/00000000.segment
      hyperparameters/00000000.segment
      images/00000000.segment
      audio/00000000.segment
      video/00000000.segment
```

A segment contains length-prefixed Arrow IPC records and rolls at 64 MiB.
`payload_locations` stores each batch's path, offset and length. A group shares
file fsyncs; directory entries are synced before the receipt commit. History
opens each segment once per query and reads referenced records.

Startup migrates committed flat `batches/<uuid>.arrow` files into segments.
Locations commit before originals are removed, so interruption is safe to retry.
Legacy envelope-less clients still use `runs/<encoded-name>/` with JSONL indexes
and media files. Earlier JSON-array/raw-Arrow directories remain readable.
These compatibility paths are intentionally retained and tested.

## GreptimeDB

New scalar and hyperparameter batches become native rows in
`aeroboard_scalars` and `aeroboard_hyperparameters`. Primary keys include batch ID
and row ordinal; retries overwrite the same rows and distinct same-millisecond
steps remain distinct. The numeric `value` column is externally queryable;
`value_bits` preserves non-finite values and signed zero. Native reads share SQL
requests. Table creation happens once per store instance; common HTTP code
handles retries with jitter.

Media bytes use local segments referenced by catalog assets. Back up this data
volume and catalog along with Greptime. Startup migrates the previous hex IPC
`aeroboard_batches` representation before removing old blobs. Older native tables
remain readable through the compatibility storage engine.

## Receipts and crash recovery

Catalog schema version 2 adds indexed session and telemetry-kind columns, plus
unique `(run_uid, session_id, sequence)` identities. An upload first reserves a
receipt. After payload durability, one transaction commits its location, assets,
run summaries, objective scores and visibility. `summary_batches` prevents a
replayed batch from updating counters twice. A repeated committed identity is
acknowledged without another live publication.

Persistence drains groups of at most 64 batches / 16 MiB, collecting for at most
2 ms. Multiple compatible batches can share a Flight RPC. SIGINT and SIGTERM
finish accepted work and drain the persistence queue.

Default summaries and scores survive restarts. A changed metric definition
requires one raw-history rebuild outside the persistence gate; optimistic run
revision checks prevent it replacing newer counters. Other analysis caches use
selected run data revisions and definition revisions. Deletion tombstones one
run, removes its payloads and invalidates its derived data. Tombstones reserve
names and reject delayed replay. Archive only changes metadata.

The SDK outbox separately journals IPC and control operations in SQLite. Its
writer notifies the sender; idle delivery does not poll. Acknowledged operations
are removed and permanently rejected operations are quarantined.

## Queries and live-only mode

`run_id` and `metric` accept comma-separated lists; timestamp bounds are inclusive.
`limit`/`cursor` retain position-based pagination. Identified scalar history stays
columnar through filtering, concatenation and sorting. Arrow responses stream in
bounded chunks. CSV reads one snapshot. `max_points` bounds each series for chart
overviews; raw export and zoom-window requests preserve full resolution.

Payload records and query snapshots each have a 128 MiB LRU budget. Query keys
include the selected runs' data revisions and deletion state, filters, and
overview budget. Pages reuse the same columnar snapshot without reopening
payloads; cursor and page size only slice it. A snapshot larger than the cache
budget is served without retention.

Live-only accepts and streams telemetry but stores no payloads or history and
advertises no durable replay. Its catalog is in memory and media bytes travel
with the live update.

When adding telemetry kinds, update the Python and Rust schemas, protocol vectors,
backend codecs, HTTP responses and frontend decoding together.
