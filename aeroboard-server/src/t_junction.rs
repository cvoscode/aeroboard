use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arrow::array::{Array, AsArray};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::telemetry::TelemetryBatch;

/// Outcome of persisting one batch, reported back to the ingesting request.
pub type PersistOutcome = Result<(), String>;

/// A batch queued for storage plus the channel its outcome is reported on.
pub struct PersistJob {
    pub batch: Arc<TelemetryBatch>,
    pub done: oneshot::Sender<PersistOutcome>,
}

/// T-Junction: the central hub that splits every ingested batch into
///
/// * **Path A (persistence)**: a bounded `mpsc` queue drained by exactly one
///   persistence loop. When storage falls behind, `publish` waits for space, so
///   backpressure flows back to the Flight client instead of batches being dropped.
/// * **Path B (live)**: a `broadcast` channel for WebSocket subscribers. Live viewers
///   are best-effort: a slow browser lags and is told to re-fetch history.
pub struct TJunction {
    live: broadcast::Sender<Arc<TelemetryBatch>>,
    /// `None` once persistence has been closed for shutdown.
    persist: Mutex<Option<mpsc::Sender<PersistJob>>>,
    live_run_ids: Mutex<BTreeSet<String>>,
}

#[derive(Debug)]
pub struct PersistenceClosed;

impl std::fmt::Display for PersistenceClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("persistence queue is closed (server shutting down)")
    }
}

impl std::error::Error for PersistenceClosed {}

impl TJunction {
    /// Returns the hub and the receiving end of the persistence queue.
    pub fn new(live_capacity: usize, persist_capacity: usize) -> (Self, mpsc::Receiver<PersistJob>) {
        let (live, _) = broadcast::channel(live_capacity.max(1));
        let (persist_tx, persist_rx) = mpsc::channel(persist_capacity.max(1));
        let hub = Self {
            live,
            persist: Mutex::new(Some(persist_tx)),
            live_run_ids: Mutex::new(BTreeSet::new()),
        };
        (hub, persist_rx)
    }

    /// Subscribe to the live stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TelemetryBatch>> {
        self.live.subscribe()
    }

    /// Fan a batch out to live subscribers and queue it for persistence.
    ///
    /// Waits while the persistence queue is full. The returned receiver resolves
    /// once the batch has been written (or failed to be), so ingestion can
    /// acknowledge uploads only after they are durable.
    pub async fn publish(&self, batch: Arc<TelemetryBatch>) -> Result<oneshot::Receiver<PersistOutcome>, PersistenceClosed> {
        self.register_batch_runs(&batch);
        // No live subscribers is normal (no dashboard open).
        let _ = self.live.send(batch.clone());

        let sender = self.persist.lock().expect("persist lock poisoned").clone();
        let Some(sender) = sender else {
            return Err(PersistenceClosed);
        };
        let (done, outcome) = oneshot::channel();
        sender.send(PersistJob { batch, done }).await.map_err(|_| PersistenceClosed)?;
        Ok(outcome)
    }

    /// Stop accepting batches for persistence; the loop drains what is queued and exits.
    pub fn close_persistence(&self) {
        self.persist.lock().expect("persist lock poisoned").take();
    }

    pub fn current_runs(&self) -> Vec<String> {
        self.live_run_ids.lock().expect("run id lock poisoned").iter().cloned().collect()
    }

    /// Pick a run ID with `choose` (given the live run IDs) and register it, all
    /// under one lock, so concurrent resolutions of the same base name diverge.
    pub fn reserve_run(&self, choose: impl FnOnce(&BTreeSet<String>) -> String) -> String {
        let mut known = self.live_run_ids.lock().expect("run id lock poisoned");
        let run_id = choose(&known);
        known.insert(run_id.clone());
        run_id
    }

    /// Clear all live run IDs (e.g., after deleting all persisted runs).
    pub fn clear_live_runs(&self) {
        self.live_run_ids.lock().expect("run id lock poisoned").clear();
    }

    fn register_batch_runs(&self, telemetry: &TelemetryBatch) {
        // Batches are validated before publishing, so column 1 is a Utf8 `run_id`.
        let run_id_array = telemetry.batch.column(1).as_string::<i32>();
        let mut known = self.live_run_ids.lock().expect("run id lock poisoned");
        for index in 0..run_id_array.len() {
            if !run_id_array.is_null(index) {
                let run_id = run_id_array.value(index);
                if !known.contains(run_id) {
                    known.insert(run_id.to_string());
                }
            }
        }
    }
}
