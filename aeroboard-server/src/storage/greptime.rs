//! GreptimeDB storage backend (Influx line protocol for writes, SQL over HTTP for reads).
//!
//! Tables are created before the first write (see `table_ddl`) with `run_id` and
//! the stream name as the primary key and `append_mode` on, so points that share
//! a run, metric and millisecond (fast loops logging several steps per ms) are all
//! kept instead of being deduplicated. Tables created by Aeroboard < 0.2 stored
//! `run_id` as a field and must be dropped before upgrading (Delete All in the
//! dashboard does this).

use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde_json::Value;

use crate::{
    history::{HistoryPoint, HyperparameterEntry, ImageHistoryPoint},
    retry::RetryConfig,
    telemetry::TelemetryBatch,
};

use super::{
    BoxError, HistoryQueryOptions, ImageFrameQuery, ImageQueryOptions, MediaKind, MediaPayload,
    MediaSource, QueryPage, StorageEngine, boxed_error,
    codec::{
        decode_hex, extract_column_indices, extract_rows, parse_i64, record_batch_to_line_protocol,
    },
    query::finalize_page,
};

/// Every table Aeroboard writes.
const TABLES: [&str; 5] = ["scalars", "hyperparameters", "images", "audio", "video"];

pub struct GreptimeStore {
    client: Client,
    base_url: String,
    retry: RetryConfig,
    /// Whether `table_ddl` has run since startup / the last clear. Held across
    /// writes so a concurrent `clear_history` can't drop a table mid-write.
    tables_ready: tokio::sync::Mutex<bool>,
}

/// `CREATE TABLE` statement for one of Aeroboard's tables.
fn table_ddl(table: &str) -> String {
    let fields = match table {
        "scalars" => r#""value" DOUBLE NULL, "step" DOUBLE NULL"#.to_string(),
        "hyperparameters" => r#""value" STRING NULL, "value_type" STRING NULL"#.to_string(),
        media => {
            let kind = match media {
                "images" => MediaKind::Image,
                "audio" => MediaKind::Audio,
                _ => MediaKind::Video,
            };
            format!(
                r#""format" STRING NULL, "{}" STRING NULL"#,
                kind.hex_column()
            )
        }
    };
    let key = if table == "hyperparameters" {
        "key"
    } else {
        "name"
    };
    format!(
        r#"CREATE TABLE IF NOT EXISTS "{table}" ("ts" TIMESTAMP(9) NOT NULL, "run_id" STRING NULL, "{key}" STRING NULL, {fields}, TIME INDEX ("ts"), PRIMARY KEY ("run_id", "{key}")) WITH (append_mode = 'true')"#
    )
}

impl GreptimeStore {
    pub fn new(base_url: String) -> Result<Self, BoxError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(5))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            retry: RetryConfig::default(),
            tables_ready: tokio::sync::Mutex::new(false),
        })
    }

    async fn create_tables(&self) -> Result<(), BoxError> {
        for table in TABLES {
            self.sql(&table_ddl(table)).await?;
        }
        Ok(())
    }

    pub(super) async fn write_payload(&self, body: String) -> Result<(), BoxError> {
        let response = self
            .send_with_retry(|| self.client.post(self.write_url()).body(body.clone()))
            .await?;
        if !response.status().is_success() {
            return Err(boxed_error(format!(
                "Greptime write: {}",
                response.text().await?
            )));
        }
        Ok(())
    }
    fn write_url(&self) -> String {
        format!("{}/v1/influxdb/write?db=public&precision=ns", self.base_url)
    }

    fn sql_url(&self) -> String {
        format!("{}/v1/sql?db=public", self.base_url)
    }

    /// Send a request, retrying transport errors, 429 and 5xx with backoff.
    async fn send_with_retry(
        &self,
        build: impl Fn() -> RequestBuilder,
    ) -> Result<reqwest::Response, BoxError> {
        let max_attempts = self.retry.max_attempts.max(1);
        for attempt in 1..=max_attempts {
            let last_attempt = attempt == max_attempts;
            match build().send().await {
                Ok(response) => {
                    let status = response.status();
                    let retryable =
                        status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
                    if !retryable || last_attempt {
                        return Ok(response);
                    }
                    tracing::warn!(
                        "GreptimeDB returned {status} (attempt {attempt}/{max_attempts}); retrying"
                    );
                }
                Err(error) => {
                    let transient = error.is_connect() || error.is_timeout() || error.is_request();
                    if !transient || last_attempt {
                        return Err(if error.is_connect() {
                            boxed_error(format!(
                                "GreptimeDB is unavailable at {}: {error}",
                                self.base_url
                            ))
                        } else {
                            Box::new(error)
                        });
                    }
                    tracing::warn!(
                        "GreptimeDB request failed (attempt {attempt}/{max_attempts}): {error}; retrying"
                    );
                }
            }
            tokio::time::sleep(self.retry.delay_for_attempt(attempt)).await;
        }
        unreachable!("the final attempt always returns")
    }

    /// Run a SQL query. `Ok(None)` means the table doesn't exist yet (nothing of
    /// that kind has been written), which callers treat as "no rows".
    pub(super) async fn sql(&self, sql: &str) -> Result<Option<Value>, BoxError> {
        let url = self.sql_url();
        // POST keeps long `IN (...)` filters out of the URL.
        let response = self
            .send_with_retry(|| self.client.post(&url).form(&[("sql", sql)]))
            .await?;
        let status = response.status();
        if status.is_success() {
            let body: Value = response.json().await?;
            if let Some(error) = body.get("error") {
                if is_missing_table(status, &error.to_string()) {
                    return Ok(None);
                }
                return Err(boxed_error(format!("GreptimeDB SQL error: {error}")));
            }
            return Ok(Some(body));
        }
        let body = response.text().await.unwrap_or_default();
        if is_missing_table(status, &body) {
            return Ok(None);
        }
        Err(boxed_error(format!(
            "GreptimeDB SQL error {status}: {body}"
        )))
    }
}

fn is_missing_table(status: StatusCode, body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    status == StatusCode::NOT_FOUND
        || (body.contains("table") && (body.contains("not found") || body.contains("not exist")))
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
        let joined = run_ids
            .iter()
            .map(|run_id| sql_string_literal(run_id))
            .collect::<Vec<_>>()
            .join(", ");
        clauses.push(format!("run_id IN ({joined})"));
    }

    if !metrics.is_empty() {
        let joined = metrics
            .iter()
            .map(|metric| sql_string_literal(metric))
            .collect::<Vec<_>>()
            .join(", ");
        clauses.push(format!("{metric_column} IN ({joined})"));
    }

    if let Some(start_ms) = start_ms {
        clauses.push(format!("ts >= {}", start_ms.saturating_mul(1_000_000)));
    }

    if let Some(end_ms) = end_ms {
        clauses.push(format!("ts <= {}", end_ms.saturating_mul(1_000_000)));
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
        // One extra row tells `finalize_page` whether another page exists.
        Some(limit) => format!("{sql} LIMIT {} OFFSET {}", limit + 1, offset),
        None if offset > 0 => format!("{sql} LIMIT 9223372036854775807 OFFSET {}", offset),
        None => sql.to_string(),
    }
}

fn as_f64(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

#[async_trait]
impl StorageEngine for GreptimeStore {
    async fn delete_run(&self, run_id: &str) -> Result<(), BoxError> {
        let _guard = self.tables_ready.lock().await;
        for table in TABLES {
            self.sql(&format!(
                "DELETE FROM {table} WHERE run_id={}",
                sql_string_literal(run_id)
            ))
            .await?;
        }
        Ok(())
    }
    async fn append_batch(&self, batch: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        let body = record_batch_to_line_protocol(&batch.batch, &batch.kind)?;
        if body.is_empty() {
            return Ok(());
        }

        let mut tables_ready = self.tables_ready.lock().await;
        if !*tables_ready {
            self.create_tables().await?;
            *tables_ready = true;
        }

        let url = self.write_url();
        let response = self
            .send_with_retry(|| self.client.post(&url).body(body.clone()))
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await.unwrap_or_default();
        let hint = if text.contains("run_id") {
            " (tables created by Aeroboard < 0.2 store run_id as a field; delete history or drop the tables)"
        } else {
            ""
        };
        Err(boxed_error(format!(
            "GreptimeDB write error {status}: {text}{hint}"
        )))
    }

    async fn query_history(
        &self,
        query: &HistoryQueryOptions,
    ) -> Result<QueryPage<HistoryPoint>, BoxError> {
        let where_clause = build_where_clause(
            &query.run_ids,
            &query.metrics,
            query.start_ms,
            query.end_ms,
            "name",
        );
        let sql = apply_page_limit(
            &format!("SELECT * FROM scalars{where_clause} ORDER BY ts ASC, step ASC"),
            query.cursor,
            query.limit,
        );
        let Some(body) = self.sql(&sql).await? else {
            return Ok(QueryPage::default());
        };

        let rows_raw = extract_rows(&body)?;
        let col_idx = extract_column_indices(&body)?;
        let (Some(&ts_idx), Some(&name_idx), Some(&run_id_idx), Some(&value_idx)) = (
            col_idx.get("ts"),
            col_idx.get("name"),
            col_idx.get("run_id"),
            col_idx.get("value"),
        ) else {
            return Ok(QueryPage::default());
        };
        let step_idx = col_idx.get("step").copied();

        let mut points = Vec::with_capacity(rows_raw.len());
        for row in rows_raw {
            let Some(arr) = row.as_array() else {
                continue;
            };
            let name = arr
                .get(name_idx)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let run_id = arr
                .get(run_id_idx)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(value) = as_f64(arr.get(value_idx)) else {
                continue;
            };
            if name.is_empty() || run_id.is_empty() {
                continue;
            }

            points.push(HistoryPoint {
                segment: None,
                ts_ms: parse_i64(arr.get(ts_idx)) / 1_000_000,
                run_id: run_id.to_string(),
                name: name.to_string(),
                value,
                step: step_idx.and_then(|index| as_f64(arr.get(index))),
            });
        }

        Ok(finalize_page(points, query.cursor, query.limit))
    }

    async fn query_hyperparameters(
        &self,
        run_ids: &BTreeSet<String>,
    ) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        let where_clause = build_where_clause(run_ids, &BTreeSet::new(), None, None, "key");
        let sql = format!(
            "SELECT run_id, key, value, value_type FROM hyperparameters{where_clause} ORDER BY ts ASC"
        );
        let Some(body) = self.sql(&sql).await? else {
            return Ok(vec![]);
        };

        let mut entries = Vec::new();
        for row in extract_rows(&body)? {
            let Some(arr) = row.as_array() else {
                continue;
            };
            let run_id = arr.first().and_then(Value::as_str).unwrap_or_default();
            let key = arr.get(1).and_then(Value::as_str).unwrap_or_default();
            if run_id.is_empty() || key.is_empty() {
                continue;
            }

            entries.push((
                run_id.to_string(),
                HyperparameterEntry {
                    key: key.to_string(),
                    value: arr
                        .get(2)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    value_type: arr
                        .get(3)
                        .and_then(Value::as_str)
                        .unwrap_or("string")
                        .to_string(),
                },
            ));
        }

        Ok(entries)
    }

    async fn query_media_history(
        &self,
        kind: MediaKind,
        query: &ImageQueryOptions,
    ) -> Result<QueryPage<ImageHistoryPoint>, BoxError> {
        let where_clause = build_where_clause(
            &query.run_ids,
            &query.metrics,
            query.start_ms,
            query.end_ms,
            "name",
        );
        let (table, hex_column) = (kind.dir_name(), kind.hex_column());
        let columns = if query.include_data {
            format!("ts, run_id, name, format, {hex_column}")
        } else {
            "ts, run_id, name, format".to_string()
        };
        let sql = apply_page_limit(
            &format!("SELECT {columns} FROM {table}{where_clause} ORDER BY ts ASC"),
            query.cursor,
            query.limit,
        );
        let Some(body) = self.sql(&sql).await? else {
            return Ok(QueryPage::default());
        };

        let mut decoded = Vec::new();
        for row in extract_rows(&body)? {
            let Some(arr) = row.as_array() else {
                continue;
            };

            let run_id = arr.get(1).and_then(Value::as_str).unwrap_or_default();
            let name = arr.get(2).and_then(Value::as_str).unwrap_or_default();
            let format = arr.get(3).and_then(Value::as_str).unwrap_or_default();
            if run_id.is_empty() || name.is_empty() || format.is_empty() {
                continue;
            }

            let image_base64 = match arr.get(4).and_then(Value::as_str) {
                Some(hex) if query.include_data && !hex.is_empty() => {
                    Some(BASE64_STANDARD.encode(decode_hex(hex)?))
                }
                _ => None,
            };
            decoded.push(ImageHistoryPoint {
                asset_id: None,
                step: None,
                ts_ms: parse_i64(arr.first()) / 1_000_000,
                run_id: run_id.to_string(),
                name: name.to_string(),
                format: format.to_string(),
                image_base64,
            });
        }

        Ok(finalize_page(decoded, query.cursor, query.limit))
    }

    async fn load_media(
        &self,
        kind: MediaKind,
        query: &ImageFrameQuery,
    ) -> Result<Option<MediaPayload>, BoxError> {
        let sql = format!(
            "SELECT format, {} FROM {} WHERE run_id = {} AND name = {} AND ts = {} LIMIT 1",
            kind.hex_column(),
            kind.dir_name(),
            sql_string_literal(&query.run_id),
            sql_string_literal(&query.name),
            query.ts_ms.saturating_mul(1_000_000)
        );
        let Some(body) = self.sql(&sql).await? else {
            return Ok(None);
        };

        let rows = extract_rows(&body)?;
        let Some(arr) = rows.first().and_then(Value::as_array) else {
            return Ok(None);
        };
        let format = arr.first().and_then(Value::as_str).unwrap_or_default();
        let hex = arr.get(1).and_then(Value::as_str).unwrap_or_default();
        if format.is_empty() || hex.is_empty() {
            return Ok(None);
        }

        Ok(Some(MediaPayload {
            format: format.to_string(),
            source: MediaSource::Bytes(decode_hex(hex)?),
        }))
    }

    /// Runs that have any persisted data, not just scalars.
    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        let mut runs = BTreeSet::new();
        let queries = TABLES.map(|table| format!("SELECT DISTINCT run_id FROM {table}"));
        let responses = futures::future::join_all(queries.iter().map(|sql| self.sql(sql))).await;
        for response in responses {
            let Some(body) = response? else {
                continue;
            };
            for row in extract_rows(&body)? {
                if let Some(run_id) = row
                    .as_array()
                    .and_then(|arr| arr.first())
                    .and_then(Value::as_str)
                    && !run_id.is_empty()
                {
                    runs.insert(run_id.to_string());
                }
            }
        }
        Ok(runs.into_iter().collect())
    }

    async fn clear_history(&self) -> Result<(), BoxError> {
        let mut tables_ready = self.tables_ready.lock().await;
        *tables_ready = false;
        let mut failures = Vec::new();
        for table in TABLES {
            // Dropping (rather than deleting rows) also clears tables created by
            // older versions with an incompatible column layout.
            if let Err(error) = self.sql(&format!("DROP TABLE IF EXISTS {table}")).await {
                failures.push(format!("{table}: {error}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_limit_requests_one_extra_row() {
        assert_eq!(
            apply_page_limit("SELECT 1", Some(10), Some(5)),
            "SELECT 1 LIMIT 6 OFFSET 10"
        );
        assert_eq!(apply_page_limit("SELECT 1", None, None), "SELECT 1");
    }

    #[test]
    fn where_clause_quotes_literals() {
        let runs = BTreeSet::from(["o'brien".to_string()]);
        let clause = build_where_clause(&runs, &BTreeSet::new(), Some(1), None, "name");
        assert_eq!(clause, " WHERE run_id IN ('o''brien') AND ts >= 1000000");
    }

    #[test]
    fn table_ddl_keys_on_run_and_stream_in_append_mode() {
        let ddl = table_ddl("hyperparameters");
        assert!(ddl.contains(r#"PRIMARY KEY ("run_id", "key")"#), "{ddl}");
        assert!(table_ddl("video").contains(r#""video_hex" STRING"#));
        assert!(table_ddl("scalars").ends_with("WITH (append_mode = 'true')"));
    }

    #[test]
    fn missing_tables_are_recognised() {
        assert!(is_missing_table(
            StatusCode::BAD_REQUEST,
            "Table not found: greptime.public.audio"
        ));
        assert!(!is_missing_table(StatusCode::BAD_REQUEST, "syntax error"));
    }
}
