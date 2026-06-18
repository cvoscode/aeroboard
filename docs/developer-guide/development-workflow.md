# Development Workflow

This page covers the day-to-day workflow for contributing to Aeroboard.

## Initial setup

The repository-standard bootstrap is:

```bash
./setup.sh
```

That script uses `uv` for Python environment management and then builds both the frontend and the Rust server through the Aeroboard CLI.

## Running the system locally

The main development entrypoint is:

```bash
uv run aeroboard start --storage filesystem
```

Use `filesystem` unless you are specifically working on Greptime-backed behavior.

## Recommended contributor loop

For most changes:

1. Start the server in filesystem mode.
2. Run one of the example scripts or a focused local reproduction.
3. Make code changes in the relevant component.
4. Run targeted tests.
5. Re-run the example or reproduction and verify dashboard behavior.

## Test commands

### Python tests

```bash
uv run pytest
```

### Rust formatting

```bash
cd aeroboard-server
cargo fmt
```

### Web linting

```bash
cd aeroboard-web
npm run lint
```

## Working on storage backends

When changing persistence, test at least these cases:

- filesystem mode
- live-only mode for empty-history semantics
- Greptime mode if your change touches the database-specific backend (requires a running Docker container as described in README.md).

Storage regressions are easy to miss if you only validate one backend.

## Working on the Python client

When changing the client API or batching logic:

- update examples if the public usage changed
- run the Python tests, especially payload and run-versioning coverage
- verify that `flush()` behavior still sends partial buffers at shutdown

## Working on the Rust server

When changing server behavior:

- check the HTTP routes in `main.rs`
- verify query filtering and pagination semantics in `history.rs`
- keep live streaming resilient when persistence fails

## Working on the dashboard

When changing frontend behavior:

- verify both initial history load and live updates
- pay attention to typed-array assumptions for scalar series
- test comparison and image timelines if your change affects history shape

## Documentation workflow

The docs site is built with MkDocs.

Preview locally:

```bash
pip install -r docs/requirements.txt
mkdocs serve
```

Build the static site:

```bash
mkdocs build
```

Documentation changes should track real code behavior. If you update a command, endpoint, or public API, update the docs in the same change.