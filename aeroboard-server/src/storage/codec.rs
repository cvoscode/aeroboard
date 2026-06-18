use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

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

use super::{boxed_error, BoxError};

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

    if *kind == TelemetryKind::Scalars {
        let value_col = batch
            .column(3)
            .as_primitive::<arrow::datatypes::Float64Type>();

        for i in 0..num_rows {
            if timestamp_col.is_null(i)
                || run_id_col.is_null(i)
                || text_col.is_null(i)
                || value_col.is_null(i)
            {
                continue;
            }

            let ts = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let name = text_col.value(i);
            let val = value_col.value(i);

            let safe_name = name.replace(' ', "\\ ").replace(',', "\\,");
            let safe_run_id = run_id
                .replace(' ', "\\ ")
                .replace(',', "\\,")
                .replace('"', "\\\"");

            lines.push_str(&format!(
                "scalars,name={safe_name} run_id=\"{safe_run_id}\",value={val} {ts_ns}\n",
                ts_ns = ts * 1_000_000
            ));
        }
    } else if *kind == TelemetryKind::Images {
        let image_col = batch.column(3).as_binary::<i32>();
        let format_col = batch.column(4).as_string::<i32>();

        for i in 0..num_rows {
            if timestamp_col.is_null(i)
                || run_id_col.is_null(i)
                || text_col.is_null(i)
                || image_col.is_null(i)
                || format_col.is_null(i)
            {
                continue;
            }

            let ts = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let name = text_col.value(i);
            let format = format_col.value(i);
            let encoded = hex_encode(image_col.value(i));

            let safe_name = escape_tag_value(name);
            let safe_run_id = escape_field_string(run_id);
            let safe_format = escape_field_string(format);
            let safe_image = escape_field_string(&encoded);

            lines.push_str(&format!(
                "images,name={safe_name} run_id=\"{safe_run_id}\",format=\"{safe_format}\",image_hex=\"{safe_image}\" {ts_ns}\n",
                ts_ns = ts * 1_000_000
            ));
        }
    } else if *kind == TelemetryKind::Hyperparameters {
        let value_col = batch.column(3).as_string::<i32>();
        let value_type_col = batch.column(4).as_string::<i32>();

        for i in 0..num_rows {
            if timestamp_col.is_null(i)
                || run_id_col.is_null(i)
                || text_col.is_null(i)
                || value_col.is_null(i)
                || value_type_col.is_null(i)
            {
                continue;
            }

            let ts = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let key = text_col.value(i);
            let value = value_col.value(i);
            let value_type = value_type_col.value(i);

            let safe_key = escape_tag_value(key);
            let safe_run_id = escape_field_string(run_id);
            let safe_value = escape_field_string(value);
            let safe_type = escape_field_string(value_type);

            lines.push_str(&format!(
                "hyperparameters,key={safe_key} run_id=\"{safe_run_id}\",value=\"{safe_value}\",value_type=\"{safe_type}\" {ts_ns}\n",
                ts_ns = ts * 1_000_000
            ));
        }
    }

    Ok(lines)
}

pub(crate) fn extract_first_run_id(batch: &RecordBatch) -> Option<String> {
    let run_id_col = batch.column(1).as_string::<i32>();
    (0..batch.num_rows())
        .find(|&index| !run_id_col.is_null(index))
        .map(|index| run_id_col.value(index).to_string())
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

fn escape_tag_value(value: &str) -> String {
    value
        .replace(' ', "\\ ")
        .replace(',', "\\,")
        .replace('=', "\\=")
}

fn escape_field_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
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
    if !hex.len().is_multiple_of(2) {
        return Err(boxed_error("Invalid hex payload length"));
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let chars: Vec<char> = hex.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let part = [chars[i], chars[i + 1]].iter().collect::<String>();
        bytes.push(u8::from_str_radix(&part, 16)?);
        i += 2;
    }
    Ok(bytes)
}
