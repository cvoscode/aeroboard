"""Aeroboard Lightning example for comparing storage backends.

Typical usage with the new uv-based workflow:

Filesystem:
    uv run aeroboard start --storage filesystem --data-dir .aeroboard/examples/fs
    uv run examples/lightning_example.py --backend filesystem

Greptime:
    uv run aeroboard db
    uv run aeroboard start --storage greptime
    uv run examples/lightning_example.py --backend greptime
"""

import argparse
import io
import time

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import lightning.pytorch as pl
import torch
import torch.nn as nn
import torch.nn.functional as F
import torch.optim as optim
from torch.utils.data import DataLoader, TensorDataset
import aeroboard_client as aeroboard
from _config import get_example_api_base_url, get_example_flight_url


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
        self.log('train/loss', loss)
        self.log('train/acc', acc)
        return loss

    def validation_step(self, batch, batch_idx):
        x, y = batch
        y_hat = self(x)
        loss = F.cross_entropy(y_hat, y)
        acc = (y_hat.argmax(dim=1) == y).float().mean()
        self.log('val/loss', loss)
        self.log('val/acc', acc)
        return loss

    def configure_optimizers(self):
        return optim.Adam(self.parameters(), lr=self.hparams.learning_rate)

def run_lightning_example(
    run_name: str,
    *,
    backend: str,
    url: str,
    api_base_url: str,
    batch_size: int,
    max_epochs: int,
):
    X = torch.randn(1000, 10)
    Y = (X[:, 0] > 0).long()
    dataset = TensorDataset(X, Y)
    train_loader = DataLoader(dataset, batch_size=32)
    val_loader = DataLoader(dataset, batch_size=32)

    model = SimpleModel(hidden_size=8 if 'high' in run_name else 5, learning_rate=0.05 if 'high' in run_name else 0.02)

    client = aeroboard.init(
        run_id=run_name,
        url=url,
        api_base_url=api_base_url,
        batch_size=batch_size,
    )
    lightning_logger = client.lightning_logger()

    client.log_hyperparameters({
        'manual/tag': 'lightning',
        'augmentation': 'gaussian-noise',
        'storage/backend': backend,
        'example/workflow': 'uv',
        'example/flight_url': url,
        'example/api_base_url': api_base_url,
        'example/run_name': run_name,
    })
    gradient_cb = GradientFigureCallback(client=client)

    trainer = pl.Trainer(
        max_epochs=max_epochs,
        callbacks=[gradient_cb],
        enable_progress_bar=True,
        enable_checkpointing=False,
        logger=lightning_logger,
    )

    print(f"🚀 Starting {run_name} with Aeroboard Logic...")
    trainer.fit(model, train_loader, val_loader)
    client.flush()
    print(f"✅ {run_name} Complete.")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the Aeroboard Lightning example against a selected storage backend.",
    )
    parser.add_argument(
        "--backend",
        choices=["filesystem", "greptime"],
        default="filesystem",
        help="Storage backend currently used by the Aeroboard server.",
    )
    parser.add_argument(
        "--url",
        default=get_example_flight_url(),
        help="Arrow Flight endpoint used by the Python client.",
    )
    parser.add_argument(
        "--api-base-url",
        default=get_example_api_base_url(),
        help="HTTP API base used for run resolution and history endpoints.",
    )
    parser.add_argument(
        "--run-prefix",
        default="lightning-eval",
        help="Prefix used for generated run names.",
    )
    parser.add_argument(
        "--variants",
        nargs="+",
        choices=["baseline", "high-lr"],
        default=["baseline", "high-lr"],
        help="Which training variants to execute for side-by-side evaluation.",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=10,
        help="Aeroboard client batch size.",
    )
    parser.add_argument(
        "--max-epochs",
        type=int,
        default=20,
        help="Number of Lightning epochs to train per run.",
    )
    parser.add_argument(
        "--sleep-between",
        type=float,
        default=2.0,
        help="Delay between sequential runs so timelines separate cleanly.",
    )
    return parser.parse_args()


def build_run_names(run_prefix: str, backend: str, variants: list[str]) -> list[str]:
    names: list[str] = []
    for variant in variants:
        suffix = "lr-high" if variant == "high-lr" else variant
        names.append(f"{run_prefix}-{backend}-{suffix}")
    return names


if __name__ == "__main__":
    args = parse_args()

    pl.seed_everything(7, workers=True)

    print("🔎 Aeroboard backend evaluation")
    print(f"   backend: {args.backend}")
    print(f"   flight:  {args.url}")
    print(f"   api:     {args.api_base_url}")
    print(f"   variants: {', '.join(args.variants)}")

    run_names = build_run_names(args.run_prefix, args.backend, args.variants)
    for index, run_name in enumerate(run_names):
        run_lightning_example(
            run_name,
            backend=args.backend,
            url=args.url,
            api_base_url=args.api_base_url,
            batch_size=args.batch_size,
            max_epochs=args.max_epochs,
        )
        if index < len(run_names) - 1 and args.sleep_between > 0:
            time.sleep(args.sleep_between)
