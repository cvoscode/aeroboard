import torch
import torch.nn as nn
import torch.nn.functional as F
import torch.optim as optim
from torch.utils.data import DataLoader, TensorDataset
import pytorch_lightning as pl
import aeroboard_client
import time

# Get Lightning version for run metadata
_PL_VERSION = pl.__version__

class AeroboardLogger(pl.Callback):
    def __init__(self, url="grpc://localhost:50051", batch_size=20):
        super().__init__()
        if aeroboard_client._default_client is None:
            aeroboard_client.init(url=url, batch_size=batch_size)
    

    def on_train_batch_end(self, trainer, pl_module, outputs, batch, batch_idx):
        loss = outputs.get('loss')
        acc = trainer.callback_metrics.get('train_acc')
        
        # Explicitly log "step" so the frontend system can find it
        log_data = {"step": float(trainer.global_step)}
        if loss is not None:
            log_data["train/loss"] = float(loss.item())
        if acc is not None:
            log_data["train/accuracy"] = float(acc.item())
            
        aeroboard_client.log(log_data)

    def on_validation_epoch_end(self, trainer, pl_module):
        metrics = trainer.callback_metrics
        # Also include step for validation so alignment works perfectly
        log_payload = {"step": float(trainer.global_step)}
        if 'val_loss' in metrics:
            log_payload["val/loss"] = float(metrics['val_loss'].item())
        if 'val_acc' in metrics:
            log_payload["val/accuracy"] = float(metrics['val_acc'].item())
        
        if len(log_payload) > 1:
            aeroboard_client.log(log_payload)
            aeroboard_client.flush()

# 2. Lightning Module
class SimpleModel(pl.LightningModule):
    def __init__(self):
        super().__init__()
        self.l1 = nn.Linear(10, 5)
        self.l2 = nn.Linear(5, 2)

    def forward(self, x):
        return self.l2(F.relu(self.l1(x)))

    def training_step(self, batch, batch_idx):
        x, y = batch
        y_hat = self(x)
        loss = F.cross_entropy(y_hat, y)
        acc = (y_hat.argmax(dim=1) == y).float().mean()
        self.log('train_loss', loss)
        self.log('train_acc', acc)
        return loss

    def validation_step(self, batch, batch_idx):
        x, y = batch
        y_hat = self(x)
        loss = F.cross_entropy(y_hat, y)
        acc = (y_hat.argmax(dim=1) == y).float().mean()
        self.log('val_loss', loss)
        self.log('val_acc', acc)
        return loss

    def configure_optimizers(self):
        return optim.Adam(self.parameters(), lr=0.02)

# 3. Data & Training Execution
def run_lightning_example(run_name="experiment_A"):
    # Create synthetic data
    X = torch.randn(1000, 10)
    Y = (X[:, 0] > 0).long()
    dataset    = TensorDataset(X, Y)
    train_loader = DataLoader(dataset, batch_size=32)
    val_loader   = DataLoader(dataset, batch_size=32)

    model = SimpleModel()

    # init() now automatically resolves versioning and detects framework info (like Lightning)
    aeroboard_client.init(run_id=run_name, batch_size=10)
    logger_cb = AeroboardLogger(url="grpc://localhost:50051", batch_size=10)

    trainer = pl.Trainer(
        max_epochs=5,
        callbacks=[logger_cb],
        enable_progress_bar=True,
        enable_checkpointing=False,
        logger=False
    )

    print(f"🚀 Starting {run_name} with Aeroboard Logic...")
    trainer.fit(model, train_loader, val_loader)
    print(f"✅ {run_name} Complete.")

if __name__ == "__main__":
    # Simulate two different experiments
    # If run twice, Aeroboard will now automatically version them: baseline, baseline-v2, etc.
    run_lightning_example("resnet_baseline")
    time.sleep(2)
    run_lightning_example("resnet_lr_high")
