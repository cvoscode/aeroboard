import torch
import torch.nn as nn
import torch.optim as optim
import aeroboard_client
import time

# 1. Initialize Aeroboard Client
print("Initializing Aeroboard Client...")
aeroboard = aeroboard_client.init("grpc://localhost:50051", batch_size=20)

# 2. Setup a Simple PyTorch Model (MLP)
class SimpleMLP(nn.Module):
    def __init__(self, input_size, hidden_size, num_classes):
        super(SimpleMLP, self).__init__()
        self.fc1 = nn.Linear(input_size, hidden_size)
        self.relu = nn.ReLU()
        self.fc2 = nn.Linear(hidden_size, num_classes)
        
    def forward(self, x):
        out = self.fc1(x)
        out = self.relu(out)
        out = self.fc2(out)
        return out

input_size = 10
hidden_size = 32
num_classes = 2

model = SimpleMLP(input_size, hidden_size, num_classes)
criterion = nn.CrossEntropyLoss()
optimizer = optim.Adam(model.parameters(), lr=0.01)

# 3. Generate Synthetic Data
batch_size = 64
num_batches = 100
epochs = 50

print(f"Training on synthetic data for {epochs} epochs...")
global_step = 0

for epoch in range(epochs):
    epoch_loss = 0.0
    correct = 0
    total = 0
    
    for i in range(num_batches):
        # Generate random data and binary labels
        inputs = torch.randn(batch_size, input_size)
        # Add a slight pattern so the model actually learns something
        labels = (inputs[:, 0] > 0).long()
        
        # Forward pass
        outputs = model(inputs)
        loss = criterion(outputs, labels)
        
        # Backward and optimize
        optimizer.zero_grad()
        loss.backward()
        optimizer.step()
        
        # Calculate accuracy
        _, predicted = torch.max(outputs.data, 1)
        total += labels.size(0)
        correct += (predicted == labels).sum().item()
        
        batch_acc = (predicted == labels).sum().item() / labels.size(0)
        
        # Log metrics to Aeroboard every step
        aeroboard_client.log({
            "train_loss": loss.item(),
            "train_accuracy": batch_acc,
            "epoch": epoch,
            "global_step": global_step,
            "learning_rate": optimizer.param_groups[0]['lr']
        })
        
        global_step += 1
        
        # Simulate slight computation delay to see it animating smoothly in the dashboard
        time.sleep(0.02)
        
    # Epoch summary
    epoch_acc = correct / total
    print(f'Epoch [{epoch+1}/{epochs}], Loss: {loss.item():.4f}, Accuracy: {epoch_acc:.4f}')

# Ensure all remaining buffered metrics are sent at the end
aeroboard_client.flush()
print("Training complete! Flushed all data to Aeroboard.")
