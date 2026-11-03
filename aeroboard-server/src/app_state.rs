use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    config::AppConfig,
    storage::StorageEngine,
    summaries::SummaryCache,
    t_junction::{PersistJob, TJunction},
};

#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub t_junction: Arc<TJunction>,
    pub storage: Arc<dyn StorageEngine>,
    pub summaries: Arc<SummaryCache>,
    pub catalog: crate::catalog::Catalog,
    pub analysis: Arc<crate::analytics::AnalysisCache>,
}

impl AppState {
    /// Build the shared state plus the receiving end of the persistence queue,
    /// which must be handed to `persistence::start_persistence_loop`.
    pub fn new(
        config: AppConfig,
        storage: Arc<dyn StorageEngine>,
    ) -> Result<(Self, mpsc::Receiver<PersistJob>), crate::storage::BoxError> {
        let catalog = crate::catalog::Catalog::open(
            &config.data_dir,
            config.storage_mode != crate::config::StorageMode::LiveOnly,
        )?;
        let storage = Arc::new(crate::storage::DurableStore::new(
            storage,
            config.clone(),
            catalog.clone(),
        ));
        let (t_junction, persist_rx) =
            TJunction::new(config.live_queue_batches, config.persist_queue_batches);
        let state = Self {
            config,
            t_junction: Arc::new(t_junction),
            storage,
            summaries: Arc::new(SummaryCache::new()),
            catalog,
            analysis: Arc::new(crate::analytics::AnalysisCache::default()),
        };
        Ok((state, persist_rx))
    }
}
