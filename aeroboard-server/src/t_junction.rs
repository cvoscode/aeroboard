use std::sync::Arc;
use std::collections::BTreeSet;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use arrow::array::{Array, AsArray};
use arrow::datatypes::DataType;

use crate::telemetry::TelemetryBatch;

/// T-Junction: The central hub for distributing incoming Flight batches
/// to the WebSocket streamers and the Database persistence layer.
#[derive(Clone)]
pub struct TJunction {
    /// Broadcast channel where producers dump RecordBatches.
    /// Clones of RecordBatch are cheap (Arc-backed arrays).
    pub sender: broadcast::Sender<Arc<TelemetryBatch>>,
    live_run_ids: Arc<RwLock<BTreeSet<String>>>,
}

impl TJunction {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            live_run_ids: Arc::new(RwLock::new(BTreeSet::new())),
        }
    }

    /// Subscribe to the live stream
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TelemetryBatch>> {
        self.sender.subscribe()
    }

    /// Broadcast a batch
    pub fn publish(&self, batch: Arc<TelemetryBatch>) -> Result<usize, broadcast::error::SendError<Arc<TelemetryBatch>>> {
        self.register_batch_runs(&batch);
        self.sender.send(batch)
    }

    pub async fn current_runs(&self) -> Vec<String> {
        self.live_run_ids.read().await.iter().cloned().collect()
    }

    /// Clear all live run IDs (e.g., after deleting all persisted runs).
    pub async fn clear_live_runs(&self) {
        self.live_run_ids.write().await.clear();
    }

    fn register_batch_runs(&self, telemetry: &TelemetryBatch) {
        let batch = &telemetry.batch;
        let Some(run_id_index) = batch.schema().fields().iter().position(|field| field.name() == "run_id") else {
            return;
        };

        if batch.schema().field(run_id_index).data_type() != &DataType::Utf8 {
            return;
        }

        let run_id_array = batch.column(run_id_index).as_string::<i32>();

        let run_ids: Vec<String> = (0..batch.num_rows())
            .filter(|&index| !run_id_array.is_null(index))
            .map(|index| run_id_array.value(index).to_string())
            .collect();

        if run_ids.is_empty() {
            return;
        }

        let live_run_ids = self.live_run_ids.clone();
        // If spawn fails (runtime at capacity), the run IDs are silently dropped for
        // this batch — prefer live streaming over crashing the task. Run IDs will be
        // recovered from subsequent batches or on restart.
        let handle = tokio::spawn(async move {
            let mut known = live_run_ids.write().await;
            known.extend(run_ids);
        });
        // Explicitly drop to suppress "unused future" lint. The spawn is fire-and-forget;
        // we accept that a full runtime queue means run IDs for this batch are dropped.
        std::mem::drop(handle);
    }
}
