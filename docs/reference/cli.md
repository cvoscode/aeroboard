# CLI Reference

Aeroboard exposes a `click`-based CLI through the `aeroboard` command.

## `aeroboard setup`

Prepares the frontend and Rust server runtime assets.

Behavior:

- downloads matching release artifacts by default
- with `--build-from-source`, installs frontend dependencies and builds the frontend and Rust server locally
- with `--force-download`, refreshes cached release artifacts

Typical usage:

```bash
uv run aeroboard setup
```

## `aeroboard start`

Prepares the runtime assets and starts the server.

```bash
uv run aeroboard start [OPTIONS]
```

Options:

| Option | Default | Description |
| --- | --- | --- |
| `--host` | `127.0.0.1` | Interface to listen on; `0.0.0.0` exposes the dashboard and ingestion to your network (no authentication) |
| `--cors-origin` | unset | Extra browser origin allowed to call the API; repeatable, `*` allows any site |
| `--port` | `3000` | HTTP port for the web UI, HTTP API, and WebSocket endpoint |
| `--flight-port` | `50051` | Arrow Flight gRPC ingestion port |
| `--storage` | `filesystem` | Storage backend: `filesystem`, `greptime`, or `live` |
| `--data-dir` | unset | Filesystem storage root when `storage=filesystem` |
| `--build-from-source` | off | Build the frontend and server locally from a source checkout |
| `--force-download` | off | Refresh downloaded runtime artifacts |

Notes:

- storage mode is passed to the server through environment variables
- `aeroboard.toml` is discovered in the directory where you invoke the command, before launching the server from its binary directory
- `AEROBOARD_DASHBOARD_CONFIG` selects another dashboard configuration file; relative paths resolve from the directory where you invoke the command

Example with shared initial chart settings:

```bash
# Create aeroboard.toml in this directory, then start:
uv run aeroboard start
# Or select a different file:
AEROBOARD_DASHBOARD_CONFIG=config/dashboard.toml uv run aeroboard start
```

See the [configuration reference](configuration.md#initial-dashboard-view) for stars, chart order, target lines, and overrides.

## `aeroboard db`

Starts GreptimeDB through Docker Compose.

```bash
uv run aeroboard db
```

Option:

| Option | Default | Description |
| --- | --- | --- |
| `--greptime-image` | `greptime/greptimedb:latest` | Docker image to use |

## `aeroboard stop-db`

Stops the Docker Compose-managed GreptimeDB service.

```bash
uv run aeroboard stop-db
```

## `aeroboard status`

Checks local Aeroboard services.

```bash
uv run aeroboard status
```

It attempts to inspect:

- `http://localhost:3000/api/runs`
- the Flight endpoint on `grpc://localhost:50051`
- GreptimeDB health on `http://localhost:4000/health`

## `aeroboard check-run`

Fetches and prints recent metrics for a run.

```bash
uv run aeroboard check-run [OPTIONS] [RUN_ID]
```

Options:

| Option | Description |
| --- | --- |
| `--no-build` | Skip rebuilding before the check |

If `RUN_ID` is omitted, the command first asks the server for `/api/runs` and uses the first available run.

## `aeroboard log-run`

Starts an auxiliary run-logging workflow. This command exists in the CLI surface but should be treated as a utility command rather than the primary instrumentation path.

For normal logging from training code, prefer the Python SDK directly.
