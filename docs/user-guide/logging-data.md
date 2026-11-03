# Logging Data

This page focuses on the Python SDK that application code uses to emit telemetry and query history.

## Initialize a client

The public entrypoint is the module-level `init()` helper:

```python
import aeroboard_client as aeroboard

client = aeroboard.init(
    run_id="my-run",
    url="grpc://localhost:50051",
    api_base_url="http://localhost:3000",
    batch_size=100,
)
```

Important parameters:

- `run_id`: base run name before auto-versioning.
- `url`: Arrow Flight endpoint.
- `api_base_url`: HTTP base URL used for run resolution and history queries.
- `batch_size`: client-side telemetry buffer size.

The module also keeps a default client so helpers like `aeroboard.log(...)` can be used without passing the client object around.

## Log scalar metrics

Use `log()` when you already have the exact metric payload you want to send:

```python
client.log({"train/loss": 0.42, "train/accuracy": 0.91}, step=10)
```

Only numeric values are emitted. Non-numeric entries are ignored.

The step is stored with every value and is what the dashboard's step axis uses. A
`"step"` key inside the dict is treated the same as the `step` argument (it is not
logged as a separate metric).

## Delivery and failures

Logging calls only buffer data; a background thread sends it to the server, so
your training loop never waits on the network. `flush()` waits (up to
`flush_timeout`) until the server confirms everything is stored, and `close()` —
also run automatically at interpreter exit — does the same before disconnecting.

If the server is unreachable, batches are retried with backoff and `flush()`
returns immediately instead of stalling training. Unsent data is capped by
`max_queue_bytes`; beyond it the oldest batches are dropped and a warning is
printed. Batches the server rejects as malformed are dropped. `had_error()`
reports whether any of this happened.

## Log metrics with prefixing and shared step

Use `log_metrics()` for training-loop style code:

```python
client.log_metrics(
    {"loss": loss, "accuracy": accuracy, "optimizer/lr": current_lr},
    step=global_step,
    prefix="train",
)
```

Behavior:

- Bare names like `loss` become `train/loss` when `prefix="train"` is provided.
- Existing names with a slash are kept as-is.
- The provided `step` is added as a separate scalar metric named `step`.
- Scalar tensors are coerced to floats when possible.

## Log hyperparameters

Use `log_hyperparameters()` for values you want to compare across runs:

```python
client.log_hyperparameters({
    "optimizer": "adamw",
    "lr": 0.001,
    "dropout": 0.2,
    "batch_size": 64,
})
```

Supported value categories include common scalar types, booleans, strings, and list-like values that can be stringified meaningfully. `None` values are ignored.

The dashboard uses these entries in comparison-oriented views.

## Log images

Use `log_image()` for snapshots you want to inspect over time:

```python
client.log_image("samples/prediction", image_bytes, format="png")
```

Accepted input types include:

- Raw bytes
- PIL images
- NumPy arrays
- Torch tensors that can be converted into image data

The client may recompress images to keep payload sizes manageable. Large images are constrained by an internal maximum payload budget.

## Query history

The SDK also wraps the HTTP history API.

### Query scalar history

```python
response = client.query_history(
    run_id="my-run",
    metric="train/loss",
    from_timestamp=0,
    to_timestamp=9999999999,
    limit=200,
)

rows = response["rows"]
run_summaries = response["run_summaries"]
next_cursor = response["next_cursor"]
```

### Query hyperparameters

```python
entries = client.query_hyperparameters(run_id="my-run")
```

This currently works by reading the run summaries returned from the history endpoint.

### Query image history

```python
images = client.query_images(run_id="my-run", name="samples/prediction", limit=20)
```

Returned image rows include base64-encoded image data and a declared format.

### List runs

```python
runs = client.list_runs()
```

## End-of-run cleanup

Call `flush()` before your process exits:

```python
client.flush()
client.close()
```

`close()` also performs a final flush.

## Failure behavior

If the Flight client cannot connect, ingestion methods raise `FlightConnectionError`. Query helpers raise `AeroboardClientError` when HTTP requests fail.

This means instrumentation code should either ensure the server is already running or handle those exceptions explicitly when Aeroboard is optional in a workflow.