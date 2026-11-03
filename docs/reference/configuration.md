# Configuration Reference

Aeroboard currently relies primarily on environment variables passed into the Rust server process.

## Server environment variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `AEROBOARD_HOST` | `127.0.0.1` | Interface both servers listen on. Set `0.0.0.0` to accept telemetry and dashboard traffic from other machines; there is no authentication, so only do this on a trusted network |
| `AEROBOARD_HTTP_PORT` | `3000` | HTTP API, dashboard, and WebSocket port |
| `AEROBOARD_FLIGHT_PORT` | `50051` | Arrow Flight ingestion port |
| `AEROBOARD_STORAGE` | `filesystem` | Storage mode: `filesystem`, `greptime`, `live` |
| `AEROBOARD_DATA_DIR` | `.aeroboard` | Filesystem storage root |
| `AEROBOARD_GREPTIME_URL` | `http://localhost:4000` | GreptimeDB base URL |
| `AEROBOARD_WEB_DIST_DIR` | `../aeroboard-web/dist` | Directory containing the built frontend bundle |
| `AEROBOARD_DASHBOARD_CONFIG` | optional `aeroboard.toml` | TOML file containing the initial dashboard view; see path resolution below |
| `AEROBOARD_CORS_ORIGINS` | unset | Comma-separated browser origins allowed to call the API cross-origin. Unset means same-origin only; `*` allows any website (including deleting runs) |
| `AEROBOARD_MAX_MESSAGE_MB` | `64` | Largest accepted Flight message |
| `AEROBOARD_PERSIST_QUEUE` | `256` | Batches buffered between ingestion and storage. When full, uploads wait (backpressure) instead of data being dropped |
| `AEROBOARD_LIVE_QUEUE` | `256` | Batches buffered per dashboard connection before it is told it lagged and re-fetches history |
| `AEROBOARD_STALE_AFTER_MS` | `60000` | Running-session freshness threshold, 1000–86400000 ms; heartbeat interval is 15 seconds |

## Client parameters

The Python client exposes these runtime knobs in code rather than through process-wide environment variables:

| Parameter | Default | Meaning |
| --- | --- | --- |
| `run_id` | generated if blank | Base run name before auto-versioning |
| `url` | `grpc://localhost:50051` | Flight endpoint |
| `api_base_url` | derived from `url` or `http://localhost:3000` | HTTP API base |
| `batch_size` | `100` | In-memory flush threshold |
| `background` | `True` | Send batches from a background thread so logging never waits on the network |
| `flush_timeout` | `10.0` | Seconds `flush()`/`close()` wait for delivery |
| `max_queue_bytes` | 256 MB | Memory budget for unsent batches while the server is unreachable; the oldest are dropped beyond it |
| `dashboard` (`aeroboard.init` only) | unset | Per-run initial view: `starred`, `order`, and `targets` |
| `project` | `default` | Project ID; created on a capable server when missing |
| `resume` | `False` | Continue the stable run UID supplied as `run_id` in a new session |
| `segment` | unset | Explicit step sequence name when restarting step numbering |
| `tags`, `metadata` | empty | Run tags and explicit provenance fields |
| `capture_provenance` | `False` | Capture Git revision/dirty flag and installed framework versions |
| `spool_dir` | unset | Root for per-session durable outboxes; enables offline capture |
| `max_spool_bytes` | 1 GiB | Unacknowledged payload quota per session, plus SQLite overhead |

## Initial dashboard view

Configure starred charts, their initial order, and target lines with a TOML file or from the training script.

### Server-wide defaults

Create `aeroboard.toml` in the directory where you invoke `aeroboard start`, before starting the server:

```toml
[dashboard]
starred = ["loss", "val/accuracy"]
order = ["loss", "accuracy", "lr"]

[dashboard.targets]
loss = 0.05
"val/accuracy" = 0.95
```

```bash
uv run aeroboard start
```

Starred charts start expanded and pinned above other charts. `order` arranges charts within the starred and unstarred groups; unlisted charts follow alphabetically. Targets are dashed horizontal reference lines, and the y-axis expands to include them.

To use another file:

```bash
AEROBOARD_DASHBOARD_CONFIG=config/dashboard.toml uv run aeroboard start
```

Path resolution:

- The CLI uses `aeroboard.toml` from the directory where you invoke `aeroboard start`, even when the server binary is stored elsewhere.
- A nonblank `AEROBOARD_DASHBOARD_CONFIG` takes priority. Relative paths resolve from the CLI's invocation directory; absolute paths are passed through.
- Without an explicit path or a default file, no configuration is required. If you create the default file after starting the CLI, restart to discover it.
- When running the Rust binary directly, both the default file and relative explicit paths resolve from the server's working directory.

The server re-reads the selected file on each `GET /api/config`, which the dashboard requests on page load. Edit the file and reload the page to apply changes. A missing selected file, invalid TOML, unknown dashboard option, or non-finite target produces a dashboard banner and empty server defaults.

### Per-run settings

```python
import aeroboard_client as aeroboard

aeroboard.init(
    run_id="resnet50",
    dashboard={
        "starred": ["loss", "accuracy"],
        "order": ["loss", "accuracy", "lr"],
        "targets": {"val/loss": 0.1, "accuracy": 0.95},
    },
)
```

You can also call `aeroboard.configure_dashboard(...)` or `client.configure_dashboard(...)` later. Each call replaces the previous per-run settings. Omitted fields inherit server defaults; an empty `starred` or `order` list clears that configured list for the run. Settings are flushed immediately.

### Precedence and metric names

The dashboard applies these layers, from highest to lowest priority:

1. Saved browser changes made in the UI.
2. Settings from the selected run's training script.
3. Server-wide TOML defaults.

Metric names can be full (`val/loss`) or chart names (`loss`). In single-run mode, `train/loss` and `val/loss` share the `loss` chart. In compare mode, they have separate charts, and `loss` matches both.

Targets match chart names separately within each layer before precedence is applied. For example, per-run `val/loss=0.1` wins over server `loss=0.05` on the grouped `loss` chart and the comparison `val/loss` chart. The comparison `train/loss` chart still inherits `0.05`. Within the same layer, an exact chart-name match wins over an alias; if several aliases match, the first matching entry supplies the target.

In compare mode, selected runs' `starred` and `order` lists are combined without duplicates. The first selected run with a matching target supplies it for that chart; other selected runs and then the server fill in missing targets.

Use **Reset chart layout** in the sidebar to clear browser overrides for stars, collapsed state, chart order, and targets. Removing a target in the UI keeps it hidden until you set it again or reset the layout.

Per-run settings use the reserved hyperparameter key `__aeroboard_dashboard__`. The dashboard and unfiltered `query_hyperparameters()` results hide this entry. Persistent backends retain it across reloads; live-only mode delivers it only to browsers connected when it is logged.

## CLI to environment mapping

`aeroboard start` maps CLI options to server environment variables before launching the Rust binary.

Examples:

- `--host` -> `AEROBOARD_HOST`
- `--cors-origin` (repeatable) -> `AEROBOARD_CORS_ORIGINS`
- `--port` -> `AEROBOARD_HTTP_PORT`
- `--flight-port` -> `AEROBOARD_FLIGHT_PORT`
- `--storage` -> `AEROBOARD_STORAGE`
- `--data-dir` -> `AEROBOARD_DATA_DIR`

## Storage mode summary

### `filesystem`

Best default for local development, examples, and CI-like usage.

### `greptime`

Use when you want database-backed persistence. Aeroboard creates its tables on first write with `run_id` and the metric name as the primary key and `append_mode` enabled. Tables created by Aeroboard < 0.2 stored `run_id` as a field and are incompatible: use **Delete All Runs** (which drops the tables) before logging with the new version. Non-finite values (NaN/inf) cannot be stored in GreptimeDB and are skipped.

### `live`

Use when you only need the live stream and do not want persisted history.
