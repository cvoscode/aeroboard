use std::sync::Arc;

use crate::{config::AppConfig, storage::StorageEngine, t_junction::TJunction};

#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub config: AppConfig,
    pub t_junction: Arc<TJunction>,
    pub storage: Arc<dyn StorageEngine>,
}
