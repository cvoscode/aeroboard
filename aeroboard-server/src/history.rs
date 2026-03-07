use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
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
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct DeleteRunsResponse {
    pub deleted: bool,
    pub error: Option<String>,
}

/// GET /api/history
pub async fn history_handler(
    State(_t_junction): State<Arc<TJunction>>,
) -> Json<HistoryResponse> {
    match query_history().await {
        Ok(rows) => Json(HistoryResponse { rows, error: None }),
        Err(e) => {
            let msg = e.to_string();
            tracing::error!("Failed to query history: {}", msg);
            Json(HistoryResponse { rows: vec![], error: Some(msg) })
        }
    }
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

async fn query_history() -> Result<Vec<HistoryPoint>, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    // Use SELECT * to avoid crashing on missing columns (e.g. if run_id hasn't been created yet)
    let sql = "SELECT * FROM scalars ORDER BY ts ASC LIMIT 1000000";
    let url = format!(
        "http://localhost:4000/v1/sql?sql={}&db=public",
        urlencoding::encode(sql)
    );

    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body: String = resp.text().await.unwrap_or_default();
        return Err(format!("GreptimeDB SQL error {}: {}", status, body).into());
    }

    let body: serde_json::Value = resp.json().await?;

    let records = body
        .get("output").and_then(|o| o.get(0))
        .and_then(|o| o.get("records"));

    let Some(records) = records else {
        return Err("Unexpected GreptimeDB response shape".into());
    };

    // Detect schema dynamically from column_schemas
    let schema = records.get("schema")
        .and_then(|s| s.get("column_schemas"))
        .and_then(|s| s.as_array());

    let Some(cols) = schema else {
        return Ok(vec![]);
    };

    // Build column index map
    let col_idx: std::collections::HashMap<&str, usize> = cols.iter().enumerate()
        .filter_map(|(i, c)| c.get("name").and_then(|n| n.as_str()).map(|n| (n, i)))
        .collect();

    let rows_raw = records.get("rows").and_then(|r| r.as_array());
    let Some(rows_raw) = rows_raw else { return Ok(vec![]); };

    // Support both old schema (name, value, ts) and new schema (metric, run_id, value, ts)
    let ts_idx      = col_idx.get("ts");
    let name_idx    = col_idx.get("metric").or_else(|| col_idx.get("name"));
    let run_id_idx  = col_idx.get("run_id");
    let value_idx   = col_idx.get("value");

    let (Some(&ts_idx), Some(&name_idx), Some(&value_idx)) = (ts_idx, name_idx, value_idx) else {
        tracing::warn!("history: column schema mismatch: {:?}", col_idx.keys().collect::<Vec<_>>());
        return Ok(vec![]);
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

    tracing::info!("Loaded {} historical data points from GreptimeDB", points.len());
    Ok(points)
}

async fn clear_history() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let statements = [
        "DELETE FROM scalars",
        "TRUNCATE TABLE scalars",
        "DELETE FROM scalars WHERE true",
    ];

    let mut last_error = String::new();

    for sql in statements {
        let url = format!(
            "http://localhost:4000/v1/sql?sql={}&db=public",
            urlencoding::encode(sql)
        );

        let resp = client.get(&url).send().await?;

        if resp.status().is_success() {
            tracing::info!("Cleared scalar history using SQL: {}", sql);
            return Ok(());
        }

        let status = resp.status();
        let body: String = resp.text().await.unwrap_or_default();
        last_error = format!("statement `{}` failed with {}: {}", sql, status, body);
        tracing::warn!("Failed to clear history using `{}`: {} {}", sql, status, body);
    }

    Err(format!("GreptimeDB could not clear `scalars`: {}", last_error).into())
}
