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

<span id="web-linting"></span>

### Web tests, build, and linting

Use Node.js 22.18 or newer for the frontend regression tests. They use Node's test runner with TypeScript type stripping and cover dashboard configuration precedence and metric aliases.

```bash
cd aeroboard-web
npm test
npm run build
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

The docs site is built with Sphinx and MyST from the Markdown files in `docs/`. Use Python 3.11 or newer. Sphinx settings live in `docs/conf.py`; the toctrees in `docs/index.md` define the navigation.

Build from the repository root:

```bash
python -m pip install -r docs/requirements.txt
python -m sphinx -b dirhtml -W --keep-going docs docs/_build/dirhtml
```

Preview the generated site at `http://localhost:8000`:

```bash
python -m http.server 8000 --directory docs/_build/dirhtml
```

Re-run the Sphinx build after changing a guide. Output in `docs/_build/` is ignored by Git. The `dirhtml` builder preserves directory-style URLs, and MyST resolves existing `.md` links and heading anchors. The heading-anchor hook in `docs/conf.py` also retains published Markdown fragment URLs for code headings, numbered steps, and repeated headings. CI and Read the Docs run strict builds that fail on warnings, including unresolved internal references.

Documentation changes should track real code behavior. If you update a command, endpoint, or public API, update the docs in the same change.

## Workspace integration and browser checks

The standard Python suite includes legacy-client compatibility and local outbox tests. Real HTTP/Flight tests are enabled by an explicit binary path and run against temporary server directories:

```bash
cargo build --manifest-path aeroboard-server/Cargo.toml
AEROBOARD_TEST_BINARY="$PWD/aeroboard-server/target/debug/aeroboard" \
  GREPTIME_TEST_URL=http://127.0.0.1:4000 \
  uv run pytest tests/test_workspace_integration.py
```

Use a disposable Greptime v0.12 instance for this suite. Fixtures create isolated projects and runs, alter their own receipt/backend records to reproduce failure windows, and verify deletion. Without `GREPTIME_TEST_URL`, the filesystem cases run and Greptime cases skip. CI provisions the supported database and runs both.

For browser checks, start a server with the built frontend, seed the example, then run Playwright:

```bash
uv run examples/workspace_example.py
cd aeroboard-web
npm install
npx playwright install chromium
npm run test:browser
```

The browser check covers fresh-browser saved views, logarithmic axes, PNG/CSV downloads, baseline/reducer controls, media-to-chart selection, and mobile rendering. `AEROBOARD_TEST_WEB_URL` selects another server; `CHROMIUM_PATH` selects an existing Chromium executable. Set the example's `AEROBOARD_EXAMPLE_API_BASE_URL` and `AEROBOARD_EXAMPLE_FLIGHT_URL` when using non-default ports. Screenshots and downloads default to `/tmp/aeroboard-browser`; override `AEROBOARD_BROWSER_OUTPUT` as needed.

Run `uv run benchmarks/workspace.py` against a local server for a repeatable 10,002-observation diagnostic. `--persistent` enables the local outbox; `--sdk-source PATH` imports an older SDK checkout for comparison. See the [verification record](workspace-verification.md) for the fixture, measurements, and limits.
