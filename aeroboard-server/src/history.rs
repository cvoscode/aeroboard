use axum::{extract::State, http::StatusCode, Json};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use crate::t_junction::TJunction;

/// A single scalar data point returned from GreptimeDB history
#[derive(Serialize)]
pub struct HistoryPoint {
    pub ts_ms: i64,
    pub run_id: String,
    pub name: String,
    pub value: f64,
}

/// Response from /api/history
#[derive(Serialize)]
pub struct HistoryResponse {
    pub rows: Vec<HistoryPoint>,
    pub run_summaries: Vec<RunSummary>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct RunSummary {
    pub run_id: String,
    pub hyperparameters: Vec<HyperparameterEntry>,
    pub metric_summaries: Vec<MetricSummary>,
}

#[derive(Serialize, Clone)]
pub struct HyperparameterEntry {
    pub key: String,
    pub value: String,
    pub value_type: String,
}

#[derive(Serialize, Clone)]
pub struct MetricSummary {
    pub name: String,
    pub last: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Serialize)]
pub struct ImageHistoryPoint {
    pub ts_ms: i64,
    pub run_id: String,
    pub name: String,
    pub format: String,
    pub image_base64: String,
}

#[derive(Serialize)]
pub struct ImageHistoryResponse {
    pub rows: Vec<ImageHistoryPoint>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct DeleteRunsResponse {
    pub deleted: bool,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct RunsResponse {
    pub runs: Vec<String>,
}

/// GET /api/history
pub async fn history_handler(
    State(_t_junction): State<Arc<TJunction>>,
) -> Json<HistoryResponse> {
    match query_history().await {
        Ok((rows, run_summaries)) => Json(HistoryResponse { rows, run_summaries, error: None }),
        Err(e) => {
            let msg = e.to_string();
            tracing::error!("Failed to query history: {}", msg);
            Json(HistoryResponse { rows: vec![], run_summaries: vec![], error: Some(msg) })
        }
    }
}

/// GET /api/history/images
pub async fn image_history_handler(
    State(_t_junction): State<Arc<TJunction>>,
) -> Json<ImageHistoryResponse> {
    match query_image_history().await {
        Ok(rows) => Json(ImageHistoryResponse { rows, error: None }),
        Err(e) => {
            let msg = e.to_string();
            tracing::error!("Failed to query image history: {}", msg);
            Json(ImageHistoryResponse { rows: vec![], error: Some(msg) })
        }
    }
}

/// GET /api/runs
pub async fn runs_handler(
    State(t_junction): State<Arc<TJunction>>,
) -> Json<RunsResponse> {
    Json(RunsResponse {
        runs: t_junction.current_runs().await,
    })
}

/// DELETE /api/history
pub async fn delete_history_handler(
    State(_t_junction): State<Arc<TJunction>>,
) -> (StatusCode, Json<DeleteRunsResponse>) {
    match clear_history().await {
        Ok(()) => (
            StatusCode::OK,
            Json(DeleteRunsResponse { deleted: true, error: None })
        ),
        Err(e) => {
            let msg = e.to_string();
            tracing::error!("Failed to delete history: {}", msg);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DeleteRunsResponse { deleted: false, error: Some(msg) })
            )
        }
    }
}

async fn query_history() -> Result<(Vec<HistoryPoint>, Vec<RunSummary>), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    // Use SELECT * to avoid crashing on missing columns (e.g. if run_id hasn't been created yet)
    let sql = "SELECT * FROM scalars ORDER BY ts ASC LIMIT 1000000";
    let url = format!(
        "http://localhost:4000/v1/sql?sql={}&db=public",
        urlencoding::encode(sql)
    );

    let resp = match client.get(&url).send().await {
        Ok(resp) => resp,
        Err(error) if error.is_connect() => {
            tracing::warn!("GreptimeDB unavailable, serving live-only scalar history");
            return Ok((vec![], vec![]));
        }
        Err(error) => return Err(error.into()),
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body: String = resp.text().await.unwrap_or_default();
        return Err(format!("GreptimeDB SQL error {}: {}", status, body).into());
    }

    let body: Value = resp.json().await?;
    let rows_raw = extract_rows(&body)?;
    let col_idx = extract_column_indices(&body)?;

    // Support both old schema (name, value, ts) and new schema (metric, run_id, value, ts)
    let ts_idx      = col_idx.get("ts");
    let name_idx    = col_idx.get("metric").or_else(|| col_idx.get("name"));
    let run_id_idx  = col_idx.get("run_id");
    let value_idx   = col_idx.get("value");

    let (Some(&ts_idx), Some(&name_idx), Some(&value_idx)) = (ts_idx, name_idx, value_idx) else {
        tracing::warn!("history: column schema mismatch: {:?}", col_idx.keys().collect::<Vec<_>>());
        return Ok((vec![], vec![]));
    };

    let mut points = Vec::with_capacity(rows_raw.len());
    for row in rows_raw {
        let arr = match row.as_array() {
            Some(a) => a,
            None => continue,
        };
        if arr.len() <= ts_idx || arr.len() <= name_idx || arr.len() <= value_idx { continue; }

        // Timestamp is nanoseconds in new schema, parse accordingly
        let ts_ns: i64 = match &arr[ts_idx] {
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
            serde_json::Value::String(s) => s.parse::<i64>().unwrap_or(0),
            _ => 0,
        };
        let ts_ms = ts_ns / 1_000_000; // ns → ms

        let name   = arr[name_idx].as_str().unwrap_or("").to_string();
        let run_id = run_id_idx
            .and_then(|&i| arr.get(i))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_run")
            .to_string();
        let value = match &arr[value_idx] {
            serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
            _ => 0.0,
        };

        if name.is_empty() { continue; }
        points.push(HistoryPoint { ts_ms, run_id, name, value });
    }

    let run_summaries = build_run_summaries(&points, query_hyperparameters().await?);

    tracing::info!("Loaded {} historical data points from GreptimeDB", points.len());
    Ok((points, run_summaries))
}

async fn query_hyperparameters() -> Result<Vec<(String, HyperparameterEntry)>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let sql = "SELECT * FROM hyperparameters ORDER BY ts ASC LIMIT 100000";
    let url = format!(
        "http://localhost:4000/v1/sql?sql={}&db=public",
        urlencoding::encode(sql)
    );

    let resp = match client.get(&url).send().await {
        Ok(resp) => resp,
        Err(error) if error.is_connect() => {
            tracing::warn!("GreptimeDB unavailable, skipping persisted hyperparameters");
            return Ok(vec![]);
        }
        Err(error) => return Err(error.into()),
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body: String = resp.text().await.unwrap_or_default();
        tracing::warn!("Hyperparameter query failed {}: {}", status, body);
        return Ok(vec![]);
    }

    let body: Value = resp.json().await?;
    let rows = extract_rows(&body)?;
    let col_idx = extract_column_indices(&body)?;

    let ts_idx = col_idx.get("ts");
    let run_id_idx = col_idx.get("run_id");
    let key_idx = col_idx.get("key");
    let value_idx = col_idx.get("value");
    let value_type_idx = col_idx.get("value_type");
    let (Some(&_ts_idx), Some(&run_id_idx), Some(&key_idx), Some(&value_idx), Some(&value_type_idx)) = (ts_idx, run_id_idx, key_idx, value_idx, value_type_idx) else {
        return Ok(vec![]);
    };

    let mut entries = Vec::new();
    for row in rows {
        let Some(arr) = row.as_array() else { continue; };
        let run_id = arr.get(run_id_idx).and_then(Value::as_str).unwrap_or_default();
        let key = arr.get(key_idx).and_then(Value::as_str).unwrap_or_default();
        let value = arr.get(value_idx).and_then(Value::as_str).unwrap_or_default();
        let value_type = arr.get(value_type_idx).and_then(Value::as_str).unwrap_or("string");
        if run_id.is_empty() || key.is_empty() {
            continue;
        }
        entries.push((
            run_id.to_string(),
            HyperparameterEntry {
                key: key.to_string(),
                value: value.to_string(),
                value_type: value_type.to_string(),
            },
        ));
    }

    Ok(entries)
}

async fn query_image_history() -> Result<Vec<ImageHistoryPoint>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let sql = "SELECT * FROM images ORDER BY ts ASC LIMIT 100000";
    let url = format!(
        "http://localhost:4000/v1/sql?sql={}&db=public",
        urlencoding::encode(sql)
    );

    let resp = match client.get(&url).send().await {
        Ok(resp) => resp,
        Err(error) if error.is_connect() => {
            tracing::warn!("GreptimeDB unavailable, serving live-only image history");
            return Ok(vec![]);
        }
        Err(error) => return Err(error.into()),
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body: String = resp.text().await.unwrap_or_default();
        return Err(format!("GreptimeDB image SQL error {}: {}", status, body).into());
    }

    let body: Value = resp.json().await?;
    let rows = extract_rows(&body)?;
    let col_idx = extract_column_indices(&body)?;

    let ts_idx = col_idx.get("ts");
    let run_id_idx = col_idx.get("run_id");
    let name_idx = col_idx.get("name");
    let format_idx = col_idx.get("format");
    let image_hex_idx = col_idx.get("image_hex");
    let (Some(&ts_idx), Some(&run_id_idx), Some(&name_idx), Some(&format_idx), Some(&image_hex_idx)) = (ts_idx, run_id_idx, name_idx, format_idx, image_hex_idx) else {
        return Ok(vec![]);
    };

    let mut decoded = Vec::new();
    for row in rows {
        let Some(arr) = row.as_array() else { continue; };

        let ts_ns = parse_i64(arr.get(ts_idx));
        let run_id = arr.get(run_id_idx).and_then(Value::as_str).unwrap_or_default();
        let name = arr.get(name_idx).and_then(Value::as_str).unwrap_or_default();
        let format = arr.get(format_idx).and_then(Value::as_str).unwrap_or_default();
        let image_hex = arr.get(image_hex_idx).and_then(Value::as_str).unwrap_or_default();

        if run_id.is_empty() || name.is_empty() || format.is_empty() || image_hex.is_empty() {
            continue;
        }

        let bytes = decode_hex(image_hex)?;
        decoded.push(ImageHistoryPoint {
            ts_ms: ts_ns / 1_000_000,
            run_id: run_id.to_string(),
            name: name.to_string(),
            format: format.to_string(),
            image_base64: BASE64_STANDARD.encode(bytes),
        });
    }

    Ok(decoded)
}

fn extract_rows(body: &Value) -> Result<&Vec<Value>, Box<dyn std::error::Error>> {
    body.get("output")
        .and_then(|o| o.get(0))
        .and_then(|o| o.get("records"))
        .and_then(|r| r.get("rows"))
        .and_then(Value::as_array)
    .ok_or_else(|| Box::<dyn std::error::Error>::from("Unexpected GreptimeDB response shape"))
}

fn extract_column_indices(body: &Value) -> Result<std::collections::HashMap<&str, usize>, Box<dyn std::error::Error>> {
    let cols = body.get("output")
        .and_then(|o| o.get(0))
        .and_then(|o| o.get("records"))
        .and_then(|r| r.get("schema"))
        .and_then(|s| s.get("column_schemas"))
        .and_then(Value::as_array)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("Unexpected GreptimeDB response schema"))?;

    Ok(cols.iter().enumerate()
        .filter_map(|(i, c)| c.get("name").and_then(Value::as_str).map(|n| (n, i)))
        .collect())
}

pub(crate) fn build_run_summaries(points: &[HistoryPoint], hyperparameters: Vec<(String, HyperparameterEntry)>) -> Vec<RunSummary> {
    let mut per_run_metrics: std::collections::BTreeMap<String, std::collections::BTreeMap<String, MetricStatAccumulator>> = std::collections::BTreeMap::new();
    for point in points {
        let metric_map = per_run_metrics.entry(point.run_id.clone()).or_default();
        let stats = metric_map.entry(point.name.clone()).or_insert_with(|| MetricStatAccumulator {
            last_ts_ms: point.ts_ms,
            last: point.value,
            min: point.value,
            max: point.value,
        });
        stats.update(point.ts_ms, point.value);
    }

    let mut per_run_hparams: std::collections::BTreeMap<String, std::collections::BTreeMap<String, HyperparameterEntry>> = std::collections::BTreeMap::new();
    for (run_id, entry) in hyperparameters {
        per_run_hparams.entry(run_id).or_default().insert(entry.key.clone(), entry);
    }

    let run_ids: std::collections::BTreeSet<String> = per_run_metrics.keys().chain(per_run_hparams.keys()).cloned().collect();
    run_ids.into_iter().map(|run_id| {
        let hyperparameters = per_run_hparams.remove(&run_id)
            .map(|entries| entries.into_values().collect())
            .unwrap_or_default();
        let metric_summaries = per_run_metrics.remove(&run_id)
            .map(|metrics| metrics.into_iter().map(|(name, acc)| MetricSummary {
                name,
                last: acc.last,
                min: acc.min,
                max: acc.max,
            }).collect())
            .unwrap_or_default();
        RunSummary { run_id, hyperparameters, metric_summaries }
    }).collect()
}

struct MetricStatAccumulator {
    last_ts_ms: i64,
    last: f64,
    min: f64,
    max: f64,
}

impl MetricStatAccumulator {
    fn update(&mut self, ts_ms: i64, value: f64) {
        if ts_ms >= self.last_ts_ms {
            self.last_ts_ms = ts_ms;
            self.last = value;
        }
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }
}

fn parse_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if !hex.len().is_multiple_of(2) {
        return Err("Invalid hex payload length".into());
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

async fn clear_history() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let mut failures = Vec::new();

    for table in ["scalars", "images", "hyperparameters"] {
        match clear_table(&client, table).await {
            Ok(ClearTableOutcome::Cleared) => {
                tracing::info!("Cleared `{}` history", table);
            }
            Ok(ClearTableOutcome::SkippedMissing) => {
                tracing::info!("Skipping missing table `{}` while deleting history", table);
            }
            Ok(ClearTableOutcome::PersistenceUnavailable) => {
                tracing::warn!("GreptimeDB unavailable while deleting history; clearing UI state only");
                return Ok(());
            }
            Err(error) => failures.push(format!("{}: {}", table, error)),
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("GreptimeDB could not clear history: {}", failures.join(" | ")).into())
    }
}

enum ClearTableOutcome {
    Cleared,
    SkippedMissing,
    PersistenceUnavailable,
}

async fn clear_table(
    client: &reqwest::Client,
    table: &str,
) -> Result<ClearTableOutcome, String> {
    let statements = [
        format!("DELETE FROM {}", table),
        format!("TRUNCATE TABLE {}", table),
        format!("DELETE FROM {} WHERE true", table),
    ];

    let mut last_error = String::new();

    for sql in statements {
        let url = format!(
            "http://localhost:4000/v1/sql?sql={}&db=public",
            urlencoding::encode(&sql)
        );

        let resp = match client.get(&url).send().await {
            Ok(resp) => resp,
            Err(error) => {
                if error.is_connect() {
                    return Ok(ClearTableOutcome::PersistenceUnavailable);
                }
                return Err(format!("request failed for `{}`: {}", sql, error));
            }
        };

        if resp.status().is_success() {
            return Ok(ClearTableOutcome::Cleared);
        }

        let status = resp.status();
        let body: String = resp.text().await.unwrap_or_default();
        if is_missing_table_error(status, &body) {
            return Ok(ClearTableOutcome::SkippedMissing);
        }

        last_error = format!("statement `{}` failed with {}: {}", sql, status, body);
        tracing::warn!("Failed to clear `{}` using `{}`: {} {}", table, sql, status, body);
    }

    Err(last_error)
}

fn is_missing_table_error(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::NOT_FOUND {
        return true;
    }

    let normalized = body.to_ascii_lowercase();
    normalized.contains("table not found")
        || normalized.contains("does not exist")
        || normalized.contains("unknown table")
}
