# Architecture

This page describes the runtime architecture of Aeroboard as implemented today.

## High-level topology

Aeroboard consists of three main runtime components and one optional persistence service:

1. Python SDK for telemetry production.
2. Rust server for ingestion, routing, persistence, and history APIs.
3. React dashboard for visualization.
4. GreptimeDB as an optional storage backend.

## Data path

The core design goal is a low-copy telemetry path.

### Ingestion path

The Python client buffers telemetry in process and periodically builds Arrow `RecordBatch` objects. It sends those batches to the Rust server over Arrow Flight.

### T-junction fan-out

Inside the server, ingestion publishes batches into a `tokio::sync::broadcast`-based hub called `TJunction`.

That hub splits traffic into two logical paths:

- persistence
- live streaming

### Persistence path

The persistence loop subscribes to the same broadcast source and forwards supported telemetry into the configured `StorageEngine` implementation.

Important property: live streaming should continue even if persistence fails.

### Live path

WebSocket clients receive Arrow IPC-serialized batches from the same source. The frontend decodes those bytes with `apache-arrow` and merges them into typed-array stores.

## Server responsibilities

The Rust server currently owns several concerns in one binary:

- Arrow Flight ingestion
- HTTP API endpoints
- WebSocket live streaming
- storage abstraction selection
- static frontend serving

This is operationally simple for local use but deliberately centralizes a lot of responsibility into one process.

## Frontend data model

The frontend avoids putting high-frequency series updates into React state. Instead, it maintains a store of typed arrays and only uses React for the surrounding view composition.

That design matches the fact that timeseries updates are append-heavy and performance-sensitive.

## Current architectural strengths

- Arrow is used end to end across ingestion and live streaming.
- `Arc<RecordBatch>` keeps Rust-side clone costs low.
- The storage boundary is explicit in the server.
- Live updates are intentionally decoupled from persistence success.

## Current architectural constraints

- The server is still a single binary with several responsibilities.
- The Python client knows about run resolution through the HTTP API.
- Only a subset of possible telemetry shapes is supported end to end.
- The frontend expects certain history semantics regardless of storage mode, so empty but explicit responses matter.

## When you change the architecture

If you add a new telemetry type, do not stop at one layer. You usually need coordinated changes in:

1. Python schema and client buffering (see `aeroboard-client/src/aeroboard_client/schema.py`).
2. Rust ingestion and telemetry-kind detection.
3. Storage serialization and query support.
4. HTTP response shaping if the UI reads it.
5. Frontend decoding and rendering.

The fastest way to create bugs in Aeroboard is to change only one half of the pipeline.