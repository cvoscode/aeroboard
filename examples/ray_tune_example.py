"""Aeroboard Ray Tune example.

Typical usage:

    uv sync --extra ray --dev
    uv run aeroboard start --storage filesystem --data-dir .aeroboard/examples/fs
    uv run examples/ray_tune_example.py

Set `AEROBOARD_EXAMPLE_FLIGHT_URL` and `AEROBOARD_EXAMPLE_API_BASE_URL` when the
server is running on non-default ports.
"""

from __future__ import annotations

import math
import time

from ray import tune
import aeroboard_client as aeroboard
from _config import get_example_api_base_url, get_example_flight_url


def objective(config: dict[str, float]) -> None:
    lr = config["lr"]
    hidden_size = int(config["hidden_size"])

    for step in range(1, 21):
        loss = (1.0 / step) + (lr * 4.0) + (64 - hidden_size) * 0.002
        accuracy = max(0.0, min(1.0, 1.0 - loss * 0.4 + math.sin(step / 4.0) * 0.02))
        tune.report(loss=loss, accuracy=accuracy, training_iteration=step)
        time.sleep(0.05)


def main() -> None:
    callback = aeroboard.AeroboardRayTuneLoggerCallback(
        url=get_example_flight_url(),
        api_base_url=get_example_api_base_url(),
        batch_size=20,
        run_prefix="ray-tune-demo",
        metric_prefix="train",
    )

    tuner = tune.Tuner(
        objective,
        param_space={
            "lr": tune.grid_search([0.01, 0.03, 0.05]),
            "hidden_size": tune.choice([32, 64]),
        },
        run_config=tune.RunConfig(name="aeroboard-ray-example", callbacks=[callback]),
    )
    tuner.fit()


if __name__ == "__main__":
    main()
