# Aeroboard 🚀

Aeroboard is a high-performance, real-time machine learning training evaluation platform. It leverages an "Arrow-Native" architecture to achieve blazing-fast, serialization-free telemetry streaming. It streams your metrics from Python to a Rust backend, out to a React frontend at 60 FPS, all without copying arrays or slowing down your training loops.

## System Components

1. **Client SDK (Python/PyArrow)**
2. **"T-Junction" Server (Rust/Axum/Tonic)**
3. **Web Dashboard (React/Vite/uPlot/Arrow-JS)**
4. **Persistence Backend (Filesystem by default, GreptimeDB optional)**

---

## 🚀 Quick Start (One Command)

To install the Aeroboard CLI and build all components (Web + Server):

First, install `uv`. Then run:

```bash
chmod +x setup.sh
./setup.sh
```

This will:
1. Create or update the local `.venv` with `uv`.
2. Install the `aeroboard` Python package, dev dependencies, and example dependencies.
3. Build the React frontend (`npm run build`).
4. Build the Rust server (`cargo build --release`).

If you prefer the manual flow, use `uv` directly:

```bash
uv sync --extra examples --dev
uv run aeroboard setup
```

This keeps Python environment creation, dependency resolution, and command execution under `uv`.

---

## 🏎️ Running Aeroboard

Once setup is complete, you can manage everything via `uv run aeroboard`:

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

### 3. Run Python tests
```bash
uv run pytest
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
