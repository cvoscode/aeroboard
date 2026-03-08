# Repository Layout

This repository contains multiple language stacks in one workspace.

## Top-level directories

### `aeroboard-client/`

Python client package that exposes the telemetry SDK, CLI entrypoints, and integration helpers.

Important paths:

- `src/aeroboard_client/client.py`: client implementation and framework integrations
- `src/aeroboard_client/cli.py`: top-level `aeroboard` CLI
- `src/aeroboard_client/schema.py`: Arrow schemas for supported telemetry types

### `aeroboard-server/`

Rust application that handles Flight ingestion, WebSocket live streaming, storage selection, and history APIs.

Important paths:

- `src/main.rs`: application bootstrap and routing
- `src/config.rs`: runtime configuration from environment
- `src/history.rs`: HTTP handlers and history query shaping
- `src/storage.rs`: storage abstraction and backend implementations
- `src/t_junction.rs`: broadcast fan-out hub

### `aeroboard-web/`

React and Vite application for the dashboard.

Important paths:

- `src/hooks/useAeroboard.ts`: main data ingestion and store logic
- `src/components/`: UI widgets and plots

### `examples/`

Runnable example scripts for plain loops, Lightning, and Ray Tune.

### `tests/`

Python tests covering run versioning, payload generation, and client-level behavior.

### `docs/`

MkDocs source tree for this documentation site.

## Package layout notes

The repository has both a top-level Python project configuration and a dedicated `aeroboard-client` package directory. When changing Python packaging or documentation, verify which path is authoritative for the behavior you are describing.

## Build outputs and generated artifacts

Common generated output locations include:

- `aeroboard-server/target/` for Rust build artifacts
- `aeroboard-web/dist/` for the frontend bundle
- `.aeroboard/` or custom data directories for filesystem persistence
- `logs/` for captured example runs in this repository snapshot

Do not document generated artifacts as if they are source-owned unless the workflow explicitly depends on them.