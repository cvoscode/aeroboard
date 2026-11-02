use std::{collections::HashMap, fmt::Write as _};

use arrow::{
    array::{Array, AsArray},
    record_batch::RecordBatch,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::Value;

use crate::{
    history::{HistoryPoint, HyperparameterEntry, ImageHistoryPoint},
    telemetry::TelemetryKind,
};

use super::{boxed_error, BoxError, MediaKind};

/// Encode a validated batch as InfluxDB line protocol for GreptimeDB.
///
/// `run_id` and the stream name (`name` / `key`) are **tags**, so together with the
/// timestamp they form the primary key. Rows from different runs that share a
/// metric name and millisecond therefore never overwrite each other, and filtering
/// by run uses the primary-key index.
///
/// Non-finite scalar values are skipped: line protocol cannot represent NaN/inf.
pub fn record_batch_to_line_protocol(
    batch: &RecordBatch,
    kind: &TelemetryKind,
) -> Result<String, BoxError> {
    let mut lines = String::new();
    let num_rows = batch.num_rows();
    if num_rows == 0 {
        return Ok(lines);
    }

    let timestamp_col = batch
        .column(0)
        .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let run_id_col = batch.column(1).as_string::<i32>();
    let text_col = batch.column(2).as_string::<i32>();
    let row_ok = |i: usize| {
        !timestamp_col.is_null(i)
            && !run_id_col.is_null(i)
            && !text_col.is_null(i)
            && !run_id_col.value(i).is_empty()
            && !text_col.value(i).is_empty()
    };

    if *kind == TelemetryKind::Scalars {
        let value_col = batch.column(3).as_primitive::<arrow::datatypes::Float64Type>();
        let step_col = (batch.num_columns() > 4)
            .then(|| batch.column(4).as_primitive::<arrow::datatypes::Float64Type>());

        for i in 0..num_rows {
            if !row_ok(i) || value_col.is_null(i) || !value_col.value(i).is_finite() {
                continue;
            }
            let _ = write!(
                lines,
                "scalars,run_id={},name={} value={}",
                escape_tag_value(run_id_col.value(i)),
                escape_tag_value(text_col.value(i)),
                value_col.value(i)
            );
            if let Some(steps) = step_col
                && !steps.is_null(i) && steps.value(i).is_finite() {
                    let _ = write!(lines, ",step={}", steps.value(i));
                }
            let _ = writeln!(lines, " {}", timestamp_col.value(i) * 1_000_000);
        }
    } else if let Some(media_kind) = MediaKind::from_telemetry(kind) {
        let table = media_kind.dir_name();
        let hex_column = media_kind.hex_column();
        let payload_col = batch.column(3).as_binary::<i32>();
        let format_col = batch.column(4).as_string::<i32>();

        for i in 0..num_rows {
            if !row_ok(i) || payload_col.is_null(i) || format_col.is_null(i) {
                continue;
            }
            let _ = writeln!(
                lines,
                "{table},run_id={},name={} format=\"{}\",{hex_column}=\"{}\" {}",
                escape_tag_value(run_id_col.value(i)),
                escape_tag_value(text_col.value(i)),
                escape_field_string(format_col.value(i)),
                hex_encode(payload_col.value(i)),
                timestamp_col.value(i) * 1_000_000
            );
        }
    } else if *kind == TelemetryKind::Hyperparameters {
        let value_col = batch.column(3).as_string::<i32>();
        let value_type_col = batch.column(4).as_string::<i32>();

        for i in 0..num_rows {
            if !row_ok(i) || value_col.is_null(i) || value_type_col.is_null(i) {
                continue;
            }
            let _ = writeln!(
                lines,
                "hyperparameters,run_id={},key={} value=\"{}\",value_type=\"{}\" {}",
                escape_tag_value(run_id_col.value(i)),
                escape_tag_value(text_col.value(i)),
                escape_field_string(value_col.value(i)),
                escape_field_string(value_type_col.value(i)),
                timestamp_col.value(i) * 1_000_000
            );
        }
    }

    Ok(lines)
}

pub(crate) fn parse_scalar_batch(batch: &RecordBatch) -> Vec<HistoryPoint> {
    let timestamp_col = batch
        .column(0)
        .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let run_id_col = batch.column(1).as_string::<i32>();
    let name_col = batch.column(2).as_string::<i32>();
    let value_col = batch
        .column(3)
        .as_primitive::<arrow::datatypes::Float64Type>();
    let step_col = (batch.num_columns() > 4)
        .then(|| batch.column(4).as_primitive::<arrow::datatypes::Float64Type>());
    let mut rows = Vec::with_capacity(batch.num_rows());

    for i in 0..batch.num_rows() {
        if timestamp_col.is_null(i)
            || run_id_col.is_null(i)
            || name_col.is_null(i)
            || value_col.is_null(i)
        {
            continue;
        }

        rows.push(HistoryPoint {
            ts_ms: timestamp_col.value(i),
            run_id: run_id_col.value(i).to_string(),
            name: name_col.value(i).to_string(),
            value: value_col.value(i),
            step: step_col.and_then(|steps| (!steps.is_null(i)).then(|| steps.value(i))),
        });
    }

    rows
}

pub(crate) fn parse_hparam_batch(batch: &RecordBatch) -> Vec<(String, HyperparameterEntry)> {
    let run_id_col = batch.column(1).as_string::<i32>();
    let key_col = batch.column(2).as_string::<i32>();
    let value_col = batch.column(3).as_string::<i32>();
    let value_type_col = batch.column(4).as_string::<i32>();
    let mut rows = Vec::with_capacity(batch.num_rows());

    for i in 0..batch.num_rows() {
        if run_id_col.is_null(i)
            || key_col.is_null(i)
            || value_col.is_null(i)
            || value_type_col.is_null(i)
        {
            continue;
        }

        rows.push((
            run_id_col.value(i).to_string(),
            HyperparameterEntry {
                key: key_col.value(i).to_string(),
                value: value_col.value(i).to_string(),
                value_type: value_type_col.value(i).to_string(),
            },
        ));
    }

    rows
}

pub(crate) fn parse_image_batch(batch: &RecordBatch, include_data: bool) -> Vec<ImageHistoryPoint> {
    let timestamp_col = batch
        .column(0)
        .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let run_id_col = batch.column(1).as_string::<i32>();
    let name_col = batch.column(2).as_string::<i32>();
    let image_col = batch.column(3).as_binary::<i32>();
    let format_col = batch.column(4).as_string::<i32>();
    let mut rows = Vec::with_capacity(batch.num_rows());

    for i in 0..batch.num_rows() {
        if timestamp_col.is_null(i)
            || run_id_col.is_null(i)
            || name_col.is_null(i)
            || image_col.is_null(i)
            || format_col.is_null(i)
        {
            continue;
        }

        rows.push(ImageHistoryPoint {
            ts_ms: timestamp_col.value(i),
            run_id: run_id_col.value(i).to_string(),
            name: name_col.value(i).to_string(),
            format: format_col.value(i).to_string(),
            image_base64: include_data.then(|| BASE64_STANDARD.encode(image_col.value(i))),
        });
    }

    rows
}

pub(crate) fn sanitize_extension(format: &str) -> String {
    let normalized = format.trim().trim_start_matches('.').to_ascii_lowercase();
    if normalized.is_empty() {
        "bin".to_string()
    } else {
        let sanitized = normalized
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .chars()
            .take(10)
            .collect::<String>();

        if sanitized.is_empty() {
            "bin".to_string()
        } else {
            sanitized
        }
    }
}

/// Escape a tag value: commas, equals signs and spaces are delimiters, and a
/// line break would end the line.
fn escape_tag_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            ',' | '=' | ' ' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' | '\r' => out.push_str("\\ "),
            _ => out.push(ch),
        }
    }
    out
}

/// Escape a quoted field string: only `"` and `\` are special inside the quotes.
fn escape_field_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn extract_rows(body: &Value) -> Result<&Vec<Value>, BoxError> {
    body.get("output")
        .and_then(|o| o.get(0))
        .and_then(|o| o.get("records"))
        .and_then(|r| r.get("rows"))
        .and_then(Value::as_array)
        .ok_or_else(|| boxed_error("Unexpected GreptimeDB response shape"))
}

pub(crate) fn extract_column_indices(body: &Value) -> Result<HashMap<&str, usize>, BoxError> {
    let cols = body
        .get("output")
        .and_then(|o| o.get(0))
        .and_then(|o| o.get("records"))
        .and_then(|r| r.get("schema"))
        .and_then(|s| s.get("column_schemas"))
        .and_then(Value::as_array)
        .ok_or_else(|| boxed_error("Unexpected GreptimeDB response schema"))?;

    Ok(cols
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.get("name").and_then(Value::as_str).map(|n| (n, i)))
        .collect())
}

pub(crate) fn parse_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

pub(crate) fn decode_hex(hex: &str) -> Result<Vec<u8>, BoxError> {
    fn nibble(byte: u8) -> Result<u8, BoxError> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(boxed_error("Invalid hex digit in payload")),
        }
    }

    let raw = hex.as_bytes();
    if !raw.len().is_multiple_of(2) {
        return Err(boxed_error("Invalid hex payload length"));
    }
    raw.chunks_exact(2)
        .map(|pair| Ok((nibble(pair[0])? << 4) | nibble(pair[1])?))
        .collect()
}
