# CLI Reference

Aeroboard exposes a `click`-based CLI through the `aeroboard` command.

## `aeroboard setup`

Builds the frontend and Rust server.

Behavior:

- installs frontend dependencies with `npm install`
- builds the frontend with `npm run build`
- builds the Rust server with `cargo build --release`

Typical usage:

```bash
uv run aeroboard setup
```

## `aeroboard start`

Builds the project and starts the server.

```bash
uv run aeroboard start [OPTIONS]
```

Options:

| Option | Default | Description |
| --- | --- | --- |
| `--port` | `3000` | HTTP port for the web UI, HTTP API, and WebSocket endpoint |
| `--flight-port` | `50051` | Arrow Flight gRPC ingestion port |
| `--storage` | `filesystem` | Storage backend: `filesystem`, `greptime`, or `live` |
| `--data-dir` | unset | Filesystem storage root when `storage=filesystem` |

Notes:

- the command tries to run the release binary after rebuilding it
- if the binary is missing, it falls back to `cargo run --release`
- storage mode is passed to the server through environment variables

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