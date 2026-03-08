# Installation

This page covers the local setup needed to run Aeroboard and build its documentation.

## Prerequisites

Aeroboard spans Python, Rust, and a React frontend, so the full development setup needs tools from each stack.

Required for the standard local workflow:

- `uv` for Python dependency management and command execution.
- Python 3.9 or newer.
- Node.js and `npm` for the dashboard build.
- Rust and `cargo` for the server build.

Optional depending on the storage mode you use:

- Docker and Docker Compose for GreptimeDB.

## Full project setup

The repository ships with a top-level setup script that installs Python dependencies through `uv`, builds the frontend bundle, and builds the Rust server.

```bash
chmod +x setup.sh
./setup.sh
```

That script performs these steps:

1. `uv sync --extra examples --dev`
2. `uv run aeroboard setup`
3. `npm install && npm run build` inside `aeroboard-web`
4. `cargo build --release` inside `aeroboard-server`

## Manual setup

If you prefer to run the steps yourself:

```bash
uv sync --extra examples --dev
uv run aeroboard setup
```

The CLI setup command will rebuild both the frontend and the Rust server.

## Running Aeroboard

Start the HTTP server, WebSocket endpoint, and Arrow Flight ingestion server:

```bash
uv run aeroboard start
```

Default ports:

- HTTP dashboard and API: `3000`
- Arrow Flight ingestion: `50051`

Useful variants:

```bash
uv run aeroboard start --storage filesystem --data-dir .aeroboard
uv run aeroboard start --storage greptime
uv run aeroboard start --storage live
uv run aeroboard start --port 3100 --flight-port 51051
```

## Running GreptimeDB

GreptimeDB is optional. Filesystem storage is the default and is usually the simplest local mode.

If you want database-backed persistence:

```bash
uv run aeroboard db
uv run aeroboard start --storage greptime
```

To stop the database:

```bash
uv run aeroboard stop-db
```

## Installing optional client integrations

Some integrations depend on optional extras:

- Ray Tune example: `uv sync --extra ray --dev`
- Lightning examples: included in the root project dependencies

## Verifying the installation

The CLI includes a status check:

```bash
uv run aeroboard status
```

That command checks:

- The HTTP API on port `3000`
- The Flight endpoint on port `50051`
- The GreptimeDB health endpoint on port `4000`

## Documentation dependencies

The docs site is built with MkDocs and is configured for Read the Docs hosting.

```bash
pip install -r docs/requirements.txt
mkdocs serve
```

If you already use `uv` for everything, you can also run MkDocs through the environment you manage yourself after installing the dependency there.