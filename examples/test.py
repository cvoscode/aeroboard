import time
import math
from aeroboard_client import init, log, log_hyperparameters, log_image, flush

def main():
    print("Initialize Aeroboard Client...")
    init(run_id="simulated_compare_demo", url="grpc://localhost:50051", batch_size=10) # Small batch size to see live effects
    log_hyperparameters({
        "learning_rate": 0.01,
        "dropout": 0.15,
        "scheduler": "cosine",
        "batch_size": 16,
    })
    
    print("Starting simulated training loop...")
    epochs = 30
    steps_per_epoch = 10
    
    try:
        for epoch in range(epochs):
            for step in range(steps_per_epoch):
                global_step = epoch * steps_per_epoch + step
                # Simulate some metrics decaying / oscillating over time
                loss = math.exp(-global_step / 20.0) + 0.1 * math.sin(global_step / 2.0)
                accuracy = 1.0 - math.exp(-global_step / 30.0)
                
                # Log metrics
                log({
                    "train/loss": loss,
                    "train/accuracy": accuracy,
                    "epoch": epoch,
                    "step": global_step,
                    "optimizer/lr": 0.01 * math.exp(-epoch / 10.0)
                })

                if step in {0, steps_per_epoch // 2, steps_per_epoch - 1}:
                    shade = int(255 * min(1.0, accuracy))
                    image_bytes = bytes([137, 80, 78, 71, 13, 10, 26, 10]) + bytes([shade]) * 32
                    log_image("samples/qualitative", image_bytes, format="png")
                
                # Sleep a tiny bit to simulate computation
                time.sleep(0.05)
                
            print(f"Epoch {epoch} finished: loss={loss:.4f}, accuracy={accuracy:.4f}")

    except KeyboardInterrupt:
        print("Training interrupted")
    finally:
        flush()
        print("Final batch flushed.")

if __name__ == "__main__":
    main()
