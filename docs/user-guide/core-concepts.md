# Core Concepts

This page explains the concepts that shape how Aeroboard behaves at runtime.

## Runs

A run is the primary unit of organization in Aeroboard. Every metric point, hyperparameter entry, and image is associated with a `run_id`.

Examples:

- `baseline`
- `baseline-v2`
- `lightning-eval-filesystem-baseline`
- `experiment-pl2.5.1`

### Auto-versioning

The Python client does not blindly reuse a run name. When a client first needs its canonical run identifier, it asks the backend whether the requested base name already exists.

If the name is already taken, Aeroboard chooses the next free version suffix:

- `experiment` -> `experiment-v2`
- `experiment-v2` and `experiment-v4` already exist -> next becomes `experiment-v5`

Framework suffixes such as `-pl2.5.1` are preserved when versioning Lightning-oriented names.

## Telemetry types

Aeroboard currently supports three public telemetry categories from the Python SDK.

### Scalars

Scalars are numeric values such as losses, accuracies, learning rates, epochs, and steps.

The scalar Arrow schema is:

| Field | Type |
| --- | --- |
| `timestamp` | `timestamp[ms]` |
| `run_id` | `string` |
| `name` | `string` |
| `value` | `float64` |

### Hyperparameters

Hyperparameters are stored as stringified values plus a declared value type.

The hyperparameter Arrow schema is:

| Field | Type |
| --- | --- |
| `timestamp` | `timestamp[ms]` |
| `run_id` | `string` |
| `key` | `string` |
| `value` | `string` |
| `value_type` | `string` |

### Images

Images are stored as encoded bytes and accompanied by a declared format.

The image Arrow schema is:

| Field | Type |
| --- | --- |
| `timestamp` | `timestamp[ms]` |
| `run_id` | `string` |
| `name` | `string` |
| `image_data` | `binary` |
| `format` | `string` |

## Metric naming

Metric names are free-form strings, but some conventions matter:

- Metrics named `step` are treated specially by the frontend for alignment.
- Names ending in `/step` should also be treated as x-axis index metrics.
- Prefixing metrics with namespaces such as `train/`, `val/`, or `optimizer/` makes the dashboard easier to read.

The convenience API `log_metrics(..., prefix="train")` automatically prefixes bare names like `loss` to `train/loss`, while leaving already-qualified names such as `optimizer/lr` unchanged.

## Batching and flushing

The Python client buffers telemetry in memory before sending Arrow batches through Flight.

- Scalars flush when the scalar buffer reaches `batch_size`.
- Hyperparameters flush when their buffer reaches `batch_size`.
- Images flush more aggressively, at roughly one quarter of the configured batch size.

Call `flush()` explicitly at the end of a training run so partially filled buffers are sent.

## Storage modes

Aeroboard supports three storage modes.

### Filesystem

The server persists batches under a local data directory. This is the current default and is the simplest option for local use.

### GreptimeDB

The server persists history to GreptimeDB and queries it back through HTTP APIs.

### Live-only

The server keeps the live stream active but intentionally skips persistence. History queries return empty results rather than failing.

## Live stream versus history

Aeroboard uses two complementary paths:

- A live path for low-latency dashboard updates over WebSocket.
- A history path for persisted queries over HTTP.

This split is important operationally. A storage failure should not stop live streaming, and a live-only deployment should still behave predictably when the UI queries history.