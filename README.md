# Aeroboard 🚀

Aeroboard is a high-performance, real-time machine learning training evaluation platform. It leverages an "Arrow-Native" architecture to achieve blazing-fast, serialization-free telemetry streaming. It streams your metrics from Python to a Rust backend, out to a React frontend at 60 FPS, all without copying arrays or slowing down your training loops.

## System Components

1. **Client SDK (Python/PyArrow)**
2. **"T-Junction" Server (Rust/Axum/Tonic)**
3. **Web Dashboard (React/Vite/uPlot/Arrow-JS)**
4. **Persistence Backend (Filesystem by default, GreptimeDB optional)**

---

## 🚀 Installation

Aeroboard is published as a Python package and uses `uv` cleanly in three modes:

### PyPI CLI install

```bash
uv tool install aeroboard
```

### PyPI project environment install

```bash
uv venv
uv pip install aeroboard
```

### GitHub install

```bash
uv tool install git+https://github.com/cvoscode/aeroboard.git@v0.1.0
```

The Python package is pure Python. On the first `aeroboard setup` or `aeroboard start`, the CLI downloads two matching release artifacts for the installed version:

1. A platform-specific Rust server binary.
2. A platform-independent web bundle.

If you are working from a source checkout instead of an installed release, use the developer bootstrap below to build everything locally with `uv`, `npm`, and `cargo`.

### Developer bootstrap from this repository

```bash
chmod +x setup.sh
./setup.sh
```

This will:
1. Create or update the local `.venv` with `uv`.
2. Install the `aeroboard` package, dev dependencies, and example dependencies.
3. Build the React frontend locally.
4. Build the Rust server locally.

---

## 🏎️ Running Aeroboard

For a project-local environment, use `uv run aeroboard ...`. For a tool install, run `aeroboard ...` directly.

Once setup is complete, you can manage everything via the CLI:

### 1. Start the Dashboard (and Server)
```bash
uv run aeroboard start
```
Go to `http://localhost:3000` to see your metrics!

Filesystem persistence is the default. Useful options:

```bash
uv run aeroboard start --storage filesystem --data-dir .aeroboard
uv run aeroboard start --storage greptime
uv run aeroboard start --port 3100 --flight-port 51051
```

### 2. Start GreptimeDB (Optional)
If you want persistence, ensure Docker is running and execute:
```bash
uv run aeroboard db
```

The CLI manages a dedicated `aeroboard-greptimedb` container and named Docker volume, so it works the same from PyPI installs and source checkouts.

### 3. Run Python tests
```bash
uv run pytest
```

# Aeroboard 🚀

Aeroboard is a high-performance, real-time machine learning training evaluation platform. It leverages an "Arrow-Native" architecture to achieve blazing-fast, serialization-free telemetry streaming. It streams your metrics from Python to a Rust backend, out to a React frontend at 60 FPS, all without copying arrays or slowing down your training loops.

## System Components

1. **Client SDK (Python/PyArrow)**
2. **"T-Junction" Server (Rust/Axum/Tonic)**
3. **Web Dashboard (React/Vite/uPlot/Arrow-JS)**
4. **Persistence Backend (Filesystem by default, GreptimeDB optional)**

---

## 🚀 Installation

Aeroboard is published as a Python package and uses `uv` cleanly in three modes:

### PyPI CLI install

```bash
uv tool install aeroboard
```

### PyPI project environment install

```bash
uv venv
uv pip install aeroboard
```

### GitHub install

```bash
uv tool install git+https://github.com/cvoscode/aeroboard.git@v0.1.0
```

The Python package is pure Python. On the first `aeroboard setup` or `aeroboard start`, the CLI downloads two matching release artifacts for the installed version:

1. A platform-specific Rust server binary.
2. A platform-independent web bundle.

If you are working from a source checkout instead of an installed release, use the developer bootstrap below to build everything locally with `uv`, `npm`, and `cargo`.

### Developer bootstrap from this repository

```bash
chmod +x setup.sh
./setup.sh
```

This will:
1. Create or update the local `.venv` with `uv`.
2. Install the `aeroboard` package, dev dependencies, and example dependencies.
3. Build the React frontend locally.
4. Build the Rust server locally.

---

## 🏎️ Running Aeroboard

For a project-local environment, use `uv run aeroboard ...`. For a tool install, run `aeroboard ...` directly.

Once setup is complete, you can manage everything via the CLI:

### 1. Start the Dashboard (and Server)
```bash
uv run aeroboard start
```
Go to `http://localhost:3000` to see your metrics!

Filesystem persistence is the default. Useful options:

```bash
uv run aeroboard start --storage filesystem --data-dir .aeroboard
uv run aeroboard start --storage greptime
uv run aeroboard start --port 3100 --flight-port 51051
```

### 2. Start GreptimeDB (Optional)
If you want persistence, ensure Docker is running and execute:
```bash
uv run aeroboard db
```

The CLI manages a dedicated `aeroboard-greptimedb` container and named Docker volume, so it works the same from PyPI installs and source checkouts.

### 3. Run Python tests
```bash
uv run pytest
```

### 4. Force a local source build

When you want to rebuild the frontend and Rust server from the checked out repository instead of using release artifacts:

```bash
uv run aeroboard setup --build-from-source
uv run aeroboard start --build-from-source
```

## 🧪 Examples

The `examples/` directory contains several scripts demonstrating how to integrate Aeroboard into different workflows. Use these to quickly verify your installation:

- **Standard PyTorch:** `python examples/torch_example.py` (Demonstrates metrics, hyperparameters, and images)
- **PyTorch Lightning:** `python examples/lightning_example.py` (Integration with the Lightning Trainer)
- **Ray Tune:** `python examples/ray_tune_example.py` (Hyperparameter optimization integration)

To run any example using the local environment:
```bash
uv run python examples/<name>.py
```

When the local server is not using the default ports, the examples share these overrides:
```bash
export AEROBOARD_EXAMPLE_FLIGHT_URL=grpc://localhost:51010
export AEROBOARD_EXAMPLE_API_BASE_URL=http://localhost:3010
uv run python examples/<name>.py
```


---

## 🐍 Using the Python Client

Now that the package is installed, you can simply import it in your training scripts:

```python
import aeroboard_client as aeroboard

# Initialize
aeroboard.init(run_id="my_experiment")

# Or set the API base explicitly when the HTTP server is not on localhost:3000
# aeroboard.init(run_id="my_experiment", url="grpc://localhost:51051", api_base_url="http://localhost:3100")

# Manual hyperparameters for comparison table / parallel coordinates
aeroboard.log_hyperparameters({
    "learning_rate": 0.001,
    "dropout": 0.2,
    "optimizer": "adamw",
})

# Log metrics
aeroboard.log({"train/loss": 0.01, "train/accuracy": 0.99, "step": 1})

# Log an image for the historized timeline viewer
aeroboard.log_image("samples/prediction", open("preview.png", "rb").read(), format="png")

# Flush at the end
aeroboard.flush()
```

---

## 🔌 Framework Integrations

The Python SDK was designed to be easily pluggable into any ML Framework. 

### Vanilla PyTorch / Training Loop

```python
import aeroboard_client as aeroboard
import torch
import torch.nn.functional as F

client = aeroboard.init(run_id="torch-demo", url="grpc://localhost:50051", batch_size=100)
client.log_hyperparameters({
    "optimizer": "adamw",
    "lr": 1e-3,
    "batch_size": 64,
})

for epoch in range(100):
    for step in range(500):
        logits = model(inputs)
        loss = F.cross_entropy(logits, labels)
        accuracy = (logits.argmax(dim=1) == labels).float().mean()

        optimizer.zero_grad()
        loss.backward()
        optimizer.step()

        client.log_metrics(
            {
                "loss": loss,
                "accuracy": accuracy,
                "epoch": epoch,
                "optimizer/lr": optimizer.param_groups[0]["lr"],
            },
            step=epoch * 500 + step,
            prefix="train",
        )

client.flush()
```

`AeroboardClient.log_metrics(...)` accepts plain numbers and scalar tensors, injects a shared `step`, and prefixes bare names like `loss` to `train/loss` when you pass `prefix="train"`.

See the full runnable example in [examples/torch_example.py](examples/torch_example.py).

### Evaluate filesystem vs Greptime with the vanilla Torch example

Filesystem:

```bash
uv run aeroboard start --storage filesystem --data-dir .aeroboard/examples/fs
uv run examples/torch_example.py
```

Greptime:

```bash
uv run aeroboard db
uv run aeroboard start --storage greptime
uv run examples/torch_example.py
```

### PyTorch Lightning Integration
Aeroboard ships with a Lightning-compatible logger that captures both manually logged hyperparameters and `LightningModule.save_hyperparameters()` values.

```python
import lightning.pytorch as pl
import aeroboard_client as aeroboard

client = aeroboard.init(run_id="lightning-demo", batch_size=50)
client.log_hyperparameters({"manual/tag": "baseline"})

class LitModel(pl.LightningModule):
    def __init__(self, hidden_size: int = 64, learning_rate: float = 0.001):
        super().__init__()
        self.save_hyperparameters()
        ...

# Usage:
# trainer = pl.Trainer(logger=aeroboard.AeroboardLightningLogger(run_id="lightning-demo"))
# trainer.fit(model)
```

Logged hyperparameters appear in the comparison table and can be selected as axes in the parallel-coordinates view. Logged images are persisted and can be scrubbed over time with the image timeline slider.

### Evaluate filesystem vs Greptime with the Lightning example

Run the same example workflow against either backend and compare the resulting runs in the UI.

Filesystem:

```bash
uv run aeroboard start --storage filesystem --data-dir .aeroboard/examples/fs
uv run examples/lightning_example.py --backend filesystem --run-prefix backend-eval
```

Greptime:

```bash
uv run aeroboard db
uv run aeroboard start --storage greptime
uv run examples/lightning_example.py --backend greptime --run-prefix backend-eval
```

The example tags each run with `storage/backend`, the API base, and the Flight endpoint so the comparison table can be used to verify which backend produced each run.

### Ray Tune Integration
Aeroboard ships with `AeroboardRayTuneLoggerCallback`, which creates one Aeroboard run per Ray trial, logs the trial config as hyperparameters, and streams numeric trial results as scalar metrics.

Install the optional Ray dependency first:

```bash
uv sync --extra ray --dev
```

```python
from ray import tune
import aeroboard_client as aeroboard

def objective(config):
    for step in range(20):
        tune.report(loss=1.0 / (step + 1) + config["lr"], accuracy=step / 20)

tuner = tune.Tuner(
    objective,
    param_space={"lr": tune.grid_search([1e-2, 5e-2]), "hidden_size": tune.choice([32, 64])},
    run_config=tune.RunConfig(
        callbacks=[
            aeroboard.AeroboardRayTuneLoggerCallback(
                url="grpc://localhost:50051",
                api_base_url="http://localhost:3000",
                run_prefix="ray-demo",
                metric_prefix="train",
            )
        ]
    ),
)

tuner.fit()
```

See the full runnable example in [examples/ray_tune_example.py](examples/ray_tune_example.py).

---

## Architecture Flow Recap
1. Python Client -> `RecordBatch` construction -> **Arrow Flight gRPC**.
2. Rust Server -> Internal `Tokio::Broadcast`.
3. **Path A**: Rust intercepts broadast -> Formats as Influx Line Protocol -> Async POST to GreptimeDB.
4. **Path B**: Rust pulls from broadcast -> Formats as `StreamWriter` binary bytes -> WebSockets `/ws/live`.
5. React Frontend -> Pulls binary `ArrayBuffer` from WS -> Extract native `Float64Array`.
6. `uPlot` React component -> Paints fast Canvas graphs natively at 60 FPS.

---

## Documentation

The repository now includes a Read the Docs-compatible MkDocs site under `docs/`.

Build or preview it locally with:

```bash
pip install -r docs/requirements.txt
mkdocs serve
```

To generate the static site:

```bash
mkdocs build
```

The documentation is organized into:

- Getting Started
- User Guide
- Developer Guide
- Reference

Read the Docs configuration lives in `.readthedocs.yaml`, and the MkDocs navigation is defined in `mkdocs.yml`.
