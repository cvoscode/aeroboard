# Quickstart

This quickstart takes you from a clean checkout to a visible run in the dashboard.

## 1. Build the project

```bash
./setup.sh
```

## 2. Start the server

Use filesystem storage first. It has the lowest operational overhead and matches the current default.

```bash
uv run aeroboard start --storage filesystem --data-dir .aeroboard
```

Once the server is running, open `http://localhost:3000`.

## 3. Log a first run

You can use one of the bundled examples:

```bash
uv run examples/torch_example.py
```

Or create a minimal script:

```python
import aeroboard_client as aeroboard

client = aeroboard.init(run_id="quickstart-demo")
client.log_hyperparameters({"optimizer": "adamw", "lr": 1e-3})

for step in range(10):
    client.log_metrics({"loss": 1.0 / (step + 1), "accuracy": step / 10}, step=step, prefix="train")

client.flush()
```

Run it with:

```bash
uv run your_script.py
```

## 4. Inspect the dashboard

After metrics start arriving, the dashboard will:

- Load persisted history from the HTTP API.
- Subscribe to the live telemetry stream over WebSocket.
- Render scalar metrics from typed-array stores.
- Show hyperparameters in comparison-oriented views.

If you log images, they also appear in the timeline viewer.

## 5. Query history programmatically

The Python client can query the HTTP API as well:

```python
import aeroboard_client as aeroboard

client = aeroboard.init(run_id="quickstart-demo")
history = client.query_history(metric="train/loss")
print(history["rows"][:3])
```

## 6. Understand run naming

Run identifiers are auto-versioned. If `quickstart-demo` already exists, Aeroboard will resolve a new identifier such as `quickstart-demo-v2` or `quickstart-demo-v3`.

Resolution happens by asking the server for `/api/runs/resolve`, with a fallback to listing runs from `/api/runs` if necessary.

## 7. Switch storage modes when needed

Use these modes depending on what you need:

- `filesystem`: best default for local development and demos.
- `greptime`: database-backed persistence through GreptimeDB.
- `live`: no persistence, live streaming only.

In `live` mode, the dashboard can still show new incoming data, but persisted history endpoints return empty results.