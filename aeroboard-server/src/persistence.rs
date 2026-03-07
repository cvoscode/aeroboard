use tokio::sync::broadcast;

use crate::app_state::AppState;

pub async fn start_persistence_loop(state: AppState) {
    let mut rx = state.t_junction.subscribe();
    let storage = state.storage.clone();

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(telemetry) => {
                    if let Err(error) = storage.append_batch(telemetry.clone()).await {
                        tracing::error!(
                            "Failed to persist {:?} batch of {} rows: {}",
                            telemetry.kind,
                            telemetry.batch.num_rows(),
                            error
                        );
                    } else {
                        tracing::debug!(
                            "Persisted {:?} batch of {} rows",
                            telemetry.kind,
                            telemetry.batch.num_rows()
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("Persistence loop lagged and skipped {} batches", skipped);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[allow(dead_code)]
pub fn record_batch_to_line_protocol(
    batch: &arrow::record_batch::RecordBatch,
) -> Result<String, crate::storage::BoxError> {
    crate::storage::record_batch_to_line_protocol(
        batch,
        &crate::telemetry::TelemetryKind::from_batch(batch),
    )
}
