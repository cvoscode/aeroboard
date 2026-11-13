# HTTP API Reference

Aeroboard serves an HTTP API from the same process that hosts the dashboard and WebSocket endpoint.

Base URL by default:

```text
http://localhost:3000
```

## `GET /api/config`

Reads the selected dashboard TOML file and returns server-wide initial-view defaults. The dashboard fetches this endpoint on page load, so file edits apply after reloading the page.

### Response shape

```json
{
  "dashboard": {
    "starred": ["loss", "val/accuracy"],
    "order": ["loss", "accuracy", "lr"],
    "targets": {"loss": 0.05, "val/accuracy": 0.95}
  }
}
```

When no file is configured or discovered, the endpoint returns empty lists and an empty target map. If the selected file cannot be read or parsed, it returns HTTP 200 with empty defaults and an `error` string, which the dashboard displays in a banner.

Per-run settings and saved browser overrides are applied by the frontend. See [configuration](configuration.md#initial-dashboard-view) for file selection and precedence.

## `GET /api/history`

Returns persisted scalar history plus aggregated run summaries.

### Query parameters

| Parameter | Type | Meaning |
| --- | --- | --- |
| `run_id` | string | Single run ID or comma-separated run IDs |
| `metric` | string | Single metric name or comma-separated metric names |
| `start_ms` | integer | Inclusive lower timestamp bound in milliseconds |
| `end_ms` | integer | Inclusive upper timestamp bound in milliseconds |
| `limit` | integer | Maximum number of rows to return |
| `cursor` | integer | Position-based pagination cursor |
| `max_points` | integer | Optional per-run/metric overview limit, from 4 to 1,000,000; omit for full resolution |
| `include_summaries` | boolean | Include `run_summaries` (default `true`) |
| `format` | string | `json` (default) or `arrow` |

### Arrow format

With `format=arrow` the response is an Arrow IPC stream
(`Content-Type: application/vnd.apache.arrow.stream`) with the columns
`timestamp` (ms), `run_id` and `name` (dictionary-encoded strings), `value` and the
nullable `step`. It is several times smaller than the JSON rows and is what the
dashboard uses. When paging, the next cursor is returned in the
`x-aeroboard-next-cursor` header. Run summaries are not included.

### Response shape

```json
{
  "rows": [
    {
      "ts_ms": 1740000000000,
      "run_id": "baseline",
      "name": "train/loss",
      "value": 0.42,
      "step": 100
    }
  ],
  "run_summaries": [
    {
      "run_id": "baseline",
      "hyperparameters": [
        {"key": "lr", "value": "0.001", "value_type": "float"}
      ],
      "metric_summaries": [
        {"name": "train/loss", "last": 0.42, "min": 0.42, "max": 1.1}
      ]
    }
  ],
  "next_cursor": null,
  "error": null
}
```

### Notes

- filtering happens with OR semantics within comma-separated `run_id` or `metric` sets
- time bounds are inclusive
- pagination is position-based rather than opaque-token based
- `step` is omitted for points logged without a step
- `run_summaries` describe whole runs (last/min/max over all points), not just the returned page
- if storage fails, the endpoint returns HTTP 500 with explicit empty arrays and an `error` string (all history endpoints behave this way)

## `DELETE /api/history`

Clears persisted history through the configured storage backend.

### Response shape

```json
{
  "deleted": true,
  "error": null
}
```

In live-only mode, deletion should not be treated as a normal persisted-history operation.

## `GET /api/history/images`

Returns persisted image history metadata.

### Query parameters

Uses the same filter fields as `GET /api/history`:

- `run_id`
- `metric`
- `start_ms`
- `end_ms`
- `limit`
- `cursor`
- `include_data` (optional, defaults to `false`)

### Response shape

```json
{
  "rows": [
    {
      "ts_ms": 1740000000000,
      "run_id": "baseline",
      "name": "samples/prediction",
      "format": "png"
    }
  ],
  "next_cursor": null,
  "error": null
}
```

When `include_data=true`, each row also includes `image_base64`. The default metadata-only mode keeps list responses small; load individual frames from `GET /api/history/media/content?kind=image&...` instead.

> The base64 frame endpoint `GET /api/history/images/frame` was removed in 0.2; use `/api/history/media/content` with `kind=image`.

## `GET /api/history/media`

Lists persisted media items (metadata only).

| Parameter | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `image`, `audio` or `video` (required) |
| `run_id`, `metric`, `start_ms`, `end_ms`, `limit`, `cursor` | | Same filters as `GET /api/history` |

The response has the same shape as `GET /api/history/images` without payloads.

## `GET /api/history/media/content`

Serves the raw bytes of one persisted media item with the matching `Content-Type`,
so it can be used directly as the `src` of an `<img>`, `<audio>` or `<video>` element.
Filesystem-backed items support HTTP `Range` requests for seeking.

| Parameter | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `image`, `audio` or `video` |
| `run_id` | string | Run ID |
| `name` | string | Stream name |
| `ts_ms` | integer | Exact item timestamp in milliseconds |

Returns `404` with a JSON `error` body when the item does not exist.

## `GET /api/runs`

Lists known run IDs.

### Response shape

```json
{
  "runs": ["baseline", "baseline-v2"]
}
```

## `GET /api/runs/summary`

Returns run summaries independently of paged scalar history.

### Query parameters

| Parameter | Type | Meaning |
| --- | --- | --- |
| `run_id` | string | Optional single run ID or comma-separated run IDs |

### Response shape

```json
{
  "run_summaries": [
    {
      "run_id": "baseline",
      "hyperparameters": [
        {"key": "lr", "value": "0.001", "value_type": "float"}
      ],
      "metric_summaries": [
        {"name": "train/loss", "last": 0.42, "min": 0.42, "max": 1.1}
      ]
    }
  ],
  "error": null
}
```

## `POST /api/runs/resolve`

Returns the canonical run ID that should be used for a proposed base run name.

### Request body

```json
{
  "base_run_id": "baseline"
}
```

### Response shape

```json
{
  "run_id": "baseline-v2"
}
```

### Versioning rules

- the returned ID is reserved immediately, so two processes resolving the same name at once get different IDs
- a brand new base name is returned unchanged
- if the base name already exists, the next highest version suffix is used
- Lightning framework suffixes (`-pl` followed by a dotted version, such as `-pl2.5.1`) are preserved while versioning; names that merely contain `-pl` (`sample-placement`) are versioned normally

## `GET /ws/live`

Upgrades to a WebSocket connection and streams committed Arrow IPC batches as binary messages.

Send `{"type":"subscribe","runs":["canonical-run-id"]}` after connecting and
when selections change. The server filters binary batches by run. Clients that
have not subscribed retain the legacy all-run stream. Persistent images, audio
and video carry metadata with empty payload columns; fetch their asset URLs.
`data_changed` messages identify runs with activity; entity `*_changed` messages
include `kind`, `id` and `revision`. Heartbeats do not broadcast catalog reloads.
A `lagged` message asks the client to resync history.

When persistence is enabled, audio and video payloads are stripped from live messages; clients load them from `/api/history/media/content`.

If a client falls too far behind, the server drops batches for it and sends a text message `{"type": "lagged", "skipped": <n>}`. Clients should re-fetch history for the runs they show; the dashboard does this automatically, and also after reconnecting.

## Workspace API

Workspace endpoints return JSON entities shaped as `{id, parent, revision, data}`. Lists of runs and events return `{rows, total, next_offset}`; other metadata list endpoints return arrays. Mutations use JSON. Validation errors return 400, missing records 404, stale revisions/conflicting identities 409, and deleted runs 410, with an `error` message.

| Endpoint | Behavior |
| --- | --- |
| `GET /api/capabilities` | Protocol/catalog version, features, persistent `server_id`, storage mode, `durable_replay`, message-size limit, stale interval |
| `GET /api/run-records` | Metadata listing; project, status, tag, search, archived, filters, offset, limit |
| `GET/PATCH/DELETE /api/run-records/{uid}` | Inspect metadata, edit display name/notes/tags/archive, or tombstone and remove one run |
| `POST /api/runs` | Idempotent registration with `run_uid`, requested `name`, `project_id`, tags, provenance |
| `POST /api/runs/{uid}/sessions` | Register a UUID `session_id`, optional resume flag and segment |
| `POST /api/runs/{uid}/sessions/{sid}/heartbeat` | Increasing control `sequence`; server receipt time supplies freshness |
| `POST /api/runs/{uid}/sessions/{sid}/finish` | Terminal status, control sequence, error, final telemetry sequence, dropped-row count, optional captured time |
| `GET/POST /api/projects` | List/create projects |
| `GET/PATCH/DELETE /api/projects/{id}` | Read/edit/remove an empty non-default project |
| `GET/POST /api/projects/{id}/views` | List/create project views |
| `GET/PATCH/DELETE /api/views/{id}` | Read/save/delete a versioned view |
| `GET/POST /api/metric-definitions` | List/upsert definitions; optional list `parent` selects project or run |
| `GET/POST /api/events` | Paginated newest-first listing or idempotent event creation |
| `PATCH/DELETE /api/events/{id}` | Edit annotation label/details or delete with revision checking |
| `GET /api/assets/{id}` | Stable media descriptor |
| `GET /api/assets/{id}/content` | Lazy media bytes with Range support |
| `GET /api/scorecards` | Finite objective summaries; required comma-separated run names/UIDs, optional metric, step, segment |
| `POST /api/comparison/aggregate` | Exact-position mean, sample standard deviation, contributor counts, population information |
| `GET /api/export/scalars.csv` | Stream raw selected run data as a CSV attachment |

Most mutable entities require `revision` in PATCH/upsert bodies; DELETE takes `?revision=N`. Run metadata PATCH/DELETE specifically uses `data.metadata_revision`, which is unaffected by heartbeat traffic. View bodies contain a name and `settings` object, up to 256 KiB. Project `default_view` must reference a view in the same project.

Run filters are a JSON array in the `filters` query parameter, combined with AND:

```json
[
  {"field": "params.learning_rate", "op": "lte", "value": 0.001},
  {"field": "metrics.val/loss.best", "op": "lt", "value": 0.1},
  {"field": "provenance.dataset", "op": "eq", "value": "digits-v2"}
]
```

At most 20 filters are accepted. Operators are `eq`, `ne`, `lt`, `lte`, `gt`, `gte`, `contains`, `exists`, and `missing`. Metric reducers are `last`, `min`, `max`, and goal-aware `best`. Numeric equality treats `0` and `0.0` equally. Missing values do not satisfy ordinary comparisons.

Scorecard and aggregation requests accept at most 50 runs. Analysis rejects selections above two million observations. Aggregation accepts `run_ids`, full `metric`, optional `group_by` (up to eight fields), `alignment` (`step`, `wall`, `rel`), `include_unfinished` (default false), `max_points` (clamped to 2–5000), and optional `segment`. Default groups preserve configuration except seed; dataset, full definition, and segment compatibility are always retained. At-step queries spanning multiple segments report missing until a segment is selected; `__global__` selects unnamed steps.

Event bodies include `id`, `run_uid`, optional `session_id`, `timestamp_ms`, optional `step`, kind, label, details object, and media ID array. IDs remain idempotent across annotation edits. SDK historical registration uses its original `captured_at_ms` and a historical flag; historical recovery never refreshes heartbeat freshness.

The live socket also sends `catalog_changed` control messages. Consumers should refresh workspace data after these, reconnects, and lag notifications, and discard repeated versioned Arrow batch IDs.

## Access control

The server has no authentication. By default it listens on `127.0.0.1` only and sends no CORS headers, so only the dashboard it serves (or the Vite dev proxy) can call the API. See `AEROBOARD_HOST` and `AEROBOARD_CORS_ORIGINS` in the [configuration reference](configuration.md).
