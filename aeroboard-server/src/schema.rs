use std::sync::Arc;
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};

/// Schema for scalar values (e.g., loss, accuracy)
pub fn scalar_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
    ]))
}

/// Schema for image data
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
