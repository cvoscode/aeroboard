# Aeroboard Implementation Plan

This document outlines the implementation of the Aeroboard ML dashboard system, which provides real-time monitoring of ML experiments using Arrow native streaming.

## Overview

Aeroboard is a real-time dashboard for monitoring machine learning experiments. It ingests telemetry data via gRPC Flight, persists it to various storage backends, and provides a web UI for visualization.

## Phase 1: Server Boundary Completion ✅

### Goals
- Finalize StorageEngine ownership by unifying storage.rs, persistence.rs, and history.rs
- Standardize history/query contracts
- Make run resolution server-authoritative
- Harden config and backend contracts

### Completed Items

#### 1. Unified storage.rs ✅
The `storage.rs` file has been consolidated to provide a clean `StorageEngine` trait with three implementations:

**StorageEngine Trait:**
- `append_batch(&self, batch: Arc<TelemetryBatch>)` - Persist telemetry batches
- `query_history(&self)` - Query scalar metrics history
- `query_hyperparameters(&self)` - Query hyperparameter settings
- `query_image_history(&self)` - Query image history
- `list_runs(&self)` - List all run IDs
- `clear_history(&self)` - Clear all persisted data

**Implementations:**

1. **LiveOnlyStore** - Stream-only mode with no persistence
   - Accepts batches without storing
   - Returns empty history queries
   - Cannot delete history (no data to delete)

2. **FilesystemStore** - Filesystem-based persistence
   - Stores batches as Arrow files per run
   - Uses manifest.json to track run existence
   - Uses index.json for image metadata
   - Supports reading from stored batches or individual image files

3. **GreptimeStore** - GreptimeDB-based persistence
   - Uses InfluxDB line protocol for ingestion
   - Uses SQL queries for retrieval
   - Handles connection errors gracefully
   - Implements clear table with fallback strategies

#### 2. Unified persistence.rs ✅
The `persistence.rs` file has been cleaned up to remove duplication:

- `start_persistence_loop()` - Receives batches via t_junction broadcast channel and persists using StorageEngine
- Graceful error handling for persistence failures
- Broadcast channel lag handling
- Clean shutdown on channel close

#### 3. Standardized history.rs ✅
The `history.rs` file has been standardized with:

**Data Structures:**
- `HistoryPoint` - Scalar metric data point with timestamp, run_id, name, value
- `ImageHistoryPoint` - Image data point with timestamp, run_id, name, format, image_base64
- `HyperparameterEntry` - Hyperparameter with key, value, value_type
- `MetricSummary` - Metric statistics with last, min, max values
- `RunSummary` - Complete run summary with hyperparameters and metrics

**Handler Functions:**
- `history_handler()` - GET /api/history - Queries history with hyperparameters
- `image_history_handler()` - GET /api/history/images - Queries image history
- `runs_handler()` - GET /api/runs - Lists runs from t_junction and storage
- `resolve_run_id_handler()` - POST /api/runs/resolve - Resolves canonical run ID
- `delete_history_handler()` - DELETE /api/history - Clears persisted history

**Helper Functions:**
- `resolve_run_id()` - Resolves canonical run ID from base run ID
- `build_run_summaries()` - Aggregates points into run summaries
- `apply_history_filters()` - Filters results by run_id, metric, timestamp range
- `apply_image_filters()` - Filters image results with same semantics
- `page_vec()` - Cursor-based pagination
- `csv_set()` - Parses comma-separated values into BTreeSet

**API Contract:**
- Backend-agnostic queries that work with all storage backends
- Explicit empty results rather than relying on frontend assumptions
- Descriptive error messages with backend-specific context
- Cursor-based pagination for consistent pagination behavior

### Verification

Phase 1 compiles successfully with no errors or warnings:
```
cargo check --manifest-path aeroboard-server/Cargo.toml
# Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
```

## Phase 2: Python SDK Consolidation ✅

### Goals
- Consolidate Python CLI and SDK code
- Simplify dependency management and build workflow
- Ensure consistent client-server API contracts

### Completed Items ✅

#### 1. Python CLI Consolidation ✅
- [x] Merged `src/aeroboard/main.py` commands into `aeroboard-client/src/aeroboard_client/cli.py`
- [x] Updated `pyproject.toml` to declare CLI entry points
- [x] Ensured `aeroboard start` works consistently across environments
- [x] Removed duplicate build logic
- [x] Removed old duplicate `src/aeroboard/` directory

**CLI Commands Available:**
- `aeroboard setup` - Build frontend and server
- `aeroboard start` - Start server and web UI
- `aeroboard db` - Start GreptimeDB via Docker Compose
- `aeroboard stop-db` - Stop GreptimeDB
- `aeroboard status` - Show Aeroboard status
- `aeroboard check-run` - Check status of specific run
- `aeroboard log-run` - Start logging metrics to existing run

#### 2. Client API Standardization ✅
- [x] Standardized telemetry sending interface in `AeroboardClient`
- [x] Standardized history querying interface
- [x] Standardized image loading interface
- [x] Documented API in docstrings

**Client Features:**
- `AeroboardClient` - Main client for sending telemetry data
- `AeroboardLightningCallback` - PyTorch Lightning callback for hparams logging
- `log()` - Send scalar metrics
- `log_hyperparameters()` - Send hyperparameter values
- `log_image()` - Send image data (PIL, numpy, torch)
- `query_history()` - Query scalar history
- `query_hyperparameters()` - Query hyperparameters via run summaries from `/api/history`
- `query_images()` - Query images
- `list_runs()` - List all run IDs
- `flush()` - Force flush buffered data
- `close()` - Clean shutdown

**Schemas:**
- `scalar_schema` - Scalar telemetry schema
- `image_schema` - Image telemetry schema
- `hyperparameter_schema` - Hyperparameter schema

**Error Classes:**
- `AeroboardClientError` - General client errors
- `FlightConnectionError` - Flight server connection errors

### Verification ✅

The CLI works correctly:
```
$ aeroboard --help
Usage: aeroboard [OPTIONS] COMMAND [ARGS]...

Commands:
  check-run  Check the status of a specific run.
  db         Start GreptimeDB via Docker Compose.
  log-run    Start logging metrics to an existing run.
  setup      Build the frontend and prepare the server.
  start      Start the Aeroboard server and web UI.
  status     Show Aeroboard status.
  stop-db    Stop GreptimeDB via Docker Compose.
```

The package installs and imports correctly:
```
$ uv run python -c "from aeroboard_client import AeroboardClient"
✅ AeroboardClient imported successfully
```

## Phase 3: Frontend Refactor ✅

### Goals
- Use useAeroboard hook for API calls
- Consolidate image loading logic
- Standardize error handling
- Simplify component APIs

### Completed Items ✅

#### 1. useAeroboard Hook Enhancements ✅
- [x] Implemented history querying interface with pagination support
- [x] Implemented image loading interface with upsert logic
- [x] Implemented hyperparameter tracking interface
- [x] Implemented WebSocket connection for live stream updates
- [x] Added `loadHistory` callback for manual history refresh
- [x] Improved error handling with descriptive console warnings
- [x] Added better error messages for non-OK responses
- [x] Added WebSocket error handler for connection failures

#### 2. Component Implementation ✅
- [x] `ImagePanel` - Simple wrapper for ImageTimelineViewer
- [x] `LineChart` - Uses history queries via useAeroboard hook
- [x] `ColorPicker` - Telemetry sending via WebSocket streaming
- [x] `ComparisonTable` - Uses run summaries from useAeroboard hook
- [x] `ImageTimelineViewer` - Uses image series from useAeroboard hook
- [x] No duplicated image loading logic - single source via useAeroboard

#### 3. API Contract ✅
- [x] Standardized telemetry receiving interface via WebSocket
- [x] Standardized history querying interface via HTTP API
- [x] Standardized image loading interface via HTTP API
- [x] Documented API in interface definitions

### Verification ✅

The frontend builds successfully:
```
npm run build
# ✓ built in 2.08s
```

**Key Changes in Phase 3:**
1. Enhanced `useAeroboard.ts` with better error handling
2. Added `loadHistory` callback for manual history refresh
3. Improved error messages for non-OK responses and WebSocket errors
4. Removed unused payload error interfaces

**Files Modified in Phase 3:**
- `aeroboard-web/src/hooks/useAeroboard.ts` - Enhanced error handling and callbacks

The frontend hook now provides:
- Consistent API for all telemetry types (scalars, images, hyperparameters)
- Comprehensive error handling with console warnings
- Manual history refresh capability via `loadHistory` callback

## Phase 4: Reliability and Cleanup (TODO)

### Goals
- Improve error handling and recovery
- Remove unused code and dead logic
- Add comprehensive error messages
- Improve logging and debugging

### Pending Items

#### 1. Error Handling
- [ ] Add proper error handling in all handlers
- [ ] Add connection retry logic for GreptimeDB
- [ ] Add graceful degradation for backend failures
- [ ] Add better logging with context

#### 2. Code Cleanup
- [ ] Remove unused dependencies
- [ ] Remove dead code paths
- [ ] Add comprehensive error messages
- [ ] Improve code documentation

## Phase 5: Workflow and Documentation (TODO)

### Goals
- Improve documentation
- Add comprehensive examples
- Streamline development workflow
- Add troubleshooting guides

### Pending Items

#### 1. Documentation
- [ ] Update README with setup instructions
- [ ] Add API documentation
- [ ] Add troubleshooting guide
- [ ] Add architecture diagrams

#### 2. Examples
- [ ] Add more examples for different use cases
- [ ] Add configuration examples
- [ ] Add deployment guides

---

## Current Status

**Phase 1 (Server Boundary Completion): ✅ COMPLETE**

Phase 1 has successfully completed the server-side architecture by:
1. ✅ Unifying storage.rs with StorageEngine trait and implementations
2. ✅ Unifying persistence.rs with clean persistence loop
3. ✅ Standardizing history.rs contracts with handlers
4. ✅ Making run resolution server-authoritative
5. ✅ Compiling successfully with no errors or warnings

**Files Modified in Phase 1:**
- `aeroboard-server/src/storage.rs` - Unified storage engine implementations
- `aeroboard-server/src/persistence.rs` - Cleaned up persistence loop
- `aeroboard-server/src/history.rs` - Standardized handlers with StorageEngine

**Phase 2 (Python SDK Consolidation): ✅ COMPLETE**

Phase 2 has successfully consolidated the Python SDK by:
1. ✅ Merging CLI commands into aeroboard-client/src/aeroboard_client/cli.py
2. ✅ Standardizing client API with comprehensive documentation
3. ✅ Adding history and image query interfaces with module-level helper APIs in `aeroboard_client`
4. ✅ Removing duplicate src/aeroboard/ code
5. ✅ Ensuring package builds and imports correctly
6. ✅ Setting up proper pyproject.toml with editable installs

**Files Created/Modified in Phase 2:**
- `aeroboard-client/src/aeroboard_client/cli.py` - Consolidated CLI commands
- `aeroboard-client/src/aeroboard_client/client.py` - Enhanced client with all methods
- `aeroboard-client/src/aeroboard_client/schema.py` - Arrow schemas
- `aeroboard-client/src/aeroboard_client/__init__.py` - Public API exports
- `pyproject.toml` - Updated with CLI entry points and dependencies

The Python client now provides:
- Consistent query APIs for persisted scalars, images, and hyperparameters
- Module-level helper APIs in the consolidated `aeroboard_client` package for examples and simple scripts
- Comprehensive error handling with specific error types
- Lightning callback integration for hparams logging
- Image support for PIL, numpy, torch tensors

This foundation enables the subsequent phases to build on a consistent, well-defined client API.

## Summary

Phase 1 and Phase 2 of the Aeroboard implementation are complete. The system now has:

**Phase 1 - Server:**
- A unified `StorageEngine` trait with three backend implementations
- Clean separation between live streaming and persistent storage
- Standardized API contracts for history queries
- Proper error handling and graceful degradation

**Phase 2 - Client:**
- Consolidated Python SDK with comprehensive client API
- CLI commands unified in aeroboard-client
- Well-documented with docstrings and examples
- Proper dependency management and installation

The next phase (Phase 3) will focus on frontend refactoring to use the unified client API.