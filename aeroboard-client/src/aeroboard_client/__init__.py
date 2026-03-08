"""
Aeroboard Python Client SDK - Public API

This module provides a Python client for sending telemetry data to the Aeroboard
Rust backend via gRPC Flight protocol, and for querying historical data.

Example:
    >>> from aeroboard_client import AeroboardClient
    >>> client = AeroboardClient(run_id="my-experiment")
    >>> client.log({"loss": 0.5, "accuracy": 0.8})
    >>> client.log_hyperparameters({"learning_rate": 0.001, "batch_size": 32})
    >>> client.flush()
    >>> client.close()
"""

from aeroboard_client.client import (
    AeroboardClient,
    AeroboardLightningLogger,
    AeroboardRayTuneLoggerCallback,
    AeroboardLightningCallback,
    AeroboardClientError,
    FlightConnectionError,
)

_default_client: AeroboardClient | None = None


def get_default_client() -> AeroboardClient | None:
    return _default_client


def init(
    run_id: str | None = None,
    url: str = "grpc://localhost:50051",
    api_base_url: str | None = None,
    batch_size: int = 100,
) -> AeroboardClient:
    global _default_client
    _default_client = AeroboardClient(
        run_id=run_id,
        url=url,
        api_base_url=api_base_url,
        batch_size=batch_size,
    )
    return _default_client


def log(metrics: dict[str, float]) -> None:
    client = _default_client or init()
    client.log(metrics)


def log_metrics(
    metrics: dict[str, object],
    *,
    step: int | float | None = None,
    prefix: str | None = None,
) -> None:
    client = _default_client or init()
    client.log_metrics(metrics, step=step, prefix=prefix)


def log_hyperparameters(hyperparameters: dict, step: int | float | None = None) -> None:
    client = _default_client or init()
    client.log_hyperparameters(hyperparameters, step=step)


def log_image(name: str, image, *, format: str | None = None, timestamp_ms: int | None = None) -> None:
    client = _default_client or init()
    client.log_image(name, image, format=format, timestamp_ms=timestamp_ms)


def flush() -> None:
    if _default_client is not None:
        _default_client.flush()

__all__ = [
    "AeroboardClient",
    "AeroboardLightningLogger",
    "AeroboardRayTuneLoggerCallback",
    "AeroboardLightningCallback",
    "AeroboardClientError",
    "FlightConnectionError",
    "get_default_client",
    "init",
    "log",
    "log_metrics",
    "log_hyperparameters",
    "log_image",
    "flush",
]