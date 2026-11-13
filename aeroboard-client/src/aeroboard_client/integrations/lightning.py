from __future__ import annotations
import warnings
import re
from typing import Any, Dict, List, Optional
from ..client import AeroboardClient, _mapping_like, _coerce_scalar_metric_value, _normalize_hparam_value
def _resolve_lightning_integration() -> tuple[type[Any], type[Any], Any, bool]:
    try:
        from lightning.pytorch import Callback as lightning_callback_base
        from lightning.pytorch.loggers.logger import Logger as lightning_logger_base
        from lightning.pytorch.utilities.rank_zero import rank_zero_only
        return lightning_callback_base, lightning_logger_base, rank_zero_only, True
    except ImportError:
        try:
            from pytorch_lightning import Callback as lightning_callback_base
            from pytorch_lightning.loggers.logger import Logger as lightning_logger_base
            from pytorch_lightning.utilities.rank_zero import rank_zero_only
            return lightning_callback_base, lightning_logger_base, rank_zero_only, True
        except ImportError:
            def passthrough(func):
                return func

            return object, object, passthrough, False

_LightningCallbackBase, _LightningLoggerBase, _rank_zero_only, _LIGHTNING_AVAILABLE = _resolve_lightning_integration()

class AeroboardLightningCallback:
    """
    PyTorch Lightning callback for automatic telemetry logging.

    This callback automatically logs:
    - Hyperparameters on fit start
    - Training metrics (loss, accuracy) on each batch end
    - Validation metrics on validation epoch end

    Example:
        >>> from lightning.pytorch import LightningModule, Trainer
        >>> callback = AeroboardLightningCallback()
        >>> trainer = Trainer(callbacks=[callback])
        >>> trainer.fit(model)
    """

    def __init__(
        self,
        run_id: Optional[str] = None,
        url: str = "grpc://localhost:50051",
        batch_size: int = 100,
        client: Optional[AeroboardClient] = None,
    ):
        """
        Initialize the Lightning callback.

        Args:
            run_id: Run name for telemetry.
            url: Flight server URL.
            batch_size: Telemetry batch size.
            client: Pre-created AeroboardClient instance.
        """
        if not _LIGHTNING_AVAILABLE:
            raise RuntimeError("lightning is required for AeroboardLightningCallback")

        warnings.warn("AeroboardLightningCallback is deprecated; use AeroboardLightningLogger to forward named metrics.",DeprecationWarning,stacklevel=2)
        # Create client if not provided
        self.client = client or AeroboardClient(run_id=run_id, url=url, batch_size=batch_size)

        # Create internal callback class
        class _Callback(_LightningCallbackBase):
            def __init__(self, outer: "AeroboardLightningCallback"):
                super().__init__()
                self._outer = outer
                self._hparams_logged = False

            def _client(self) -> AeroboardClient:
                return self._outer.client

            def _maybe_log_hparams(self, trainer, pl_module):
                """Log hyperparameters if not already logged."""
                if self._hparams_logged:
                    return
                combined = {}

                # Get module hyperparameters
                module_hparams = getattr(pl_module, "hparams", None)
                if module_hparams is not None:
                    combined.update(_mapping_like(module_hparams))

                # Get trainer logger hyperparameters
                if getattr(trainer, "logger", None) is not None:
                    maybe_hparams = getattr(trainer.logger, "hparams", None)
                    if maybe_hparams:
                        combined.update(_mapping_like(maybe_hparams))

                if combined:
                    self._client().log_hyperparameters(combined)
                    self._hparams_logged = True

            def on_fit_start(self, trainer, pl_module):
                """Log hyperparameters at fit start."""
                self._maybe_log_hparams(trainer, pl_module)

            def on_train_batch_end(self, trainer, pl_module, outputs, batch, batch_idx):
                """Log metrics after each training batch."""
                self._maybe_log_hparams(trainer, pl_module)
                payload: Dict[str, float] = {"step": float(trainer.global_step)}
                loss = outputs.get("loss") if isinstance(outputs, dict) else None
                if loss is not None:
                    payload["train/loss"] = float(loss.detach().item() if hasattr(loss, "detach") else loss)
                train_acc = trainer.callback_metrics.get("train_acc")
                if train_acc is not None:
                    payload["train/accuracy"] = float(train_acc.detach().item() if hasattr(train_acc, "detach") else train_acc)
                self._client().log(payload)

            def on_validation_epoch_end(self, trainer, pl_module):
                """Log validation metrics at epoch end."""
                self._maybe_log_hparams(trainer, pl_module)
                metrics = trainer.callback_metrics
                payload: Dict[str, float] = {"step": float(trainer.global_step)}
                if "val_loss" in metrics:
                    value = metrics["val_loss"]
                    payload["val/loss"] = float(value.detach().item() if hasattr(value, "detach") else value)
                if "val_acc" in metrics:
                    value = metrics["val_acc"]
                    payload["val/accuracy"] = float(value.detach().item() if hasattr(value, "detach") else value)
                if len(payload) > 1:
                    self._client().log(payload)
                    self._client().flush()

            def on_train_end(self, trainer, pl_module):
                """Flush remaining data at training end."""
                self._client().flush()

            @_rank_zero_only
            def on_fit_end(self, trainer, pl_module):
                self._client().finish("completed")

            @_rank_zero_only
            def on_exception(self, trainer, pl_module, exception):
                self._client().finish("cancelled" if isinstance(exception,KeyboardInterrupt) else "failed",error=str(exception))

        self.callback = _Callback(self)

    def __getattr__(self, item: str) -> Any:
        """Delegate attribute access to the internal callback."""
        return getattr(self.callback, item)


class AeroboardLightningLogger(_LightningLoggerBase):
    """
    Lightning-compatible logger for Aeroboard telemetry.

    This logger forwards metrics emitted by `LightningModule.log(...)` and
    hyperparameters provided through Lightning's logger interface.

    Example:
        >>> from lightning.pytorch import Trainer
        >>> logger = AeroboardLightningLogger(run_id="lightning-demo")
        >>> trainer = Trainer(logger=logger)
        >>> trainer.fit(model)
    """

    def __init__(
        self,
        run_id: Optional[str] = None,
        url: str = "grpc://localhost:50051",
        batch_size: int = 100,
        client: Optional[AeroboardClient] = None,
    ):
        if not _LIGHTNING_AVAILABLE:
            raise RuntimeError("lightning is required for AeroboardLightningLogger")

        super().__init__()
        self.client = client or AeroboardClient(run_id=run_id, url=url, batch_size=batch_size)
        self._name = "aeroboard"
        self._version = self.client.run_id
        self._hparams: Dict[str, Any] = {}
        self._last_step: Optional[float] = None

    @property
    def name(self) -> str:
        return self._name

    @property
    def version(self) -> str:
        return self._version

    @property
    def experiment(self) -> AeroboardClient:
        return self.client

    @property
    def hparams(self) -> Dict[str, Any]:
        return dict(self._hparams)

    @_rank_zero_only
    def log_hyperparams(self, params: Any) -> None:
        values = _mapping_like(params)
        if not values:
            return
        self._hparams.update(values)
        self.client.log_hyperparameters(values)

    @_rank_zero_only
    def log_metrics(self, metrics: Dict[str, Any], step: Optional[int] = None) -> None:
        payload: Dict[str, Any] = {key: value for key, value in metrics.items() if key != "step"}

        resolved_step = _coerce_scalar_metric_value(step)
        if resolved_step is None:
            resolved_step = self._last_step
        if resolved_step is None and "epoch" in payload:
            resolved_step = _coerce_scalar_metric_value(payload["epoch"])

        if resolved_step is not None:
            self._last_step = resolved_step

        self.client.log_metrics(payload, step=resolved_step)

    @_rank_zero_only
    def save(self) -> None:
        self.client.flush()

    @_rank_zero_only
    def finalize(self, status: str) -> None:
        self.client.finish("completed" if status in ("success", "finished", "completed") else "cancelled" if status in ("interrupted","cancelled") else "failed")

