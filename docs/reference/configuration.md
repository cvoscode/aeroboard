# Configuration Reference

Aeroboard currently relies primarily on environment variables passed into the Rust server process.

## Server environment variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `AEROBOARD_HTTP_PORT` | `3000` | HTTP API, dashboard, and WebSocket port |
| `AEROBOARD_FLIGHT_PORT` | `50051` | Arrow Flight ingestion port |
| `AEROBOARD_STORAGE` | `filesystem` | Storage mode: `filesystem`, `greptime`, `live` |
| `AEROBOARD_DATA_DIR` | `.aeroboard` | Filesystem storage root |
| `AEROBOARD_GREPTIME_URL` | `http://localhost:4000` | GreptimeDB base URL |
| `AEROBOARD_WEB_DIST_DIR` | `../aeroboard-web/dist` | Directory containing the built frontend bundle |

## Client parameters

The Python client exposes these runtime knobs in code rather than through process-wide environment variables:

| Parameter | Default | Meaning |
| --- | --- | --- |
| `run_id` | generated if blank | Base run name before auto-versioning |
| `url` | `grpc://localhost:50051` | Flight endpoint |
| `api_base_url` | derived from `url` or `http://localhost:3000` | HTTP API base |
| `batch_size` | `100` | In-memory flush threshold |

## CLI to environment mapping

`aeroboard start` maps CLI options to server environment variables before launching the Rust binary.

Examples:

- `--port` -> `AEROBOARD_HTTP_PORT`
- `--flight-port` -> `AEROBOARD_FLIGHT_PORT`
- `--storage` -> `AEROBOARD_STORAGE`
- `--data-dir` -> `AEROBOARD_DATA_DIR`

## Storage mode summary

### `filesystem`

Best default for local development, examples, and CI-like usage.

### `greptime`

Use when validating the Greptime-specific persistence path.

### `live`

Use when you only need the live stream and do not want persisted history.