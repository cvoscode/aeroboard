# Using the Dashboard

The dashboard is the browser-facing part of Aeroboard. It combines persisted history with the live telemetry stream so runs appear immediately while still remaining queryable after reload.

## How the dashboard gets data

On startup, the frontend does two things:

1. Fetches persisted history from the HTTP API.
2. Opens a WebSocket to `/ws/live` for new batches.

This split lets the UI recover existing data after a refresh and then continue updating in real time.

## What the dashboard shows

The current UI is oriented around experiment inspection and comparison.

### Scalar charts

Scalar metrics are decoded from Arrow IPC frames and stored in typed arrays before being rendered with `uPlot`.

Practical implications:

- high-frequency metric streams remain cheap to render
- the UI benefits from metrics that include a shared `step`
- consistent metric names make comparison easier

### Run summaries

The history API returns `run_summaries` alongside raw rows. Those summaries include:

- hyperparameters per run
- per-metric last value
- per-metric minimum
- per-metric maximum

These summaries drive overview and comparison-oriented widgets.

### Comparison views

Hyperparameters are especially useful in views that compare multiple runs side by side. If you want those views to be meaningful, log hyperparameters early in the run.

### Image timeline

Images are shown as historized frames ordered by timestamp. This works best when you log the same image `name` repeatedly over time, for example:

- `samples/prediction`
- `diagnostics/gradient-norms`
- `validation/input-preview`

## Best practices for readable runs

- Use a clear run prefix such as `baseline`, `ablation-dropout`, or `ray-search`.
- Keep metric names stable across related runs.
- Always emit a `step` metric or use `log_metrics(..., step=...)`.
- Log hyperparameters before or early in training.
- Use consistent image names when logging timelines.

## Storage-mode implications

The dashboard works differently depending on the server mode.

### Filesystem and Greptime modes

Both support:

- persisted history reloads
- run listing
- comparison summaries
- image history queries

### Live-only mode

Live-only mode supports:

- live updates over WebSocket

It intentionally does not provide persisted history. The history endpoints return empty collections instead of historical data.

## Troubleshooting empty views

If the dashboard is empty:

1. Confirm the server is running.
2. Confirm your client is pointing at the correct Flight and HTTP ports.
3. Check whether you are in `live` mode and expecting persisted history.
4. Make sure your script calls `flush()` before exiting.
5. Use `uv run aeroboard status` to inspect what the local server can see.