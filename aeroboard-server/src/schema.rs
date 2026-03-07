use std::sync::Arc;
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};

/// Schema for scalar values (e.g., loss, accuracy)
#[allow(dead_code)]
pub fn scalar_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
    ]))
}

/// Schema for image data
#[allow(dead_code)]
pub fn image_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("image_data", DataType::Binary, false),
        Field::new("format", DataType::Utf8, false),
    ]))
}

/// Schema for distribution data
#[allow(dead_code)]
pub fn distribution_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new(
            "values",
            DataType::List(Arc::new(Field::new("item", DataType::Float64, true))),
            false,
        ),
    ]))
}

/// Schema for run-level hyperparameters / metadata.
#[allow(dead_code)]
pub fn hyperparameter_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
        Field::new("value_type", DataType::Utf8, false),
    ]))
}
