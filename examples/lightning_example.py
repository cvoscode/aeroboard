import io
import time

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import torch
import torch.nn as nn
import torch.nn.functional as F
import torch.optim as optim
from torch.utils.data import DataLoader, TensorDataset
import pytorch_lightning as pl
import aeroboard


class GradientFigureCallback(pl.Callback):
    def __init__(self, client: aeroboard.AeroboardClient, image_name: str = 'diagnostics/gradient-norms'):
        super().__init__()
        self.client = client
        self.image_name = image_name
        self._latest_gradients: dict[str, float] = {}

    def on_after_backward(self, trainer, pl_module) -> None:
        gradient_norms: dict[str, float] = {}
        for name, parameter in pl_module.named_parameters():
            if parameter.grad is None:
                continue
            gradient_norms[name] = float(parameter.grad.detach().norm().item())
        if gradient_norms:
            self._latest_gradients = gradient_norms

    def on_train_epoch_end(self, trainer, pl_module) -> None:
        if not self._latest_gradients:
            return

        names = list(self._latest_gradients.keys())
        values = [self._latest_gradients[name] for name in names]

        fig, ax = plt.subplots(figsize=(8, 4.5))
        ax.bar(range(len(names)), values, color='#00d2ff', edgecolor='#0f1115')
        ax.set_title(f'Gradient norms — epoch {trainer.current_epoch}')
        ax.set_ylabel('L2 norm')
        ax.set_xlabel('Parameter')
        ax.set_xticks(range(len(names)))
        ax.set_xticklabels(names, rotation=35, ha='right', fontsize=8)
        ax.grid(axis='y', alpha=0.25, linestyle='--')
        fig.tight_layout()

        buffer = io.BytesIO()
        fig.savefig(buffer, format='png', dpi=140, bbox_inches='tight')
        plt.close(fig)
        buffer.seek(0)

        self.client.log_image(self.image_name, buffer.getvalue(), format='png')
        self.client.flush()

# 2. Lightning Module
class SimpleModel(pl.LightningModule):
    def __init__(self, hidden_size: int = 5, learning_rate: float = 0.02):
        super().__init__()
        self.save_hyperparameters()
        self.l1 = nn.Linear(10, hidden_size)
        self.l2 = nn.Linear(hidden_size, 2)

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
        return optim.Adam(self.parameters(), lr=self.hparams.learning_rate)

# 3. Data & Training Execution
def run_lightning_example(run_name="experiment_A"):
    # Create synthetic data
    X = torch.randn(1000, 10)
    Y = (X[:, 0] > 0).long()
    dataset    = TensorDataset(X, Y)
    train_loader = DataLoader(dataset, batch_size=32)
    val_loader   = DataLoader(dataset, batch_size=32)

    model = SimpleModel(hidden_size=8 if 'high' in run_name else 5, learning_rate=0.05 if 'high' in run_name else 0.02)

    client = aeroboard.init(run_id=run_name, url="grpc://localhost:50051", batch_size=10)
    logger_cb = client.attach_lightning()

    client.log_hyperparameters({
        'manual/tag': 'lightning',
        'augmentation': 'gaussian-noise',
    })
    gradient_cb = GradientFigureCallback(client=client)

    trainer = pl.Trainer(
        max_epochs=20,
        callbacks=[logger_cb, gradient_cb],
        enable_progress_bar=True,
        enable_checkpointing=False,
        logger=False
    )

    print(f"🚀 Starting {run_name} with Aeroboard Logic...")
    trainer.fit(model, train_loader, val_loader)
    client.flush()
    print(f"✅ {run_name} Complete.")

if __name__ == "__main__":
    # Simulate two different experiments
    # If run twice, Aeroboard will now automatically version them: baseline, baseline-v2, etc.
    run_lightning_example("resnet_baseline")
    time.sleep(2)
    run_lightning_example("resnet_lr_high")
