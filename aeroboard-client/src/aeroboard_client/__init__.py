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
    _normalize_dashboard_config,
)
from aeroboard_client._version import __version__

_default_client: AeroboardClient | None = None


def get_default_client() -> AeroboardClient | None:
    return _default_client


def init(
    run_id: str | None = None,
    url: str = "grpc://localhost:50051",
    api_base_url: str | None = None,
    batch_size: int = 100,
    dashboard: dict | None = None,
    *,
    project: str | None = None,
    resume: bool = False,
    tags: list[str] | None = None,
    metadata: dict | None = None,
    capture_provenance: bool = False,
    spool_dir: str | None = None,
    max_spool_bytes: int = 1024**3,
    segment: str | None = None,
) -> AeroboardClient:
    """Create the default client.

    Args:
        dashboard: Optional initial dashboard view for this run, e.g.
            ``{"starred": ["loss"], "targets": {"val/accuracy": 0.95}, "order": [...]}``.
            See :meth:`AeroboardClient.configure_dashboard`.
    """
    global _default_client
    # Validate before connecting so a typo like "stared" fails fast with a clear message.
    dashboard_config = _normalize_dashboard_config(dashboard) if dashboard is not None else None
    _default_client = AeroboardClient(
        run_id=run_id,
        url=url,
        api_base_url=api_base_url,
        batch_size=batch_size,
        project=project, resume=resume, tags=tags, metadata=metadata, capture_provenance=capture_provenance,
        spool_dir=spool_dir, max_spool_bytes=max_spool_bytes,
        segment=segment,
    )
    if dashboard_config is not None:
        _default_client.configure_dashboard(**dashboard_config)
    return _default_client


def log(metrics: dict[str, float], step: int | float | None = None) -> None:
    client = _default_client or init()
    client.log(metrics, step=step)


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


def log_image(name: str, image, *, format: str | None = None, timestamp_ms: int | None = None,
              step: float | None = None, segment: str | None = None) -> str | None:
    client = _default_client or init()
    return client.log_image(name, image, format=format, timestamp_ms=timestamp_ms, step=step, segment=segment)


def log_audio(
    name: str,
    audio,
    *,
    sample_rate: int | None = None,
    format: str | None = None,
    timestamp_ms: int | None = None,
    step: float | None = None,
    segment: str | None = None,
) -> str | None:
    client = _default_client or init()
    return client.log_audio(name, audio, sample_rate=sample_rate, format=format, timestamp_ms=timestamp_ms, step=step, segment=segment)


def log_video(name: str, video, *, format: str | None = None, timestamp_ms: int | None = None,
              step: float | None = None, segment: str | None = None) -> str | None:
    client = _default_client or init()
    return client.log_video(name, video, format=format, timestamp_ms=timestamp_ms, step=step, segment=segment)


def configure_dashboard(
    *,
    starred: list[str] | None = None,
    targets: dict[str, float] | None = None,
    order: list[str] | None = None,
) -> None:
    client = _default_client or init()
    client.configure_dashboard(starred=starred, targets=targets, order=order)


def flush(*, local_only: bool = False) -> bool:
    if _default_client is not None:
        return _default_client.flush(local_only=local_only)
    return True


def define_metric(name: str, *, goal=None, target=None, unit=None) -> None:
    (_default_client or init()).define_metric(name, goal=goal, target=target, unit=unit)


def log_event(label: str, **kwargs) -> str:
    return (_default_client or init()).log_event(label, **kwargs)


def finish(status: str = "completed", *, error: str | None = None) -> None:
    if _default_client is not None:
        _default_client.finish(status, error=error)

__all__ = [
    "__version__",
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
    "log_audio",
    "log_video",
    "configure_dashboard",
    "flush",
    "define_metric",
    "log_event",
    "finish",
]
