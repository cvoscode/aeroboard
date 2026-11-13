from __future__ import annotations
import warnings
import re
from typing import Any, Dict, List, Optional
from ..client import AeroboardClient, _mapping_like, _coerce_scalar_metric_value, _normalize_hparam_value
def _resolve_ray_tune_integration() -> tuple[type[Any], bool]:
    try:
        from ray.tune.logger import LoggerCallback as ray_logger_callback_base
        return ray_logger_callback_base, True
    except ImportError:
        return object, False

_RayTuneLoggerCallbackBase, _RAY_TUNE_AVAILABLE = _resolve_ray_tune_integration()

class AeroboardRayTuneLoggerCallback(_RayTuneLoggerCallbackBase):
    """
    Ray Tune callback that creates one Aeroboard run per trial.

    Trial configuration is logged as hyperparameters, and numeric trial results are
    streamed as scalar metrics to the matching Aeroboard run.
    """

    def __init__(
        self,
        *,
        url: str = "grpc://localhost:50051",
        api_base_url: Optional[str] = None,
        batch_size: int = 100,
        run_prefix: Optional[str] = None,
        metric_prefix: Optional[str] = None,
    ):
        if not _RAY_TUNE_AVAILABLE:
            raise RuntimeError("ray[tune] is required for AeroboardRayTuneLoggerCallback")

        super().__init__()
        self.url = url
        self.api_base_url = api_base_url
        self.batch_size = batch_size
        self.run_prefix = run_prefix
        self.metric_prefix = metric_prefix
        self._clients: Dict[str, AeroboardClient] = {}

    def _trial_key(self, trial: Any) -> str:
        return str(getattr(trial, "trial_id", id(trial)))

    def _trial_run_id(self, trial: Any) -> str:
        trial_name = (
            getattr(trial, "trial_name", None)
            or getattr(trial, "experiment_tag", None)
            or getattr(trial, "trial_id", None)
            or "trial"
        )
        if self.run_prefix:
            return f"{self.run_prefix}-{trial_name}"
        return str(trial_name)

    def _build_trial_hparams(self, trial: Any) -> Dict[str, Any]:
        payload = _mapping_like(getattr(trial, "config", {}) or {})
        trial_id = getattr(trial, "trial_id", None)
        trial_name = getattr(trial, "trial_name", None)
        experiment_tag = getattr(trial, "experiment_tag", None)
        if trial_id is not None:
            payload["ray/trial_id"] = str(trial_id)
        if trial_name is not None:
            payload["ray/trial_name"] = str(trial_name)
        if experiment_tag is not None:
            payload["ray/experiment_tag"] = str(experiment_tag)
        return payload

    def _get_client(self, trial: Any) -> AeroboardClient:
        trial_key = self._trial_key(trial)
        if trial_key in self._clients:
            return self._clients[trial_key]

        client = AeroboardClient(
            run_id=self._trial_run_id(trial),
            url=self.url,
            api_base_url=self.api_base_url,
            batch_size=self.batch_size,
        )
        hparams = self._build_trial_hparams(trial)
        if hparams:
            client.log_hyperparameters(hparams)
        self._clients[trial_key] = client
        return client

    def log_trial_start(self, trial: Any) -> None:
        self._get_client(trial)

    def log_trial_result(self, iteration: int, trial: Any, result: Dict[str, Any]) -> None:
        client = self._get_client(trial)
        step = _coerce_scalar_metric_value(result.get("training_iteration"))
        if step is None:
            step = _coerce_scalar_metric_value(iteration)

        filtered = {
            key: value
            for key, value in result.items()
            if key not in {
                "config",
                "date",
                "done",
                "experiment_id",
                "hostname",
                "node_ip",
                "pid",
                "timestamp",
                "time_since_restore",
                "timesteps_since_restore",
                "iterations_since_restore",
                "training_iteration",
                "trial_id",
                "should_checkpoint",
            }
        }
        client.log_metrics(filtered, step=step, prefix=self.metric_prefix)

    def log_trial_end(self, trial: Any, failed: bool = False) -> None:
        client = self._clients.pop(self._trial_key(trial), None)
        if client is None:
            return
        client.log_hyperparameters({"ray/failed": failed})
        client.finish("failed" if failed else "completed")

    def on_experiment_end(self, trials: List[Any], **info: Any) -> None:
        del trials, info
        for client in list(self._clients.values()):
            client.flush()
            client.close()
        self._clients.clear()

