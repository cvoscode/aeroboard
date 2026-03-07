use std::sync::Arc;
use tokio::sync::broadcast;
use arrow::record_batch::RecordBatch;

/// T-Junction: The central hub for distributing incoming Flight batches
/// to the WebSocket streamers and the Database persistence layer.
#[derive(Clone)]
pub struct TJunction {
    /// Broadcast channel where producers dump RecordBatches.
    /// Clones of RecordBatch are cheap (Arc-backed arrays).
    pub sender: broadcast::Sender<Arc<RecordBatch>>,
}

impl TJunction {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to the live stream
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<RecordBatch>> {
        self.sender.subscribe()
    }

    /// Broadcast a batch
    pub fn publish(&self, batch: Arc<RecordBatch>) -> Result<usize, broadcast::error::SendError<Arc<RecordBatch>>> {
        self.sender.send(batch)
    }
}
