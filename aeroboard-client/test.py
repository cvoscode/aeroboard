import time
import math
from aeroboard_client import init, log, flush

def main():
    print("Initialize Aeroboard Client...")
    init("grpc://localhost:50051", batch_size=10) # Small batch size to see live effects
    
    print("Starting simulated training loop...")
    epochs = 100
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
                    "loss": loss,
                    "accuracy": accuracy,
                    "epoch": epoch,
                    "step": step,
                    "learning_rate": 0.01 * math.exp(-epoch / 10.0)
                })
                
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
