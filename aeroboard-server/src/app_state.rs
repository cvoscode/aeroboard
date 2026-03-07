use std::sync::Arc;

use crate::{config::AppConfig, storage::StorageEngine, t_junction::TJunction};

#[derive(Clone)]
pub struct AppState {
    #[allow(dead_code)]
    pub config: AppConfig,
    pub t_junction: Arc<TJunction>,
    pub storage: Arc<dyn StorageEngine>,
}