use std::sync::Arc;

use arrow::datatypes::Schema;
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

        match first.to_ascii_lowercase().as_str() {
            "scalars" => Self::Scalars,
            "images" => Self::Images,
            "hyperparameters" => Self::Hyperparameters,
            "distributions" => Self::Distributions,
            _ => Self::Unknown,
        }
    }

    pub fn from_batch(batch: &RecordBatch) -> Self {
        let schema = batch.schema();
        let field_count = schema.fields().len();

        // Helper to safely check field name at index
        fn field_name_eq(schema: &Schema, index: usize, expected_name: &str) -> bool {
            if index < schema.fields().len() {
                schema.field(index).name() == expected_name
            } else {
                false
            }
        }

        // Scalars: 4 fields, field[3] == "value"
        if field_count == 4 && field_name_eq(&schema, 3, "value") {
            return Self::Scalars;
        }
        // Images: 5 fields, field[3] == "image_data"
        if field_count == 5 && field_name_eq(&schema, 3, "image_data") {
            return Self::Images;
        }
        // Hyperparameters: 5 fields, field[2] == "key" and field[3] == "value"
        if field_count == 5
            && field_name_eq(&schema, 2, "key")
            && field_name_eq(&schema, 3, "value")
        {
            return Self::Hyperparameters;
        }
        // Distributions: 4 fields, field[3] == "values"
        if field_count == 4 && field_name_eq(&schema, 3, "values") {
            return Self::Distributions;
        }

        Self::Unknown
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
