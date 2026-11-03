"""Two seed runs with objectives, linked media, provenance, and offline capture.

Start Aeroboard, then run: uv run examples/workspace_example.py
"""
import math
import time

from PIL import Image
from aeroboard_client import AeroboardClient
from _config import get_example_api_base_url, get_example_flight_url


def main():
    for seed in (1, 2):
        with AeroboardClient("classifier",project="examples",tags=["demo"],
            metadata={"experiment":"classifier-v1","dataset":"synthetic"},
            capture_provenance=True,spool_dir="aeroboard-spool",
            url=get_example_flight_url(),api_base_url=get_example_api_base_url()) as run:
            run.configure_dashboard(starred=["loss"],order=["loss"],targets={"val/loss":0.1})
            run.define_metric("val/loss",goal="minimize",target=0.1)
            run.log_hyperparameters({"seed":seed,"learning_rate":0.001})
            for step in range(40):
                run.log({"val/loss":math.exp(-step/(10+seed))},step=step)
                if step==20:
                    asset=run.log_image("prediction",Image.new("RGB",(96,96),(seed*80,100,180)),step=step)
                    run.log_event("Checkpoint preview",kind="checkpoint",step=step,
                        details={"path":f"checkpoints/seed-{seed}.pt"},media_ids=[asset] if asset else [])
                time.sleep(0.02)
            print("Resume ID:",run.run_uid)


if __name__=="__main__":
    main()
