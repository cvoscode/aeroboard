use crate::{app_state::AppState, t_junction::PersistJob};
use tokio::{sync::mpsc, task::JoinHandle};

/// Drain a bounded group, sharing the catalog's durable transactions and acknowledgments.
pub fn start_persistence_loop(
    state: AppState,
    mut rx: mpsc::Receiver<PersistJob>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut jobs = vec![first];
            // A short bounded window allows concurrent uploaders to share a commit.
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(2);
            let mut bytes = jobs[0].batch.batch.get_array_memory_size();
            while jobs.len() < 64 && bytes < 16 * 1024 * 1024 {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(job)) => {
                        bytes += job.batch.batch.get_array_memory_size();
                        jobs.push(job);
                    }
                    _ => break,
                }
            }
            let _gate = state.analysis.gate.lock().await;
            let outcomes = state
                .storage
                .append_group(jobs.iter().map(|job| job.batch.clone()).collect())
                .await;
            for (job, outcome) in jobs.into_iter().zip(outcomes) {
                match &outcome {
                    Ok(true) => {
                        state.analysis.invalidate();
                        state.summaries.record(&job.batch).await;
                        state.t_junction.committed(job.batch.clone());
                    }
                    Err(error) => tracing::error!("Failed to persist telemetry: {error}"),
                    _ => {}
                }
                let _ = job.done.send(outcome.map(|_| ()));
            }
        }
        tracing::info!("Persistence queue drained");
    })
}
