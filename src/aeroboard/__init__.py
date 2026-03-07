from .client import AeroboardClient
from .schema import scalar_schema, image_schema, distribution_schema

__all__ = [
    "init",
    "log",
    "flush",
    "AeroboardClient"
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
    
    from .client import _normalize_run_id, _with_lightning_suffix

    actual_run_id = _with_lightning_suffix(_normalize_run_id(run_id))

    _default_client = AeroboardClient(run_id=actual_run_id, url=url, batch_size=batch_size)
    return _default_client

def log(metrics: dict):
    """
    Log metrics to the globally initialized Aeroboard.
    """
    global _default_client
    if _default_client is None:
        init()
    _default_client.log(metrics)

def flush():
    """
    Force flush any buffered metrics.
    """
    global _default_client
    if _default_client is not None:
        _default_client.flush()
