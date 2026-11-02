use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    config::AppConfig, storage::StorageEngine, summaries::SummaryCache,
    t_junction::{PersistJob, TJunction},
};

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub t_junction: Arc<TJunction>,
    pub storage: Arc<dyn StorageEngine>,
    pub summaries: Arc<SummaryCache>,
}

impl AppState {
    /// Build the shared state plus the receiving end of the persistence queue,
    /// which must be handed to `persistence::start_persistence_loop`.
    pub fn new(
        config: AppConfig,
        storage: Arc<dyn StorageEngine>,
    ) -> (Self, mpsc::Receiver<PersistJob>) {
        let (t_junction, persist_rx) = TJunction::new(config.live_queue_batches, config.persist_queue_batches);
        let state = Self {
            config,
            t_junction: Arc::new(t_junction),
            storage,
            summaries: Arc::new(SummaryCache::new()),
        };
        (state, persist_rx)
    }
}
