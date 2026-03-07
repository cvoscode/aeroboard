# Aeroboard - Architecture and Implementation Plan

## 1. System Overview
Aeroboard is a high-performance, real-time model training evaluation platform. It leverages an "Arrow-Native" architecture to achieve serialization-free data paths. The system enables machine learning practitioners to log scalars, distributions, and images, persisting them in a high-performance Time-Series Database (TSDB) while concurrently streaming them to a live web dashboard at 60 FPS.

## 2. Core Architecture

### 2.1 The Components
- **Client SDK (Python):** Collects metrics, images, and distributions from ML scripts (e.g., PyTorch, JAX). Packages data as Apache Arrow batches and securely streams them to the backend using gRPC / Arrow Flight. Should be also compatible with lightning and ray tune.
- **Backend (Rust):** A blazing-fast ingestion and routing server using `axum`/`tonic` and `tokio`. It serves as the "T-Junction" of the system.
- **Database (TSDB):** **GreptimeDB** (recommended for a lightweight, Rust-native, Arrow-compatible solution).
- **Frontend (Web):** A real-time dashboard subscribing to WebSockets. It receives binary Arrow batches and renders them at 60 FPS using zero-copy (or minimal-copy) mechanisms over `apache-arrow` JS and `uPlot`/`ECharts`.

### 2.2 Data Flow
1. **Ingestion:** The Python Trainer batches logs and transmits them via Arrow Flight / gRPC to the Rust Backend.
2. **The "T-Junction" (Tokio Broadcast):** 
   - The Rust backend receives the Arrow batch in memory.
   - It leverages `tokio::spawn` to concurrently execute two paths:
     - **Path A (Persistence):** Writes the batch asynchronously to GreptimeDB / InfluxDB.
     - **Path B (Live Stream):** Pushes the raw binary Arrow IPC batch into a `tokio::sync::broadcast` channel.
3. **Delivery:** Web clients viewing the "Live Board" establish WebSocket connections to the Rust server, which pushes binary frames from the broadcast channel.
4. **Rendering:** The browser receives the binary WebSocket frame, parses it with the `apache-arrow` JavaScript library, and directly updates the WebGL/Canvas rendering engines (`ECharts` / `uPlot`).

---

## 3. Technology Stack

### Backend
- **Language:** Rust
- **Async Runtime:** `tokio`
- **Web/WebSocket Framework:** `axum`
- **gRPC/Flight Server:** `tonic`, `arrow-flight`
- **Data Manipulation:** `arrow-rs`
- **Database Client:** Proprietary DB crate (`greptimedb-client` or HTTP API using Arrow IPC)

### Frontend
- **Framework:** React or Svelte (with Vite)
- **Data Parsing:** `apache-arrow` (npm package)
- **Visualization:** `ECharts` (for histograms/distributions, images, time series charts, possible Webgpu/WebGL)
- **Communication:** Native WebSockets `WebSocket` API (Binary Type: `arraybuffer`)

### Client SDK
- **Language:** Python
- **Libraries:** `pyarrow`, `grpcio`, `flight`

---

## 4. Detailed Implementation Plan

### Phase 1: Foundation & Project Structure Setup
1. **Initialize Repositories/Workspaces:**
   - Create Rust workspace for the server (`/aeroboard-server`).
   - Create Python package locally (`/aeroboard-client`).
   - Create Frontend project (`/aeroboard-web`).
2. **Define Arrow Schemas:**
   - Define strict Arrow schemas for `scalars` (float64), `images` (binary/encoded arrays), and `distributions` (lists of floats).
3. **Database Initialization:**
   - Spin up a GreptimeDB docker container.
   - Validate Arrow batch insertions via Rust.

### Phase 2: Ingestion & The "T-Junction" (Rust Backend)
1. **Arrow Flight Ingestion Endpoint:**
   - Set up an Arrow Flight server using `tonic`.
   - Implement `do_put` to receive `RecordBatch` streams from the Python client.
2. **Tokio Broadcast Hub:**
   - Implement a central channel `tokio::sync::broadcast::channel`.
   - Upon receiving a `RecordBatch`, push a lightweight clone (Arrow in Rust is `Arc`-backed) into the channel.
3. **Async Persistence (Path A):**
   - Consume the `RecordBatch` and construct GreptimeDB insert requests. 

### Phase 3: WebSocket Streaming (Rust Backend)
1. **WebSocket Route:**
   - Add an `/ws/live` route via `axum`.
2. **Binary IPC Encoding (Path B):**
   - When a client connects, subscribe them to the broadcast channel.
   - For every intercepted `RecordBatch`, serialize it to Arrow IPC format using `arrow::ipc::writer::StreamWriter`.
   - Send as a binary WebSocket frame.

### Phase 4: Frontend Development & Zero-Copy Rendering
1. **WebSocket Client Connection:**
   - Connect web app to `ws://localhost:PORT/ws/live`.
   - Set `ws.binaryType = "arraybuffer"`.
2. **Arrow Deserialization in Browser:**
   - Listen for `onmessage`, pass the ArrayBuffer to `RecordBatchReader.from(buffer)` (from `apache-arrow`).
3. **High-Performance UI Configuration:**
   - Extract raw `Float64Array` typed arrays from the Arrow columns.
   - Pass typed arrays directly into `uPlot` data structures (avoiding Javascript array copies).
   - Implement Image rendering by extracting binary chunks, creating `Blob` URLs, and assigning them to `<img>` or canvas elements.

### Phase 5: Python Client SDK Development
1. **Client API Construction:**
   - Implement `aeroboard.init(...)` and `aeroboard.log({"loss": 0.5, "epoch": 1, "grad_hist": [...]})`.
2. **Batching Mechanism:**
   - Buffer logs in RAM. On a specific threshold (e.g., every 50ms or 100 rows), construct a `pyarrow.RecordBatch`.
3. **Transmission:**
   - Send the constructed batch to the Rust backend using `pyarrow.flight.FlightClient.do_put()`.

---

## 5. Extensibility & Considerations
- **Downsampling:** For long-running trainings, fetching historical data directly from GreptimeDB should be tsdownsample or a database native solution.
- **Image Compression:** Raw Arrow binary for uncompressed images is expensive over WS. Images should be encoded as WebP bytes *before* putting them in the Arrow array layout in Python.
- **Schema Evolution:** Allow Python to send a schema change log event so the frontend and database can dynamically adapt.
