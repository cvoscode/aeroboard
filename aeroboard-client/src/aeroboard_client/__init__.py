from .client import AeroboardClient, AeroboardLightningCallback
from .schema import scalar_schema, image_schema, distribution_schema, hyperparameter_schema

__all__ = [
    "init",
    "log",
    "log_hyperparameters",
    "log_image",
    "flush",
    "get_default_client",
    "AeroboardClient",
    "AeroboardLightningCallback",
]

_default_client = None


def get_default_client():
    return _default_client

def init(run_id=None, url="grpc://localhost:50051", batch_size=100) -> AeroboardClient:
    """
    Initialize the Aeroboard globally.
    If multiple connections are needed, construct AeroboardClient directly.
    """
    global _default_client

    from .client import _normalize_run_id

    _default_client = AeroboardClient(run_id=_normalize_run_id(run_id), url=url, batch_size=batch_size)
    return _default_client

def log(metrics: dict):
    """
    Log metrics to the globally initialized Aeroboard.
    """
    global _default_client
    if _default_client is None:
        init()
    _default_client.log(metrics)

def log_hyperparameters(hyperparameters: dict, step: int | float | None = None):
    global _default_client
    if _default_client is None:
        init()
    _default_client.log_hyperparameters(hyperparameters, step=step)

def log_image(name: str, image, *, format: str | None = None, timestamp_ms: int | None = None):
    global _default_client
    if _default_client is None:
        init()
    _default_client.log_image(name, image, format=format, timestamp_ms=timestamp_ms)

def flush():
    """
    Force flush any buffered metrics.
    """
    global _default_client
    if _default_client is not None:
        _default_client.flush()
