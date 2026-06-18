mod codec;
mod filesystem;
mod greptime;
mod query;

use std::{collections::BTreeSet, io, sync::Arc};

use async_trait::async_trait;

use crate::{
    config::{AppConfig, StorageMode},
    history::{HistoryPoint, HyperparameterEntry, ImageHistoryPoint},
    telemetry::TelemetryBatch,
};

pub use codec::record_batch_to_line_protocol;
pub use filesystem::FilesystemStore;
pub use greptime::GreptimeStore;
pub use query::{HistoryQueryOptions, ImageFramePayload, ImageFrameQuery, ImageQueryOptions, QueryPage};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait StorageEngine: Send + Sync {
    async fn append_batch(&self, batch: Arc<TelemetryBatch>) -> Result<(), BoxError>;
    async fn query_history(&self, query: &HistoryQueryOptions) -> Result<QueryPage<HistoryPoint>, BoxError>;
    async fn query_hyperparameters(&self, run_ids: &BTreeSet<String>) -> Result<Vec<(String, HyperparameterEntry)>, BoxError>;
    async fn query_image_history(&self, query: &ImageQueryOptions) -> Result<QueryPage<ImageHistoryPoint>, BoxError>;
    async fn load_image_frame(&self, query: &ImageFrameQuery) -> Result<Option<ImageFramePayload>, BoxError>;
    async fn list_runs(&self) -> Result<Vec<String>, BoxError>;
    async fn clear_history(&self) -> Result<(), BoxError>;
}

pub fn build_storage(config: &AppConfig) -> Result<Arc<dyn StorageEngine>, BoxError> {
    match config.storage_mode {
        StorageMode::Filesystem => Ok(Arc::new(FilesystemStore::new(config.data_dir.clone())?)),
        StorageMode::Greptime => Ok(Arc::new(GreptimeStore::new(config.greptime_url.clone()))),
        StorageMode::LiveOnly => Ok(Arc::new(LiveOnlyStore)),
    }
}

pub struct LiveOnlyStore;

#[async_trait]
impl StorageEngine for LiveOnlyStore {
    async fn append_batch(&self, _batch: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        tracing::debug!("LiveOnlyStore::append_batch - accepting batch without persistence");
        Ok(())
    }

    async fn query_history(&self, _query: &HistoryQueryOptions) -> Result<QueryPage<HistoryPoint>, BoxError> {
        tracing::debug!("LiveOnlyStore::query_history - returning empty history");
        Ok(QueryPage::default())
    }

    async fn query_hyperparameters(&self, _run_ids: &BTreeSet<String>) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        tracing::debug!("LiveOnlyStore::query_hyperparameters - returning empty set");
        Ok(vec![])
    }

    async fn query_image_history(&self, _query: &ImageQueryOptions) -> Result<QueryPage<ImageHistoryPoint>, BoxError> {
        tracing::debug!("LiveOnlyStore::query_image_history - returning empty set");
        Ok(QueryPage::default())
    }

    async fn load_image_frame(&self, _query: &ImageFrameQuery) -> Result<Option<ImageFramePayload>, BoxError> {
        Ok(None)
    }

    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        tracing::debug!("LiveOnlyStore::list_runs - returning empty run list");
        Ok(vec![])
    }

    async fn clear_history(&self) -> Result<(), BoxError> {
        Err(boxed_error(
            "live-only mode does not support deleting persisted history (no persisted data exists)",
        ))
    }
}

pub(crate) fn boxed_error(message: impl Into<String>) -> BoxError {
    Box::new(io::Error::other(message.into()))
}
