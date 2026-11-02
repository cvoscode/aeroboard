# Python SDK Reference

This page documents the public Python client API exposed by `aeroboard_client`.

## Module-level helpers

Import style:

```python
import aeroboard_client as aeroboard
```

Available helpers:

| Function | Purpose |
| --- | --- |
| `init(...)` | Create and register the default client |
| `get_default_client()` | Return the current default client or `None` |
| `log(metrics)` | Log scalar metrics through the default client |
| `log_metrics(metrics, step=None, prefix=None)` | Log metrics with optional prefixing and shared step |
| `log_hyperparameters(hyperparameters, step=None)` | Log hyperparameters |
| `log_image(name, image, format=None, timestamp_ms=None)` | Log an image |
| `configure_dashboard(starred=None, targets=None, order=None)` | Set the run's initial chart stars, targets, and order; arguments are keyword-only |
| `flush()` | Flush the default client |

### `init(..., dashboard=None)`

The module-level initializer accepts an optional dashboard configuration:

```python
aeroboard.init(
    run_id="baseline",
    dashboard={
        "starred": ["loss", "accuracy"],
        "order": ["loss", "accuracy"],
        "targets": {"val/loss": 0.1},
    },
)
```

Unknown dashboard options and invalid target values are rejected before connecting. Use `client.configure_dashboard(...)` when constructing an `AeroboardClient` directly.

## `AeroboardClient`

Constructor:

```python
AeroboardClient(
    run_id=None,
    url="grpc://localhost:50051",
    batch_size=100,
    api_base_url=None,
)
```

### Core attributes

| Attribute | Meaning |
| --- | --- |
| `base_run_id` | The requested run name before resolution |
| `run_id` | The resolved canonical run name |
| `url` | Flight endpoint |
| `api_base_url` | HTTP API base for run resolution and history queries |
| `batch_size` | Client-side buffer threshold |

### Logging methods

#### `log(metrics: dict[str, float])`

Logs raw scalar metrics.

#### `log_metrics(metrics, step=None, prefix=None)`

Convenience method for training loops.

Rules:

- non-numeric values are skipped
- scalar tensors are coerced when possible
- `prefix` is only applied to names without `/`
- `step` is logged as the scalar metric `step`

#### `log_hyperparameters(hyperparameters, step=None)`

Stores hyperparameters as string values plus a `value_type` classification.

#### `log_image(name, image, format=None, timestamp_ms=None)`

Stores image payloads for timeline-style inspection.

The client accepts multiple image input forms and normalizes them to encoded bytes before upload.

### `configure_dashboard(*, starred=None, targets=None, order=None)`

Sets the run's initial dashboard view and flushes the settings immediately. The module-level helper has the same keyword-only arguments.

| Argument | Meaning |
| --- | --- |
| `starred` | List of chart or metric names to expand and pin to the top |
| `targets` | Mapping from chart or metric names to finite numeric target values |
| `order` | List of chart or metric names in the desired initial order |

```python
client.configure_dashboard(
    starred=["loss"],
    targets={"val/loss": 0.1},
    order=["loss", "accuracy"],
)
```

Each call replaces the previous per-run settings. Omitted fields inherit server defaults; empty `starred` and `order` lists override server lists with empty ones. Targets match full metric names or chart names before applying precedence, so per-run `val/loss=0.1` overrides server `loss=0.05` on the grouped `loss` chart. Saved browser changes take priority; **Reset chart layout** clears them.

See [initial dashboard configuration](configuration.md#initial-dashboard-view) for server files, comparison behavior, and metric matching.

### Flush and lifecycle methods

#### `flush()`

Flushes scalar, hyperparameter, and image buffers to the Flight server.

#### `close()`

Flushes remaining buffers and closes the underlying Flight client.

### Query methods

#### `query_history(...)`

Returns a dictionary with:

- `rows`
- `run_summaries`
- `next_cursor`

Accepted filters:

- `run_id`
- `metric`
- `from_timestamp`
- `to_timestamp`
- `limit`
- `cursor`

#### `query_hyperparameters(...)`

Returns a flattened list of hyperparameter entries extracted from history summaries.

The reserved dashboard entry `__aeroboard_dashboard__` is hidden unless explicitly requested with `key="__aeroboard_dashboard__"`.

#### `query_images(...)`

Returns image history rows from `/api/history/images` and can optionally filter by image format client-side.

#### `list_runs()`

Returns the server-reported run identifiers.

## Exceptions

| Exception | Meaning |
| --- | --- |
| `AeroboardClientError` | Base client error for query or client-side failures |
| `FlightConnectionError` | Raised when ingestion is attempted without a working Flight connection |

## Lightning integration

### `lightning_logger(...)`

Preferred helper for Lightning projects. Returns an `AeroboardLightningLogger` suitable for `Trainer(logger=...)`.

### `attach_lightning(...)`

Compatibility alias that returns the same logger type. It is deprecated in favor of `lightning_logger(...)`.

### `AeroboardLightningCallback`

Alternative callback-based integration for Lightning.

## Ray Tune integration

### `AeroboardRayTuneLoggerCallback`

Creates one Aeroboard run per Ray trial, logs trial configuration as hyperparameters, and forwards numeric result values as scalar metrics.

## Arrow schemas

The current public schemas are defined in `aeroboard_client.schema`.

### Scalars

`timestamp`, `run_id`, `name`, `value`

### Hyperparameters

`timestamp`, `run_id`, `key`, `value`, `value_type`

### Images

`timestamp`, `run_id`, `name`, `image_data`, `format`
