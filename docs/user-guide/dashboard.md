# Using the Dashboard

The dashboard is the browser-facing part of Aeroboard. It combines persisted history with the live telemetry stream so runs appear immediately while still remaining queryable after reload.

See [projects, objectives, and recovery](workspaces.md) for project organization, lifecycle badges, saved views, goal-aware scorecards, baseline and multi-seed comparison, linked annotations/media, exports, and offline delivery.

## How the dashboard gets data

On startup, the frontend does two things:

1. Fetches persisted history from the HTTP API.
2. Opens a WebSocket to `/ws/live` for new batches.

This split lets the UI recover existing data after a refresh and then continue updating in real time.

## What the dashboard shows

The current UI is oriented around experiment inspection and comparison.

The top bar contains project and saved-view selection, followed by the page tabs. The left panel lists your runs, newest first, grouped by experiment. Hover a row for its full name, or delete a run or a whole group with its trash icon. **Delete all runs** removes every run on the server, in all projects. Chart settings sit above the charts. On smaller screens, open the run list with **Runs**.

Use the metrics search to narrow the chart list. **Show hidden charts** opens a list where you pick which charts to bring back; a chart marked *Not in selected runs* reappears once a selected run reports it. **Metric objectives** is a slim bar that stays closed until you open it, and it remembers your choice; it reports how many targets were reached, and each tile shows a metric's best value. **Compact**, **Wide**, and **Full** adjust chart width to the available space. Cards in a row share the same height, every expanded chart is the same size, and long titles wrap at `/`, `.`, and `_`. Target values appear on their plot lines; the crosshair button opens the target editor.

In persistent storage modes, the trash button on a run card deletes that run and its stored data after confirmation. The trash button on a group header deletes every version in that group, including versions hidden by the run filter. Other groups are kept.

### Scalar charts

Scalar metrics are decoded from Arrow IPC frames and stored in typed arrays before being rendered with `uPlot`.

Hover over a chart to see the nearest step or timestamp and each series' value at that position. With smoothing enabled, the tooltip shows both raw and smoothed values. A dash means the series has no sample at that position.

Each chart's scale selector offers **Linear**, **Log**, and **Exponential**. Exponential gives small differences near the high end of the displayed range more space. It supports zero and negative values, and the axis labels, hover values, and target lines remain in the original units. Scale choices are saved per metric and shared between single-run and compare mode.

Drag across a chart to zoom in, and double-click to reset. The y-axis refits to what is on screen, for drag zooms and for the **Range** slider alike, so a target line outside the zoomed view moves off the chart until you zoom back out.

Use the trend button beside the scale selector to add a dashed linear trend for each trace. Trends fit raw samples in the visible window and update when you zoom, adjust the window, or receive live data. Hover values include the fitted values. At least two distinct x positions are needed. Trend choices are saved per metric and shared between single-run and compare mode. The fit uses original units, so nonlinear axes can make it appear curved.

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

### Initial view and target lines

To start with your most important charts expanded and pinned to the top, configure the training run:

```python
import aeroboard_client as aeroboard

aeroboard.init(
    run_id="baseline",
    dashboard={
        "starred": ["loss", "accuracy"],
        "order": ["loss", "accuracy", "lr"],
        "targets": {"val/loss": 0.1, "accuracy": 0.95},
    },
)
```

For defaults shared by all runs, create `aeroboard.toml` in the directory where you run `aeroboard start`. The [configuration reference](../reference/configuration.md#initial-dashboard-view) covers the file format and custom paths.

Use each chart's star button to pin it, drag a chart by its header to reorder it, or click the crosshair to add or edit a target. A dragged chart stays where you drop it, even among pinned charts. Targets appear as dashed horizontal lines, and the y-axis grows to keep them visible.

Choose **Metric groups** to create named groups such as **Losses** and **Metrics**, then select the charts to include. Click a group's heading to hide or show all its charts. Drag the group's grip, or use its up/down buttons, to move the whole group. Charts can still be reordered within each group. Group membership, visibility, and order are saved in your browser and included in saved views, and apply in both single-run and compare mode. Searching also matches group names. To change membership or rename a group, select it in **Metric groups**; **Ungroup charts** removes the group and returns its charts to the ungrouped section. Individually hidden charts stay hidden when you show their group again.

Your browser saves chart changes and gives them priority over the training script and server defaults. Stars, hidden charts, collapsed charts, order, targets, and the y-axis scale belong to the metric, so they apply in both single-run and compare mode: changing `val/loss` also changes `train/loss`, and both follow the `loss` chart. Choose **Reset chart layout** above the charts to apply the configured initial view again, including any target you removed manually.

Full metric names such as `val/loss` also match the grouped `loss` chart in single-run mode. Per-run targets override server targets even when one uses a full name and the other uses a chart name. When comparing runs, the first selected run with a matching target supplies that chart's line.

### Comparison views

Hyperparameters are especially useful in views that compare multiple runs side by side. If you want those views to be meaningful, log hyperparameters early in the run.

In **Hyperparameters**, the comparison table uses AG Grid Community, with virtual scrolling for rows and columns and pinned run names. Click a header to sort, Shift-click to sort by several columns, drag headers to reorder them, or drag their edges to resize. Each column has a text or numeric filter; the search field searches across run names, statuses, hyperparameters, and metrics. **Clear filters** resets both kinds of filter. CSV exports follow the grid's current filters, sort order, and column order, and retain the original hyperparameter values.

Long configuration values stay compact; click one to inspect its full contents in a dialog. Choose a baseline to see configuration differences grouped by run, with nested fields such as `encoder.dropout` compared separately. JSON and Python dictionary values are supported.

Parallel coordinates keep numeric axes spaced apart and scroll horizontally when you select many fields. Missing values leave gaps instead of removing a run; the legend reports how many observations each run has. Constant axes place all observations at the center. Metric axes use the same **Last observation**, **Best (defined goal)**, or **At training step** reducer as the comparison table.

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
