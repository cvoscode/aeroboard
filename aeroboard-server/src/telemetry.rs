use std::sync::{Arc, OnceLock};

use arrow::record_batch::RecordBatch;
use axum::body::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryKind {
    Scalars,
    Images,
    Audio,
    Video,
    Hyperparameters,
    Distributions,
    Unknown,
}

impl TelemetryKind {
    pub fn from_descriptor_path(path: &[String]) -> Self {
        let Some(first) = path.first() else {
            return Self::Unknown;
        };

        match first.to_ascii_lowercase().as_str() {
            "scalars" => Self::Scalars,
            "images" => Self::Images,
            "audio" => Self::Audio,
            "video" => Self::Video,
            "hyperparameters" => Self::Hyperparameters,
            "distributions" => Self::Distributions,
            _ => Self::Unknown,
        }
    }

    /// Infer the kind from column names when the client sent no descriptor.
    /// The result is only a guess; `schema::normalize_batch` does the real validation.
    pub fn from_batch(batch: &RecordBatch) -> Self {
        let schema = batch.schema();
        let has = |name: &str| schema.index_of(name).is_ok();

        if has("image_data") {
            Self::Images
        } else if has("audio_data") {
            Self::Audio
        } else if has("video_data") {
            Self::Video
        } else if has("key") && has("value_type") {
            Self::Hyperparameters
        } else if has("values") {
            Self::Distributions
        } else if has("name") && has("value") {
            Self::Scalars
        } else {
            Self::Unknown
        }
    }

    pub fn as_dir_name(&self) -> Option<&'static str> {
        match self {
            Self::Scalars => Some("scalars"),
            Self::Images => Some("images"),
            Self::Audio => Some("audio"),
            Self::Video => Some("video"),
            Self::Hyperparameters => Some("hyperparameters"),
            Self::Distributions | Self::Unknown => None,
        }
    }

    /// Name of the binary payload column for media kinds.
    pub fn media_data_column(&self) -> Option<&'static str> {
        match self {
            Self::Images => Some("image_data"),
            Self::Audio => Some("audio_data"),
            Self::Video => Some("video_data"),
            _ => None,
        }
    }
}

/// A validated batch in the canonical layout for its kind (see `schema::normalize_batch`).
#[derive(Debug)]
pub struct TelemetryBatch {
    pub kind: TelemetryKind,
    pub batch: Arc<RecordBatch>,
    /// Arrow IPC encoding pushed to WebSocket subscribers, computed once per batch
    /// instead of once per connected browser. Index 0: payloads kept, 1: AV payloads stripped.
    live_ipc: [OnceLock<Bytes>; 2],
}

impl TelemetryBatch {
    pub fn new(kind: TelemetryKind, batch: Arc<RecordBatch>) -> Self {
        Self { kind, batch, live_ipc: [OnceLock::new(), OnceLock::new()] }
    }

    pub(crate) fn cached_live_ipc(
        &self,
        strip_media_payloads: bool,
        encode: impl FnOnce() -> Result<Bytes, arrow::error::ArrowError>,
    ) -> Result<Bytes, arrow::error::ArrowError> {
        let slot = &self.live_ipc[usize::from(strip_media_payloads)];
        if let Some(bytes) = slot.get() {
            return Ok(bytes.clone());
        }
        let bytes = encode()?;
        Ok(slot.get_or_init(|| bytes).clone())
    }
}
