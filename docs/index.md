# Aeroboard Documentation

Aeroboard is an Arrow-native telemetry platform for machine learning experiments. It combines a Python client, a Rust ingestion and history server, and a React dashboard so you can stream training metrics with low overhead and inspect them live or from persisted history.

This documentation is organized for two audiences:

- Users who want to install Aeroboard, instrument training code, and inspect runs.
- Developers who want to understand the internals, extend storage backends, or contribute changes across the Python, Rust, and web components.

## What Aeroboard does

Aeroboard focuses on a fast telemetry path:

1. Python code buffers scalar metrics, hyperparameters, and images into Arrow `RecordBatch` payloads.
2. Arrow Flight sends those batches to the Rust server.
3. The Rust server fans batches out through a central broadcast hub.
4. One path persists data through the configured storage engine.
5. Another path serializes Arrow IPC frames to the dashboard over WebSockets.

The result is a system that can support real-time updates without pushing chart rendering work back into your training process.

## Documentation map

```{toctree}
:hidden:
:maxdepth: 2
:caption: Getting Started

getting-started/installation
getting-started/quickstart
```

```{toctree}
:hidden:
:maxdepth: 2
:caption: User Guide

user-guide/core-concepts
user-guide/logging-data
user-guide/integrations
user-guide/dashboard
user-guide/workspaces
```

```{toctree}
:hidden:
:maxdepth: 2
:caption: Developer Guide

developer-guide/architecture
developer-guide/repository-layout
developer-guide/development-workflow
developer-guide/storage-model
developer-guide/feature-implementation-plan
developer-guide/workspace-verification
```

```{toctree}
:hidden:
:maxdepth: 2
:caption: Reference

reference/cli
reference/python-sdk
reference/http-api
reference/configuration
```

### Getting started

- Installation covers prerequisites, environment setup, and the basic local workflow.
- Quickstart walks through bringing up the server, logging your first run, and querying the resulting data.

### User guide

- Core Concepts explains runs, auto-versioning, metric naming, buffering, and storage modes.
- Logging Data describes the Python SDK APIs for scalars, hyperparameters, images, and history queries.
- Framework Integrations covers the current PyTorch, Lightning, and Ray Tune patterns.
- Using the Dashboard explains how Aeroboard presents runs, summaries, comparisons, and image timelines.

### Developer guide

- Architecture explains the end-to-end runtime design.
- Repository Layout describes where code lives and which directory owns which concern.
- Development Workflow gives setup, test, lint, and local debugging guidance.
- Storage Model explains the storage abstraction and how filesystem, GreptimeDB, and live-only modes behave.
- Feature Implementation Plan records the scope and verification gates for lifecycle, metric goals, saved views, comparisons, events, and offline capture.

### Reference

- CLI lists the supported `aeroboard` commands.
- Python SDK documents the public client API.
- HTTP API documents the history and run-management endpoints.
- Configuration lists the current environment variables and runtime knobs.

## Typical local workflow

For most contributors and users, the shortest path is:

```bash
./setup.sh
uv run aeroboard start --storage filesystem
uv run examples/torch_example.py
```

Then open `http://localhost:3000` and inspect the generated run.

## Build the docs locally

Aeroboard uses Sphinx with MyST to build these Markdown guides and is configured for hosting on Read the Docs. Documentation builds require Python 3.11 or newer. Run these commands from the repository root:

```bash
python -m pip install -r docs/requirements.txt
python -m sphinx -b dirhtml -W --keep-going docs docs/_build/dirhtml
```

To preview the generated site:

```bash
python -m http.server 8000 --directory docs/_build/dirhtml
```

Open `http://localhost:8000`. Re-run the Sphinx build after editing a guide. Generated output lives in `docs/_build/dirhtml/` and is ignored by Git.
