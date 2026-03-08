# Framework Integrations

Aeroboard is designed to work both with explicit training loops and with common ML framework hooks.

## Plain Python or PyTorch loops

The simplest pattern is to keep an `AeroboardClient` instance alongside your optimizer and training loop.

```python
import aeroboard_client as aeroboard

client = aeroboard.init(run_id="torch-demo", batch_size=50)
client.log_hyperparameters({"optimizer": "adam", "lr": 1e-3})

for step in range(1000):
    client.log_metrics({"loss": loss, "accuracy": accuracy}, step=step, prefix="train")

client.flush()
```

This pattern gives you the most control over metric names and flush timing.

## PyTorch Lightning logger

For Lightning, the preferred integration is `lightning_logger()`:

```python
import aeroboard_client as aeroboard
import lightning.pytorch as pl

client = aeroboard.init(run_id="lightning-demo")
logger = client.lightning_logger()

trainer = pl.Trainer(logger=logger)
trainer.fit(model)
```

The logger:

- forwards metrics emitted through `LightningModule.log(...)`
- records hyperparameters passed through Lightning's logger interface
- tracks the most recent step value and reuses it when Lightning omits an explicit step
- flushes remaining telemetry during `save()` and `finalize()`

## PyTorch Lightning callback

Aeroboard also exposes `AeroboardLightningCallback`.

Use it if you want a callback-oriented integration instead of a logger-oriented one.

The callback attempts to:

- log model and logger hyperparameters at fit start
- emit training metrics at the end of train batches
- emit validation summaries at validation epoch end
- flush data when training ends

The logger is the cleaner default when your training code already follows Lightning's logging model.

## Ray Tune callback

Aeroboard ships with `AeroboardRayTuneLoggerCallback`, which creates one Aeroboard run per Ray trial.

```python
from ray import tune
import aeroboard_client as aeroboard

callback = aeroboard.AeroboardRayTuneLoggerCallback(
    run_prefix="ray-demo",
    metric_prefix="train",
)
```

Behavior:

- each trial gets its own Aeroboard run
- trial configuration is logged as hyperparameters
- numeric trial results are streamed as scalar metrics
- standard Ray metadata such as trial identifiers is attached as hyperparameters

## Example workflows in this repository

The repository includes runnable examples:

- `examples/torch_example.py`
- `examples/lightning_example.py`
- `examples/ray_tune_example.py`

The Lightning example is especially useful for comparing filesystem and Greptime-backed runs because it tags each run with storage metadata that appears in comparison views.

## Choosing an integration style

Use this rule of thumb:

- Prefer direct `AeroboardClient` calls in plain loops or when you need tight control.
- Prefer `lightning_logger()` for Lightning projects.
- Prefer `AeroboardRayTuneLoggerCallback` for Ray Tune experiments.

When in doubt, start with direct client logging because it makes metric naming and flush behavior explicit.