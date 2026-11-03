# Aeroboard 🚀

Aeroboard is a high-performance, real-time machine learning training evaluation platform. It leverages an "Arrow-Native" architecture to achieve blazing-fast, serialization-free telemetry streaming. It streams your metrics from Python to a Rust backend, out to a React frontend at 60 FPS, all without copying arrays or slowing down your training loops.

Projects, run lifecycle and resume, metric objectives, saved views, baseline and multi-seed comparisons, linked events/media, and disk-backed offline recovery are available in the [workspace guide](docs/user-guide/workspaces.md). Try [the runnable example](examples/workspace_example.py) after starting the server.

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

**Network access.** The server listens on `127.0.0.1` only and has no authentication.
To log from or view on other machines, start it with `--host 0.0.0.0` on a trusted
network. Cross-origin browser access is off unless you add `--cors-origin <origin>`.

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

# Log audio: encoded files/bytes pass through untouched; waveforms become 16-bit WAV
aeroboard.log_audio("eval/tts", "outputs/sample.mp3")
aeroboard.log_audio("eval/speech", waveform, sample_rate=16_000)  # numpy array or torch tensor

# Log video: pass an encoded, browser-playable file or bytes (MP4/H.264, WebM, GIF)
aeroboard.log_video("eval/rollout", "videos/episode_10.mp4")

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

### Audio and video

Audio and video clips appear in the **Audio / Video** tab, one card per stream with a clip slider that follows the newest clip.

- The SDK never transcodes video. Encode frames first (for example `ffmpeg -i frames_%04d.png -c:v libx264 -pix_fmt yuv420p out.mp4`) and log the file.
- Each clip can be at most 32 MB. The server accepts Flight messages up to `AEROBOARD_MAX_MESSAGE_MB` (default `64`).
- With persistent storage, live updates carry only clip metadata. The browser streams the bytes from `/api/history/media/content`, with HTTP Range support for seeking, and only when a clip is shown. In live-only mode the bytes are pushed over the WebSocket instead.

### Optimal value reference line

Click the crosshair button on any scalar chart to set an optimal (target) value. It is drawn as a dashed horizontal line, the y-axis grows to keep it in view, and the value is saved per chart in the browser.

Playback pins the current clip, so a newly logged clip won't replace a video while it plays. A **Latest** button then jumps back to the newest clip.

### Initial dashboard view (starred charts, order, targets)

You can preconfigure which charts start starred (expanded and pinned to the top), their order, and target lines. There are two layers. Anything a user changes in the UI overrides both, and **Reset chart layout** in the sidebar clears those UI changes.

**1. Server-wide defaults:** create `aeroboard.toml` in the directory where you run `aeroboard start`, before starting the server. To select another file, set `AEROBOARD_DASHBOARD_CONFIG`; relative paths also resolve from the directory where you invoke the CLI. The selected file is re-read on every page load, and a missing explicitly selected file or malformed config shows a banner in the dashboard. When launching the Rust binary directly, paths resolve from its working directory.

```toml
[dashboard]
starred = ["loss", "val/accuracy"]   # expanded + pinned to the top
order   = ["loss", "accuracy", "lr"] # unlisted charts follow alphabetically

[dashboard.targets]                  # dashed target lines
"val/accuracy" = 0.95
loss = 0.05
```

```bash
uv run aeroboard start
# Or use a different config, relative to the current directory:
AEROBOARD_DASHBOARD_CONFIG=config/dashboard.toml uv run aeroboard start
```

**2. Per run, from the training script:** these values override the server defaults for that run.

```python
aeroboard.init(
    run_id="resnet50",
    dashboard={"starred": ["loss", "accuracy"], "targets": {"accuracy": 0.95}},
)
# or later: aeroboard.configure_dashboard(starred=[...], targets={...}, order=[...])
```

Metric names can be full (`train/loss`) or chart names (`loss`). A run's `starred`/`order` list replaces the server's list. Targets resolve per chart: a run's `val/loss=0.1` overrides a server's `loss=0.05` on the grouped `loss` chart, even though the names differ. In compare mode, starred/order lists are combined, and the first selected run with a matching target wins; server targets supply the remaining defaults. Per-run settings travel as a reserved hyperparameter (`__aeroboard_dashboard__`), which the dashboard and `query_hyperparameters()` hide.

See the [configuration reference](docs/reference/configuration.md#initial-dashboard-view) for precedence, metric matching, and reset behavior.

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

The documentation in `docs/` uses Sphinx with MyST for Markdown support and is configured for Read the Docs hosting. Documentation builds require Python 3.11 or newer.

Build it from the repository root:

```bash
python -m pip install -r docs/requirements.txt
python -m sphinx -b dirhtml -W --keep-going docs docs/_build/dirhtml
```

Preview the generated site at `http://localhost:8000`:

```bash
python -m http.server 8000 --directory docs/_build/dirhtml
```

The documentation is organized into:

- Getting Started
- User Guide
- Developer Guide
- Reference

Sphinx settings live in `docs/conf.py`, navigation is defined by the toctrees in `docs/index.md`, and hosting configuration lives in `.readthedocs.yaml`. The `dirhtml` builder preserves directory-style page URLs. Re-run the build after editing a guide; generated output in `docs/_build/` is ignored by Git. CI and Read the Docs treat documentation warnings as build failures.
