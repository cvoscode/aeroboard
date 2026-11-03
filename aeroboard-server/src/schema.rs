//! Canonical Arrow schemas for every telemetry kind, and the validation that
//! every ingested batch goes through before it reaches storage or the live stream.
//!
//! Storage codecs read columns by position with typed downcasts, so a batch that
//! reaches them with the wrong layout would panic. `normalize_batch` is the single
//! gate that guarantees the layout: it checks column names, casts compatible types
//! (e.g. `int64` values, `us` timestamps, `large_string`) to the canonical ones, and
//! rejects anything else with a descriptive error.

use std::sync::Arc;

use arrow::compute::{can_cast_types, cast};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use arrow::record_batch::RecordBatch;

use crate::telemetry::TelemetryKind;

fn timestamp_field() -> Field {
    Field::new(
        "timestamp",
        DataType::Timestamp(TimeUnit::Millisecond, None),
        false,
    )
}

/// Schema for scalar values (e.g., loss, accuracy). `step` is optional: older
/// clients send the first four columns only.
pub fn scalar_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        timestamp_field(),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
        Field::new("step", DataType::Float64, true),
    ]))
}

/// Pre-`step` scalar layout, still accepted from older clients.
#[cfg(test)]
pub fn legacy_scalar_schema() -> SchemaRef {
    let schema = scalar_schema();
    Arc::new(Schema::new(schema.fields()[..4].to_vec()))
}

/// Schema for image data
#[cfg(test)]
pub fn image_schema() -> SchemaRef {
    media_schema("image_data")
}

/// Schema for encoded media (images, audio or video). `data_column` is
/// `"image_data"`, `"audio_data"` or `"video_data"`; the column name is what
/// distinguishes the kinds.
pub fn media_schema(data_column: &str) -> SchemaRef {
    Arc::new(Schema::new(vec![
        timestamp_field(),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new(data_column, DataType::Binary, false),
        Field::new("format", DataType::Utf8, false),
    ]))
}

/// Schema for distribution data
pub fn distribution_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        timestamp_field(),
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
pub fn hyperparameter_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        timestamp_field(),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
        Field::new("value_type", DataType::Utf8, false),
    ]))
}

/// The canonical schema for a telemetry kind (`None` for `Unknown`).
pub fn canonical_schema(kind: &TelemetryKind) -> Option<SchemaRef> {
    match kind {
        TelemetryKind::Scalars => Some(scalar_schema()),
        TelemetryKind::Images => Some(media_schema("image_data")),
        TelemetryKind::Audio => Some(media_schema("audio_data")),
        TelemetryKind::Video => Some(media_schema("video_data")),
        TelemetryKind::Hyperparameters => Some(hyperparameter_schema()),
        TelemetryKind::Distributions => Some(distribution_schema()),
        TelemetryKind::Unknown => None,
    }
}

/// Validate `batch` against the canonical schema of `kind` and return it in the
/// canonical layout. Scalars without a `step` column get an all-null one so
/// downstream code can rely on a single layout.
pub fn normalize_batch(kind: &TelemetryKind, batch: &RecordBatch) -> Result<RecordBatch, String> {
    let Some(expected) = canonical_schema(kind) else {
        return Err(format!(
            "unrecognised telemetry batch with columns [{}]; set the Flight descriptor path to one of \
             scalars, images, audio, video, hyperparameters, distributions",
            column_names(batch.schema().as_ref())
        ));
    };

    let actual = batch.schema();
    let expected = if kind.media_data_column().is_some() && actual.index_of("asset_id").is_ok() {
        let mut fields = expected
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect::<Vec<_>>();
        fields.extend([
            Field::new("asset_id", DataType::Utf8, false),
            Field::new("step", DataType::Float64, true),
            Field::new("segment", DataType::Utf8, true),
        ]);
        Arc::new(Schema::new(fields))
    } else {
        expected
    };
    let optional_trailing = usize::from(*kind == TelemetryKind::Scalars);
    let required = expected.fields().len() - optional_trailing;
    let column_count = actual.fields().len();
    if column_count < required || column_count > expected.fields().len() {
        return Err(format!(
            "{kind:?} batch has columns [{}], expected [{}]",
            column_names(actual.as_ref()),
            column_names(expected.as_ref())
        ));
    }

    let mut columns = Vec::with_capacity(expected.fields().len());
    for (index, expected_field) in expected.fields().iter().enumerate() {
        let Some(actual_field) = actual.fields().get(index) else {
            // Only the optional trailing scalar `step` column can be missing.
            columns.push(arrow::array::new_null_array(
                expected_field.data_type(),
                batch.num_rows(),
            ));
            continue;
        };
        if actual_field.name() != expected_field.name() {
            return Err(format!(
                "{kind:?} batch column {index} is named '{}', expected '{}'",
                actual_field.name(),
                expected_field.name()
            ));
        }

        let column = batch.column(index);
        if column.data_type() == expected_field.data_type() {
            columns.push(column.clone());
        } else if can_cast_types(column.data_type(), expected_field.data_type()) {
            let casted = cast(column, expected_field.data_type()).map_err(|error| {
                format!(
                    "{kind:?} column '{}' could not be converted from {} to {}: {error}",
                    expected_field.name(),
                    column.data_type(),
                    expected_field.data_type()
                )
            })?;
            // Safe casts turn unconvertible values (e.g. the string "oops" to a
            // float) into nulls; report that as the type error it is.
            let lost = casted.null_count().saturating_sub(column.null_count());
            if lost > 0 {
                return Err(format!(
                    "{kind:?} column '{}' has type {}, expected {} ({lost} value(s) could not be converted)",
                    expected_field.name(),
                    column.data_type(),
                    expected_field.data_type()
                ));
            }
            columns.push(casted);
        } else {
            return Err(format!(
                "{kind:?} column '{}' has type {}, expected {}",
                expected_field.name(),
                column.data_type(),
                expected_field.data_type()
            ));
        }

        let column = columns.last().expect("column was just pushed");
        if !expected_field.is_nullable() && column.null_count() > 0 {
            return Err(format!(
                "{kind:?} column '{}' contains {} null value(s)",
                expected_field.name(),
                column.null_count()
            ));
        }
    }

    RecordBatch::try_new(expected, columns).map_err(|error| error.to_string())
}

fn column_names(schema: &Schema) -> String {
    schema
        .fields()
        .iter()
        .map(|field| format!("{}: {}", field.name(), field.data_type()))
        .collect::<Vec<_>>()
        .join(", ")
}
