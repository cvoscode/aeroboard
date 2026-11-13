use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

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
    pub fn new(
        live_capacity: usize,
        persist_capacity: usize,
    ) -> (Self, mpsc::Receiver<PersistJob>) {
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
    pub async fn publish(
        &self,
        batch: Arc<TelemetryBatch>,
    ) -> Result<oneshot::Receiver<PersistOutcome>, PersistenceClosed> {
        let sender = self.persist.lock().expect("persist lock poisoned").clone();
        let Some(sender) = sender else {
            return Err(PersistenceClosed);
        };
        let (done, outcome) = oneshot::channel();
        sender
            .send(PersistJob { batch, done })
            .await
            .map_err(|_| PersistenceClosed)?;
        Ok(outcome)
    }

    /// Publish only successful, newly committed writes.
    pub fn committed(&self, batch: Arc<TelemetryBatch>) {
        self.register_batch_runs(&batch);
        let _ = self.live.send(batch);
    }
    pub fn remove_run(&self, run_id: &str) {
        self.live_run_ids
            .lock()
            .expect("run id lock poisoned")
            .remove(run_id);
    }

    /// Stop accepting batches for persistence; the loop drains what is queued and exits.
    pub fn close_persistence(&self) {
        self.persist.lock().expect("persist lock poisoned").take();
    }

    pub fn current_runs(&self) -> Vec<String> {
        self.live_run_ids
            .lock()
            .expect("run id lock poisoned")
            .iter()
            .cloned()
            .collect()
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
        self.live_run_ids
            .lock()
            .expect("run id lock poisoned")
            .clear();
    }

    fn register_batch_runs(&self, telemetry: &TelemetryBatch) {
        let runs = telemetry.run_ids();
        self.live_run_ids
            .lock()
            .expect("run id lock poisoned")
            .extend(runs.iter().cloned());
    }
}
