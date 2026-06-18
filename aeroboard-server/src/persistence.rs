use crate::app_state::AppState;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Start the persistence loop that buffers telemetry batches and appends them to storage.
///
/// This loop receives batches via the t_junction broadcast channel and uses StorageEngine
/// to persist them to the configured backend. Errors are logged but do not terminate
/// the loop - persistence failures are handled gracefully to allow live streaming to continue.
///
/// The returned `JoinHandle` must be awaited to ensure the loop runs to completion.
/// If dropped without awaiting, the persistence task is silently cancelled.
///
/// The loop handles broadcast channel errors gracefully:
/// - `Lagged`: Warns but continues (persistence is behind but live streaming continues)
/// - `Closed`: Normal shutdown condition
pub fn start_persistence_loop(state: AppState) -> JoinHandle<()> {
    let mut rx = state.t_junction.subscribe();
    let storage = state.storage.clone();

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(telemetry) => {
                    match storage.append_batch(telemetry.clone()).await {
                        Err(error) => {
                            tracing::warn!(
                                "Failed to persist {:?} batch of {} rows: {}",
                                telemetry.kind,
                                telemetry.batch.num_rows(),
                                error
                            );
                            // Continue processing - don't terminate on persistence errors
                            // The backend may be temporarily unavailable
                        }
                        Ok(()) => {
                            tracing::debug!(
                                "Persisted {:?} batch of {} rows",
                                telemetry.kind,
                                telemetry.batch.num_rows()
                            );
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        "Persistence loop lagged and skipped {} batches",
                        skipped
                    );
                    // Continue processing - don't terminate on lag events
                    // Live streaming continues even if persistence is behind
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}