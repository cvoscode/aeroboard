use std::sync::Arc;

use arrow::record_batch::RecordBatch;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryKind {
    Scalars,
    Images,
    Hyperparameters,
    Distributions,
    Unknown,
}

impl TelemetryKind {
    pub fn from_descriptor_path(path: &[String]) -> Self {
        let Some(first) = path.first() else {
            return Self::Unknown;
        };

        match first
            .to_ascii_lowercase()
            .as_str()
        {
            "scalars" => Self::Scalars,
            "images" => Self::Images,
            "hyperparameters" => Self::Hyperparameters,
            "distributions" => Self::Distributions,
            _ => Self::Unknown,
        }
    }

    pub fn from_batch(batch: &RecordBatch) -> Self {
        let schema = batch.schema();

        if schema.fields().len() == 4 && schema.field(3).name() == "value" {
            Self::Scalars
        } else if schema.fields().len() == 5 && schema.field(3).name() == "image_data" {
            Self::Images
        } else if schema.fields().len() == 5
            && schema.field(2).name() == "key"
            && schema.field(3).name() == "value"
        {
            Self::Hyperparameters
        } else if schema.fields().len() == 4 && schema.field(3).name() == "values" {
            Self::Distributions
        } else {
            Self::Unknown
        }
    }

    pub fn as_dir_name(&self) -> Option<&'static str> {
        match self {
            Self::Scalars => Some("scalars"),
            Self::Images => Some("images"),
            Self::Hyperparameters => Some("hyperparameters"),
            Self::Distributions | Self::Unknown => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TelemetryBatch {
    pub kind: TelemetryKind,
    pub batch: Arc<RecordBatch>,
}

impl TelemetryBatch {
    pub fn new(kind: TelemetryKind, batch: Arc<RecordBatch>) -> Self {
        Self { kind, batch }
    }
}