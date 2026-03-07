# Aeroboard 🚀

Aeroboard is a high-performance, real-time machine learning training evaluation platform. It leverages an "Arrow-Native" architecture to achieve blazing-fast, serialization-free telemetry streaming. It streams your metrics from Python to a Rust backend, out to a React frontend at 60 FPS, all without copying arrays or slowing down your training loops.

## System Components

1. **Client SDK (Python/PyArrow)**
2. **"T-Junction" Server (Rust/Axum/Tonic)**
3. **Web Dashboard (React/Vite/uPlot/Arrow-JS)**
4. **Time-Series DB (GreptimeDB - Path A)**

---

## 🚀 Quick Start (One Command)

To install the Aeroboard CLI and build all components (Web + Server):

```bash
chmod +x setup.sh
./setup.sh
```

This will:
1. Install the `aeroboard` python package and CLI.
2. Build the React frontend (`npm run build`).
3. Build the Rust server (`cargo build --release`).

---

## 🏎️ Running Aeroboard

Once setup is complete, you can manage everything via `uv run aeroboard`:

### 1. Start the Dashboard (and Server)
```bash
uv run aeroboard start
```
Go to `http://localhost:3000` to see your metrics!

### 2. Start GreptimeDB (Optional)
If you want persistence, ensure Docker is running and execute:
```bash
uv run aeroboard db
```

---

## 🐍 Using the Python Client

Now that the package is installed, you can simply import it in your training scripts:

```python
import aeroboard

# Initialize
aeroboard.init(run_id="my_experiment")

# Log metrics
aeroboard.log({"loss": 0.01, "accuracy": 0.99})

# Flush at the end
aeroboard.flush()
```

---

## 🔌 Framework Integrations

The Python SDK was designed to be easily pluggable into any ML Framework. 

### Vanilla PyTorch / Training Loop

```python
import time
import aeroboard_client

# Initialize the global client
aeroboard_client.init("grpc://localhost:50051", batch_size=100)

for epoch in range(100):
    for step in range(500):
        # ... training log logic
        loss = 0.5 # Calculate actual loss
        
        # Log your metrics seamlessly
        aeroboard_client.log({
            "loss": loss,
            "epoch": epoch,
            "step": step
        })

# Ensure final buffer is emptied
aeroboard_client.flush()
```

### PyTorch Lightning Integration
To use Aeroboard with PyTorch Lightning, create a custom `Callback` to stream metrics on each batch or epoch.

```python
import pytorch_lightning as pl
import aeroboard_client

class AeroboardLightningCallback(pl.Callback):
    def __init__(self, backend_url="grpc://localhost:50051", batch_size=50):
        super().__init__()
        aeroboard_client.init(url=backend_url, batch_size=batch_size)

    def on_train_batch_end(self, trainer, pl_module, outputs, batch, batch_idx):
        # Ensure loss exists
        loss = outputs.get('loss')
        if loss is not None:
            aeroboard_client.log({
                "train_loss": loss.item(),
                "epoch": trainer.current_epoch,
                "global_step": trainer.global_step
            })

    def on_validation_epoch_end(self, trainer, pl_module):
        # Log validation metrics
        val_metrics = {k: v.item() for k, v in trainer.callback_metrics.items() if "val" in k}
        aeroboard_client.log(val_metrics)
        aeroboard_client.flush()
        
    def on_train_end(self, trainer, pl_module):
        aeroboard_client.flush()

# Usage:
# trainer = pl.Trainer(callbacks=[AeroboardLightningCallback()])
# trainer.fit(model)
```

### Ray Tune Integration
For Ray Tune distributed hyperparameter searching, you can use the `LoggerCallback`.

```python
from ray.tune.logger import LoggerCallback
import aeroboard_client

class AeroboardRayLogger(LoggerCallback):
    def __init__(self, url="grpc://localhost:50051", batch_size=50):
        super().__init__()
        self.url = url
        self.batch_size = batch_size
        self._initialized = False
        
    def log_trial_result(self, iteration: int, trial, result: dict):
        if not self._initialized:
            aeroboard_client.init(self.url, self.batch_size)
            self._initialized = True
            
        # We can prepend the trial ID or experiment name to track overlapping experiments
        trial_id = trial.trial_id
        
        metrics_to_log = {}
        # Filter purely numerical data
        for key, value in result.items():
            if isinstance(value, (int, float)):
                # Store it as "trial_123_loss" to visually segment channels on the frontend
                metrics_to_log[f"{trial_id}_{key}"] = value 
                
        aeroboard_client.log(metrics_to_log)

    def on_experiment_end(self, trials, **info):
        aeroboard_client.flush()

# Usage:
# tune.run(
#    trainable,
#    callbacks=[AeroboardRayLogger()]
# )
```

---

## Architecture Flow Recap
1. Python Client -> `RecordBatch` construction -> **Arrow Flight gRPC**.
2. Rust Server -> Internal `Tokio::Broadcast`.
3. **Path A**: Rust intercepts broadast -> Formats as Influx Line Protocol -> Async POST to GreptimeDB.
4. **Path B**: Rust pulls from broadcast -> Formats as `StreamWriter` binary bytes -> WebSockets `/ws/live`.
5. React Frontend -> Pulls binary `ArrayBuffer` from WS -> Extract native `Float64Array`.
6. `uPlot` React component -> Paints fast Canvas graphs natively at 60 FPS.
