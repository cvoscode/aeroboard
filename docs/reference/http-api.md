# HTTP API Reference

Aeroboard serves an HTTP API from the same process that hosts the dashboard and WebSocket endpoint.

Base URL by default:

```text
http://localhost:3000
```

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

### Response shape

```json
{
  "rows": [
    {
      "ts_ms": 1740000000000,
      "run_id": "baseline",
      "name": "train/loss",
      "value": 0.42
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
- the endpoint returns explicit empty arrays and an optional error string if storage fails

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

Returns persisted image history.

### Query parameters

Uses the same filter fields as `GET /api/history`:

- `run_id`
- `metric`
- `start_ms`
- `end_ms`
- `limit`
- `cursor`

### Response shape

```json
{
  "rows": [
    {
      "ts_ms": 1740000000000,
      "run_id": "baseline",
      "name": "samples/prediction",
      "format": "png",
      "image_base64": "..."
    }
  ],
  "next_cursor": null,
  "error": null
}
```

## `GET /api/runs`

Lists known run IDs.

### Response shape

```json
{
  "runs": ["baseline", "baseline-v2"]
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

- a brand new base name is returned unchanged
- if the base name already exists, the next highest version suffix is used
- framework suffixes such as `-pl2.5.1` are preserved while versioning

## `GET /ws/live`

Upgrades to a WebSocket connection and streams live Arrow IPC batches.

The frontend uses this endpoint for real-time updates after loading persisted history.