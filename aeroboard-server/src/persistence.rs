use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::app_state::AppState;
use crate::t_junction::PersistJob;

/// Start the loop that appends every published batch to the configured storage.
///
/// It drains the T-junction's bounded persistence queue, so no batch is skipped
/// when storage is slow: the queue fills up and ingestion waits instead. A failed
/// append is logged and the loop moves on (the backend may be temporarily
/// unavailable; GreptimeDB writes are retried inside the store).
///
/// Each job's outcome is sent back to the Flight request that published it, which
/// acknowledges the upload only once it is stored (or reports the failure so the
/// client retries).
///
/// The loop ends once `TJunction::close_persistence` has been called and the queue
/// is empty, which lets shutdown flush everything that was already accepted.
pub fn start_persistence_loop(
    state: AppState,
    mut rx: mpsc::Receiver<PersistJob>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(PersistJob { batch: telemetry, done }) = rx.recv().await {
            let outcome = match state.storage.append_batch(telemetry.clone()).await {
                Ok(()) => {
                    state.summaries.record(&telemetry).await;
                    tracing::debug!(
                        "Persisted {:?} batch of {} rows",
                        telemetry.kind,
                        telemetry.batch.num_rows()
                    );
                    Ok(())
                }
                Err(error) => {
                    tracing::error!(
                        "Failed to persist {:?} batch of {} rows: {}",
                        telemetry.kind,
                        telemetry.batch.num_rows(),
                        error
                    );
                    Err(error.to_string())
                }
            };
            // The uploader may have disconnected; the batch is stored either way.
            let _ = done.send(outcome);
        }
        tracing::info!("Persistence queue drained");
    })
}
