use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::{Client, StatusCode};
use serde_json::Value;

use crate::{
    history::{HistoryPoint, HyperparameterEntry, ImageHistoryPoint},
    retry::{RetryConfig, RetryStrategy},
    telemetry::TelemetryBatch,
};

use super::{
    boxed_error,
    codec::{
        decode_hex, extract_column_indices, extract_rows, parse_i64, record_batch_to_line_protocol,
    },
    query::finalize_page,
    BoxError, HistoryQueryOptions, ImageFramePayload, ImageFrameQuery, ImageQueryOptions,
    QueryPage, StorageEngine,
};

pub struct GreptimeStore {
    client: Client,
    base_url: String,
}

impl GreptimeStore {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .connect_timeout(std::time::Duration::from_secs(5))
                .pool_max_idle_per_host(10)
                .pool_idle_timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("Failed to create HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn write_url(&self) -> String {
        format!("{}/v1/influxdb/write?db=public", self.base_url)
    }

    fn sql_url(&self, sql: &str) -> String {
        format!(
            "{}/v1/sql?sql={}&db=public",
            self.base_url,
            urlencoding::encode(sql)
        )
    }
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn build_where_clause(
    run_ids: &BTreeSet<String>,
    metrics: &BTreeSet<String>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    metric_column: &str,
) -> String {
    let mut clauses = Vec::new();

    if !run_ids.is_empty() {
        let joined = run_ids.iter().map(|run_id| sql_string_literal(run_id)).collect::<Vec<_>>().join(", ");
        clauses.push(format!("run_id IN ({joined})"));
    }

    if !metrics.is_empty() {
        let joined = metrics.iter().map(|metric| sql_string_literal(metric)).collect::<Vec<_>>().join(", ");
        clauses.push(format!("{metric_column} IN ({joined})"));
    }

    if let Some(start_ms) = start_ms {
        clauses.push(format!("ts >= {}", start_ms * 1_000_000));
    }

    if let Some(end_ms) = end_ms {
        clauses.push(format!("ts <= {}", end_ms * 1_000_000));
    }

    if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    }
}

fn apply_page_limit(sql: &str, cursor: Option<usize>, limit: Option<usize>) -> String {
    let offset = cursor.unwrap_or(0);
    match limit {
        Some(limit) => format!("{sql} LIMIT {} OFFSET {}", limit + 1, offset),
        None if offset > 0 => format!("{sql} LIMIT 1000000 OFFSET {}", offset),
        None => sql.to_string(),
    }
}

#[async_trait]
impl StorageEngine for GreptimeStore {
    async fn append_batch(&self, batch: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        let line_protocol_data = record_batch_to_line_protocol(&batch.batch, &batch.kind)?;
        if line_protocol_data.is_empty() {
            tracing::trace!("Skipping empty batch");
            return Ok(());
        }

        let write_url = &self.write_url();
        let retry_strategy = RetryStrategy::with_config(RetryConfig::default());
        let mut attempt_body: Option<String> = Some(line_protocol_data.clone());

        for attempt in 1..=retry_strategy.max_attempts() {
            let body = attempt_body.take().expect("Retry logic error: missing body");
            match self.client.post(write_url).body(body.clone()).send().await {
                Ok(resp) => match resp.status() {
                    StatusCode::OK | StatusCode::CREATED => return Ok(()),
                    StatusCode::TOO_MANY_REQUESTS => {
                        let resp_text = resp.text().await.unwrap_or_default();
                        tracing::warn!(
                            "GreptimeDB rate limited (attempt {}/{}): {}",
                            attempt,
                            retry_strategy.max_attempts(),
                            resp_text
                        );
                        if attempt < retry_strategy.max_attempts() {
                            let delay = retry_strategy.delay_for_attempt(attempt);
                            tokio::time::sleep(delay).await;
                            attempt_body = Some(line_protocol_data.clone());
                            continue;
                        }
                        return Err(boxed_error("GreptimeDB rate limited, max attempts exceeded"));
                    }
                    status => {
                        let body = resp.text().await.unwrap_or_default();
                        return Err(boxed_error(format!("GreptimeDB error {}: {}", status, body)));
                    }
                },
                Err(error) => {
                    if retry_strategy.should_retry_error(&error.to_string()) {
                        let delay = retry_strategy.delay_for_attempt(attempt);
                        tokio::time::sleep(delay).await;
                        attempt_body = Some(line_protocol_data.clone());
                        continue;
                    }
                    return Err(Box::new(error));
                }
            }
        }

        Err(boxed_error(format!(
            "GreptimeDB append failed after {} attempts",
            retry_strategy.max_attempts()
        )))
    }

    async fn query_history(&self, query: &HistoryQueryOptions) -> Result<QueryPage<HistoryPoint>, BoxError> {
        let where_clause = build_where_clause(&query.run_ids, &query.metrics, query.start_ms, query.end_ms, "metric");
        let sql = apply_page_limit(&format!("SELECT * FROM scalars{where_clause} ORDER BY ts ASC"), query.cursor, query.limit);
        let resp = query_with_retry_configured(&self.client, &self.sql_url(&sql), RetryConfig::default()).await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(boxed_error(format!("GreptimeDB history SQL error {}: {}", status, body)));
        }

        let body: Value = resp.json().await?;
        let rows_raw = extract_rows(&body)?;
        let col_idx = extract_column_indices(&body)?;

        let ts_idx = col_idx.get("ts");
        let name_idx = col_idx.get("metric").or_else(|| col_idx.get("name"));
        let run_id_idx = col_idx.get("run_id");
        let value_idx = col_idx.get("value");

        let (Some(&ts_idx), Some(&name_idx), Some(&value_idx)) = (ts_idx, name_idx, value_idx) else {
            return Ok(QueryPage::default());
        };

        let mut points = Vec::with_capacity(rows_raw.len());
        for row in rows_raw {
            let Some(arr) = row.as_array() else {
                continue;
            };
            if arr.len() <= ts_idx || arr.len() <= name_idx || arr.len() <= value_idx {
                continue;
            }

            let name = arr.get(name_idx).and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                continue;
            }

            let run_id = run_id_idx
                .and_then(|&i| arr.get(i))
                .and_then(Value::as_str)
                .unwrap_or("unknown_run");

            let value = match &arr[value_idx] {
                Value::Number(n) => n.as_f64().unwrap_or(0.0),
                _ => 0.0,
            };

            points.push(HistoryPoint {
                ts_ms: parse_i64(arr.get(ts_idx)) / 1_000_000,
                run_id: run_id.to_string(),
                name: name.to_string(),
                value,
            });
        }

        Ok(finalize_page(points, query.cursor, query.limit))
    }

    async fn query_hyperparameters(&self, run_ids: &BTreeSet<String>) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        let where_clause = build_where_clause(run_ids, &BTreeSet::new(), None, None, "key");
        let sql = format!("SELECT * FROM hyperparameters{where_clause} ORDER BY ts ASC LIMIT 100000");
        let resp = query_with_retry_configured(&self.client, &self.sql_url(&sql), RetryConfig::default()).await?;

        if !resp.status().is_success() {
            return Ok(vec![]);
        }

        let body: Value = resp.json().await?;
        let rows = extract_rows(&body)?;
        let col_idx = extract_column_indices(&body)?;

        let run_id_idx = col_idx.get("run_id");
        let key_idx = col_idx.get("key");
        let value_idx = col_idx.get("value");
        let value_type_idx = col_idx.get("value_type");
        let (Some(&run_id_idx), Some(&key_idx), Some(&value_idx), Some(&value_type_idx)) =
            (run_id_idx, key_idx, value_idx, value_type_idx)
        else {
            return Ok(vec![]);
        };

        let mut entries = Vec::new();
        for row in rows {
            let Some(arr) = row.as_array() else {
                continue;
            };
            let run_id = arr.get(run_id_idx).and_then(Value::as_str).unwrap_or_default();
            let key = arr.get(key_idx).and_then(Value::as_str).unwrap_or_default();
            if run_id.is_empty() || key.is_empty() {
                continue;
            }

            entries.push((
                run_id.to_string(),
                HyperparameterEntry {
                    key: key.to_string(),
                    value: arr.get(value_idx).and_then(Value::as_str).unwrap_or_default().to_string(),
                    value_type: arr
                        .get(value_type_idx)
                        .and_then(Value::as_str)
                        .unwrap_or("string")
                        .to_string(),
                },
            ));
        }

        Ok(entries)
    }

    async fn query_image_history(&self, query: &ImageQueryOptions) -> Result<QueryPage<ImageHistoryPoint>, BoxError> {
        let where_clause = build_where_clause(&query.run_ids, &query.metrics, query.start_ms, query.end_ms, "name");
        let columns = if query.include_data {
            "ts, run_id, name, format, image_hex"
        } else {
            "ts, run_id, name, format"
        };
        let sql = apply_page_limit(&format!("SELECT {columns} FROM images{where_clause} ORDER BY ts ASC"), query.cursor, query.limit);
        let resp = query_with_retry_configured(&self.client, &self.sql_url(&sql), RetryConfig::default()).await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(boxed_error(format!("GreptimeDB image SQL error {}: {}", status, body)));
        }

        let body: Value = resp.json().await?;
        let rows = extract_rows(&body)?;
        let col_idx = extract_column_indices(&body)?;

        let ts_idx = col_idx.get("ts");
        let run_id_idx = col_idx.get("run_id");
        let name_idx = col_idx.get("name");
        let format_idx = col_idx.get("format");
        let image_hex_idx = col_idx.get("image_hex").copied();
        let (Some(&ts_idx), Some(&run_id_idx), Some(&name_idx), Some(&format_idx)) =
            (ts_idx, run_id_idx, name_idx, format_idx)
        else {
            return Ok(QueryPage::default());
        };

        let mut decoded = Vec::new();
        for row in rows {
            let Some(arr) = row.as_array() else {
                continue;
            };

            let run_id = arr.get(run_id_idx).and_then(Value::as_str).unwrap_or_default();
            let name = arr.get(name_idx).and_then(Value::as_str).unwrap_or_default();
            let format = arr.get(format_idx).and_then(Value::as_str).unwrap_or_default();
            if run_id.is_empty() || name.is_empty() || format.is_empty() {
                continue;
            }

            decoded.push(ImageHistoryPoint {
                ts_ms: parse_i64(arr.get(ts_idx)) / 1_000_000,
                run_id: run_id.to_string(),
                name: name.to_string(),
                format: format.to_string(),
                image_base64: if query.include_data {
                    let image_hex = image_hex_idx
                        .and_then(|index| arr.get(index))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if image_hex.is_empty() {
                        None
                    } else {
                        Some(BASE64_STANDARD.encode(decode_hex(image_hex)?))
                    }
                } else {
                    None
                },
            });
        }

        Ok(finalize_page(decoded, query.cursor, query.limit))
    }

    async fn load_image_frame(&self, query: &ImageFrameQuery) -> Result<Option<ImageFramePayload>, BoxError> {
        let sql = format!(
            "SELECT format, image_hex FROM images WHERE run_id = {} AND name = {} AND ts = {} ORDER BY ts ASC LIMIT 1",
            sql_string_literal(&query.run_id),
            sql_string_literal(&query.name),
            query.ts_ms * 1_000_000
        );
        let resp = query_with_retry_configured(&self.client, &self.sql_url(&sql), RetryConfig::default()).await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let body: Value = resp.json().await?;
        let rows = extract_rows(&body)?;
        let Some(arr) = rows.first().and_then(Value::as_array) else {
            return Ok(None);
        };
        let format = arr.first().and_then(Value::as_str).unwrap_or_default();
        let image_hex = arr.get(1).and_then(Value::as_str).unwrap_or_default();
        if format.is_empty() || image_hex.is_empty() {
            return Ok(None);
        }

        Ok(Some(ImageFramePayload {
            format: format.to_string(),
            image_base64: BASE64_STANDARD.encode(decode_hex(image_hex)?),
        }))
    }

    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        let sql = "SELECT DISTINCT run_id FROM scalars ORDER BY run_id ASC LIMIT 100000";
        let resp = query_with_retry_configured(&self.client, &self.sql_url(sql), RetryConfig::default()).await?;

        if !resp.status().is_success() {
            return Ok(vec![]);
        }

        let body: Value = resp.json().await?;
        let rows = extract_rows(&body)?;
        let mut runs = Vec::new();
        for row in rows {
            let Some(arr) = row.as_array() else {
                continue;
            };
            let Some(run_id) = arr.first().and_then(Value::as_str) else {
                continue;
            };
            if !run_id.is_empty() {
                runs.push(run_id.to_string());
            }
        }
        runs.sort();
        runs.dedup();
        Ok(runs)
    }

    async fn clear_history(&self) -> Result<(), BoxError> {
        let mut failures = Vec::new();
        for table in ["scalars", "images", "hyperparameters"] {
            if let Err(error) = clear_table(&self.client, &self.base_url, table).await {
                failures.push(format!("{}: {}", table, error));
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(boxed_error(format!(
                "GreptimeDB could not clear history: {}",
                failures.join(" | ")
            )))
        }
    }
}

async fn query_with_retry_configured(
    client: &Client,
    url: &str,
    retry_config: RetryConfig,
) -> Result<reqwest::Response, BoxError> {
    let retry_strategy = RetryStrategy::with_config(retry_config);
    let mut last_error: Option<String> = None;

    for attempt in 1..=retry_strategy.max_attempts() {
        match client.get(url).send().await {
            Ok(resp) => return Ok(resp),
            Err(error) => {
                let error_msg = error.to_string();
                last_error = Some(error_msg.clone());
                tracing::error!(
                    "Query connection error (attempt {}/{}): {}",
                    attempt,
                    retry_strategy.max_attempts(),
                    error_msg
                );

                if retry_strategy.should_retry_error(&error_msg) {
                    tokio::time::sleep(retry_strategy.delay_for_attempt(attempt)).await;
                } else {
                    return Err(Box::new(error));
                }
            }
        }
    }

    Err(boxed_error(format!(
        "Query failed after {} attempts: {}",
        retry_strategy.max_attempts(),
        last_error.unwrap_or_else(|| "unknown error".to_string())
    )))
}

async fn clear_table(client: &Client, base_url: &str, table: &str) -> Result<(), String> {
    let statements = [
        format!("DELETE FROM {}", table),
        format!("TRUNCATE TABLE {}", table),
    ];

    for sql in &statements {
        let url_with_sql = format!(
            "{}/v1/sql?sql={}&db=public",
            base_url,
            urlencoding::encode(sql.as_str())
        );
        let resp = match client.get(&url_with_sql).send().await {
            Ok(resp) => resp,
            Err(error) => {
                if error.is_connect() {
                    return Err("GreptimeDB is unavailable".to_string());
                }
                return Err(format!("request failed for `{}`: {}", sql, error));
            }
        };

        if resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
    }

    Err("Unknown error during table deletion".to_string())
}
