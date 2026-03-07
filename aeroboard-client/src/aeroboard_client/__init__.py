from .client import AeroboardClient
from .schema import scalar_schema, image_schema, distribution_schema

__all__ = [
    "init",
    "log",
    "flush"
]

_default_client = None

def init(run_id="default_run", url="grpc://localhost:50051", batch_size=100) -> AeroboardClient:
    """
    Initialize the Aeroboard globally.
    If multiple connections are needed, construct AeroboardClient directly.
    """
    global _default_client
    
    # Auto-detect frameork versions (e.g. PyTorch Lightning)
    framework_suffix = ""
    try:
        import pytorch_lightning as pl
        if f"pl{pl.__version__}" not in run_id:
            framework_suffix = f"-pl{pl.__version__}"
    except ImportError:
        pass
    
    actual_run_id = f"{run_id}{framework_suffix}"

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
