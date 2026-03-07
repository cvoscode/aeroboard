use std::{
    collections::HashMap,
    fs,
    io,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use arrow::{
    array::{Array, AsArray},
    ipc::{reader::FileReader, writer::FileWriter},
    record_batch::RecordBatch,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    config::{AppConfig, StorageMode},
    history::{HistoryPoint, HyperparameterEntry, ImageHistoryPoint},
    telemetry::{TelemetryBatch, TelemetryKind},
};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait StorageEngine: Send + Sync {
    async fn append_batch(&self, batch: Arc<TelemetryBatch>) -> Result<(), BoxError>;
    async fn query_history(&self) -> Result<Vec<HistoryPoint>, BoxError>;
    async fn query_hyperparameters(&self) -> Result<Vec<(String, HyperparameterEntry)>, BoxError>;
    async fn query_image_history(&self) -> Result<Vec<ImageHistoryPoint>, BoxError>;
    async fn list_runs(&self) -> Result<Vec<String>, BoxError>;
    async fn clear_history(&self) -> Result<(), BoxError>;
}

pub fn build_storage(config: &AppConfig) -> Result<Arc<dyn StorageEngine>, BoxError> {
    match config.storage_mode {
        StorageMode::Filesystem => Ok(Arc::new(FilesystemStore::new(config.data_dir.clone())?)),
        StorageMode::Greptime => Ok(Arc::new(GreptimeStore::new(config.greptime_url.clone()))),
        StorageMode::LiveOnly => Ok(Arc::new(LiveOnlyStore)),
    }
}

pub struct LiveOnlyStore;

#[async_trait]
impl StorageEngine for LiveOnlyStore {
    async fn append_batch(&self, _batch: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn query_history(&self) -> Result<Vec<HistoryPoint>, BoxError> {
        Ok(vec![])
    }

    async fn query_hyperparameters(&self) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        Ok(vec![])
    }

    async fn query_image_history(&self) -> Result<Vec<ImageHistoryPoint>, BoxError> {
        Ok(vec![])
    }

    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        Ok(vec![])
    }

    async fn clear_history(&self) -> Result<(), BoxError> {
        Err(boxed_error("live-only mode does not support deleting persisted history"))
    }
}

pub struct FilesystemStore {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct RunManifest {
    run_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct ImageIndexEntry {
    ts_ms: i64,
    run_id: String,
    name: String,
    format: String,
    file_name: String,
}

impl FilesystemStore {
    pub fn new(root: PathBuf) -> Result<Self, BoxError> {
        fs::create_dir_all(root.join("runs"))?;
        Ok(Self { root })
    }

    fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    fn run_dir_for(&self, run_id: &str) -> PathBuf {
        self.runs_dir().join(urlencoding::encode(run_id).into_owned())
    }

    fn image_metric_dir(&self, run_id: &str, metric_name: &str) -> PathBuf {
        self.run_dir_for(run_id)
            .join("images")
            .join(urlencoding::encode(metric_name).into_owned())
    }

    fn write_manifest(&self, run_id: &str) -> Result<(), BoxError> {
        let run_dir = self.run_dir_for(run_id);
        fs::create_dir_all(&run_dir)?;
        let manifest_path = run_dir.join("manifest.json");
        if manifest_path.exists() {
            return Ok(());
        }

        let manifest = RunManifest {
            run_id: run_id.to_string(),
        };
        fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
        Ok(())
    }

    fn batch_file_paths(&self, kind_dir: &str) -> Result<Vec<PathBuf>, BoxError> {
        let mut files = Vec::new();
        let runs_dir = self.runs_dir();
        if !runs_dir.exists() {
            return Ok(files);
        }

        for run_entry in fs::read_dir(runs_dir)? {
            let run_entry = run_entry?;
            if !run_entry.file_type()?.is_dir() {
                continue;
            }

            let kind_path = run_entry.path().join(kind_dir);
            if !kind_path.exists() {
                continue;
            }

            for file_entry in fs::read_dir(kind_path)? {
                let file_entry = file_entry?;
                if file_entry.file_type()?.is_file()
                    && file_entry.path().extension().and_then(|ext| ext.to_str()) == Some("arrow")
                {
                    files.push(file_entry.path());
                }
            }
        }

        files.sort();
        Ok(files)
    }

    fn read_batches(&self, kind_dir: &str) -> Result<Vec<RecordBatch>, BoxError> {
        let mut batches = Vec::new();
        for path in self.batch_file_paths(kind_dir)? {
            let file = fs::File::open(&path)?;
            let reader = FileReader::try_new(file, None)?;
            for batch in reader {
                batches.push(batch?);
            }
        }
        Ok(batches)
    }

    fn append_image_batch(&self, batch: &RecordBatch) -> Result<(), BoxError> {
        let timestamp_col = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
        let run_id_col = batch.column(1).as_string::<i32>();
        let name_col = batch.column(2).as_string::<i32>();
        let image_col = batch.column(3).as_binary::<i32>();
        let format_col = batch.column(4).as_string::<i32>();

        for i in 0..batch.num_rows() {
            if timestamp_col.is_null(i)
                || run_id_col.is_null(i)
                || name_col.is_null(i)
                || image_col.is_null(i)
                || format_col.is_null(i)
            {
                continue;
            }

            let ts_ms = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let name = name_col.value(i);
            let format = sanitize_extension(format_col.value(i));
            let bytes = image_col.value(i);

            self.write_manifest(run_id)?;
            let metric_dir = self.image_metric_dir(run_id, name);
            fs::create_dir_all(&metric_dir)?;

            let file_name = format!("{}-{}.{}", ts_ms, Uuid::new_v4(), format);
            let file_path = metric_dir.join(&file_name);
            fs::write(&file_path, bytes)?;

            let index_path = metric_dir.join("index.json");
            let mut index = self.read_image_index(&index_path)?;
            index.push(ImageIndexEntry {
                ts_ms,
                run_id: run_id.to_string(),
                name: name.to_string(),
                format,
                file_name,
            });
            index.sort_by_key(|entry| entry.ts_ms);
            fs::write(index_path, serde_json::to_vec_pretty(&index)?)?;
        }

        Ok(())
    }

    fn read_image_index(&self, path: &PathBuf) -> Result<Vec<ImageIndexEntry>, BoxError> {
        if !path.exists() {
            return Ok(vec![]);
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn query_image_history_from_files(&self) -> Result<Vec<ImageHistoryPoint>, BoxError> {
        let mut rows = Vec::new();
        let runs_dir = self.runs_dir();
        if !runs_dir.exists() {
            return Ok(rows);
        }

        for run_entry in fs::read_dir(runs_dir)? {
            let run_entry = run_entry?;
            if !run_entry.file_type()?.is_dir() {
                continue;
            }

            let images_dir = run_entry.path().join("images");
            if !images_dir.exists() {
                continue;
            }

            for metric_entry in fs::read_dir(images_dir)? {
                let metric_entry = metric_entry?;
                if !metric_entry.file_type()?.is_dir() {
                    continue;
                }

                let metric_dir = metric_entry.path();
                let index_path = metric_dir.join("index.json");
                let entries = self.read_image_index(&index_path)?;
                for entry in entries {
                    let file_path = metric_dir.join(&entry.file_name);
                    if !file_path.exists() {
                        continue;
                    }

                    let bytes = fs::read(file_path)?;
                    rows.push(ImageHistoryPoint {
                        ts_ms: entry.ts_ms,
                        run_id: entry.run_id,
                        name: entry.name,
                        format: entry.format,
                        image_base64: BASE64_STANDARD.encode(bytes),
                    });
                }
            }
        }

        rows.sort_by_key(|row| row.ts_ms);
        Ok(rows)
    }
}

#[async_trait]
impl StorageEngine for FilesystemStore {
    async fn append_batch(&self, batch: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        let kind = &batch.kind;
        let Some(kind_dir) = kind.as_dir_name() else {
            tracing::debug!("Skipping unsupported filesystem batch kind: {:?}", kind);
            return Ok(());
        };

        if *kind == TelemetryKind::Images {
            return self.append_image_batch(&batch.batch);
        }

        let run_id = extract_first_run_id(&batch.batch).unwrap_or_else(|| "unknown_run".to_string());
        self.write_manifest(&run_id)?;

        let output_dir = self.run_dir_for(&run_id).join(kind_dir);
        fs::create_dir_all(&output_dir)?;

        let file_name = format!("{}-{}.arrow", now_ms(), Uuid::new_v4());
        let file_path = output_dir.join(file_name);
        let file = fs::File::create(file_path)?;
        let mut writer = FileWriter::try_new(file, &batch.batch.schema())?;
        writer.write(&batch.batch)?;
        writer.finish()?;

        Ok(())
    }

    async fn query_history(&self) -> Result<Vec<HistoryPoint>, BoxError> {
        let mut rows = Vec::new();
        for batch in self.read_batches("scalars")? {
            rows.extend(parse_scalar_batch(&batch));
        }
        rows.sort_by_key(|row| row.ts_ms);
        Ok(rows)
    }

    async fn query_hyperparameters(&self) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        let mut rows = Vec::new();
        for batch in self.read_batches("hyperparameters")? {
            rows.extend(parse_hparam_batch(&batch));
        }
        Ok(rows)
    }

    async fn query_image_history(&self) -> Result<Vec<ImageHistoryPoint>, BoxError> {
        let mut rows = self.query_image_history_from_files()?;
        for batch in self.read_batches("images")? {
            rows.extend(parse_image_batch(&batch));
        }
        rows.sort_by_key(|row| row.ts_ms);
        Ok(rows)
    }

    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        let mut runs = Vec::new();
        let runs_dir = self.runs_dir();
        if !runs_dir.exists() {
            return Ok(runs);
        }

        for run_entry in fs::read_dir(runs_dir)? {
            let run_entry = run_entry?;
            if !run_entry.file_type()?.is_dir() {
                continue;
            }

            let manifest_path = run_entry.path().join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }

            let manifest: RunManifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
            runs.push(manifest.run_id);
        }

        runs.sort();
        runs.dedup();
        Ok(runs)
    }

    async fn clear_history(&self) -> Result<(), BoxError> {
        let runs_dir = self.runs_dir();
        if runs_dir.exists() {
            fs::remove_dir_all(&runs_dir)?;
        }
        fs::create_dir_all(runs_dir)?;
        Ok(())
    }
}

pub struct GreptimeStore {
    client: reqwest::Client,
    base_url: String,
}

impl GreptimeStore {
    pub fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn write_url(&self) -> String {
        format!("{}/v1/influxdb/write?db=public", self.base_url)
    }

    fn sql_url(&self, sql: &str) -> String {
        format!("{}/v1/sql?sql={}&db=public", self.base_url, urlencoding::encode(sql))
    }
}

#[async_trait]
impl StorageEngine for GreptimeStore {
    async fn append_batch(&self, batch: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        let line_protocol_data = record_batch_to_line_protocol(&batch.batch, &batch.kind)?;
        if line_protocol_data.is_empty() {
            return Ok(());
        }

        let response = self.client.post(self.write_url()).body(line_protocol_data).send().await?;
        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        Err(boxed_error(format!("Failed to insert to GreptimeDB: {} - {}", status, text)))
    }

    async fn query_history(&self) -> Result<Vec<HistoryPoint>, BoxError> {
        let sql = "SELECT * FROM scalars ORDER BY ts ASC LIMIT 1000000";
        let resp = match self.client.get(self.sql_url(sql)).send().await {
            Ok(resp) => resp,
            Err(error) if error.is_connect() => return Ok(vec![]),
            Err(error) => return Err(Box::new(error)),
        };

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(boxed_error(format!("GreptimeDB SQL error {}: {}", status, body)));
        }

        let body: Value = resp.json().await?;
        let rows_raw = extract_rows(&body)?;
        let col_idx = extract_column_indices(&body)?;

        let ts_idx = col_idx.get("ts");
        let name_idx = col_idx.get("metric").or_else(|| col_idx.get("name"));
        let run_id_idx = col_idx.get("run_id");
        let value_idx = col_idx.get("value");

        let (Some(&ts_idx), Some(&name_idx), Some(&value_idx)) = (ts_idx, name_idx, value_idx) else {
            return Ok(vec![]);
        };

        let mut points = Vec::with_capacity(rows_raw.len());
        for row in rows_raw {
            let Some(arr) = row.as_array() else { continue; };
            if arr.len() <= ts_idx || arr.len() <= name_idx || arr.len() <= value_idx {
                continue;
            }

            let ts_ns = parse_i64(arr.get(ts_idx));
            let name = arr.get(name_idx).and_then(Value::as_str).unwrap_or_default().to_string();
            let run_id = run_id_idx
                .and_then(|&i| arr.get(i))
                .and_then(Value::as_str)
                .unwrap_or("unknown_run")
                .to_string();
            let value = match &arr[value_idx] {
                Value::Number(n) => n.as_f64().unwrap_or(0.0),
                _ => 0.0,
            };

            if name.is_empty() {
                continue;
            }

            points.push(HistoryPoint {
                ts_ms: ts_ns / 1_000_000,
                run_id,
                name,
                value,
            });
        }

        Ok(points)
    }

    async fn query_hyperparameters(&self) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        let sql = "SELECT * FROM hyperparameters ORDER BY ts ASC LIMIT 100000";
        let resp = match self.client.get(self.sql_url(sql)).send().await {
            Ok(resp) => resp,
            Err(error) if error.is_connect() => return Ok(vec![]),
            Err(error) => return Err(Box::new(error)),
        };

        if !resp.status().is_success() {
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
        let (Some(&_ts_idx), Some(&run_id_idx), Some(&key_idx), Some(&value_idx), Some(&value_type_idx)) =
            (ts_idx, run_id_idx, key_idx, value_idx, value_type_idx)
        else {
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

    async fn query_image_history(&self) -> Result<Vec<ImageHistoryPoint>, BoxError> {
        let sql = "SELECT * FROM images ORDER BY ts ASC LIMIT 100000";
        let resp = match self.client.get(self.sql_url(sql)).send().await {
            Ok(resp) => resp,
            Err(error) if error.is_connect() => return Ok(vec![]),
            Err(error) => return Err(Box::new(error)),
        };

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
        let image_hex_idx = col_idx.get("image_hex");
        let (Some(&ts_idx), Some(&run_id_idx), Some(&name_idx), Some(&format_idx), Some(&image_hex_idx)) =
            (ts_idx, run_id_idx, name_idx, format_idx, image_hex_idx)
        else {
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

    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        let sql = "SELECT DISTINCT run_id FROM scalars ORDER BY run_id ASC LIMIT 100000";
        let resp = match self.client.get(self.sql_url(sql)).send().await {
            Ok(resp) => resp,
            Err(error) if error.is_connect() => return Ok(vec![]),
            Err(error) => return Err(Box::new(error)),
        };

        if !resp.status().is_success() {
            return Ok(vec![]);
        }

        let body: Value = resp.json().await?;
        let rows = extract_rows(&body)?;
        let mut runs = Vec::new();
        for row in rows {
            let Some(arr) = row.as_array() else { continue; };
            let Some(run_id) = arr.first().and_then(Value::as_str) else { continue; };
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
            match clear_table(&self.client, &self.base_url, table).await {
                Ok(ClearTableOutcome::Cleared) | Ok(ClearTableOutcome::SkippedMissing) => {}
                Ok(ClearTableOutcome::PersistenceUnavailable) => {
                    return Err(boxed_error("GreptimeDB is unavailable; persisted history could not be deleted"));
                }
                Err(error) => failures.push(format!("{}: {}", table, error)),
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(boxed_error(format!("GreptimeDB could not clear history: {}", failures.join(" | "))))
        }
    }
}

pub fn record_batch_to_line_protocol(batch: &RecordBatch, kind: &TelemetryKind) -> Result<String, BoxError> {
    let mut lines = String::new();
    let num_rows = batch.num_rows();

    if num_rows == 0 {
        return Ok(lines);
    }

    let timestamp_col = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let run_id_col = batch.column(1).as_string::<i32>();
    let text_col = batch.column(2).as_string::<i32>();

    if *kind == TelemetryKind::Scalars {
        let value_col = batch.column(3).as_primitive::<arrow::datatypes::Float64Type>();

        for i in 0..num_rows {
            if timestamp_col.is_null(i) || run_id_col.is_null(i) || text_col.is_null(i) || value_col.is_null(i) {
                continue;
            }

            let ts = timestamp_col.value(i);
            let run_id = run_id_col.value(i);
            let name = text_col.value(i);
            let val = value_col.value(i);

            let safe_name = name.replace(' ', "\\ ").replace(',', "\\,");
            let safe_run_id = run_id.replace(' ', "\\ ").replace(',', "\\,").replace('"', "\\\"");

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
    } else {
        tracing::debug!("Skipping distribution persistence via InfluxDB line protocol; requires JSON API or flattened schema.");
    }

    Ok(lines)
}

fn extract_first_run_id(batch: &RecordBatch) -> Option<String> {
    let run_id_col = batch.column(1).as_string::<i32>();
    (0..batch.num_rows())
        .find(|&index| !run_id_col.is_null(index))
        .map(|index| run_id_col.value(index).to_string())
}

fn parse_scalar_batch(batch: &RecordBatch) -> Vec<HistoryPoint> {
    let timestamp_col = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let run_id_col = batch.column(1).as_string::<i32>();
    let name_col = batch.column(2).as_string::<i32>();
    let value_col = batch.column(3).as_primitive::<arrow::datatypes::Float64Type>();
    let mut rows = Vec::with_capacity(batch.num_rows());

    for i in 0..batch.num_rows() {
        if timestamp_col.is_null(i) || run_id_col.is_null(i) || name_col.is_null(i) || value_col.is_null(i) {
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

fn parse_hparam_batch(batch: &RecordBatch) -> Vec<(String, HyperparameterEntry)> {
    let run_id_col = batch.column(1).as_string::<i32>();
    let key_col = batch.column(2).as_string::<i32>();
    let value_col = batch.column(3).as_string::<i32>();
    let value_type_col = batch.column(4).as_string::<i32>();
    let mut rows = Vec::with_capacity(batch.num_rows());

    for i in 0..batch.num_rows() {
        if run_id_col.is_null(i) || key_col.is_null(i) || value_col.is_null(i) || value_type_col.is_null(i) {
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

fn parse_image_batch(batch: &RecordBatch) -> Vec<ImageHistoryPoint> {
    let timestamp_col = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
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
            image_base64: BASE64_STANDARD.encode(image_col.value(i)),
        });
    }

    rows
}

fn sanitize_extension(format: &str) -> String {
    let normalized = format.trim().trim_start_matches('.').to_ascii_lowercase();
    if normalized.is_empty() {
        return "bin".to_string();
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
    value.replace(' ', "\\ ").replace(',', "\\,").replace('=', "\\=")
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

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn boxed_error(message: impl Into<String>) -> BoxError {
    Box::new(io::Error::other(message.into()))
}

fn extract_rows(body: &Value) -> Result<&Vec<Value>, BoxError> {
    body.get("output")
        .and_then(|o| o.get(0))
        .and_then(|o| o.get("records"))
        .and_then(|r| r.get("rows"))
        .and_then(Value::as_array)
        .ok_or_else(|| boxed_error("Unexpected GreptimeDB response shape"))
}

fn extract_column_indices(body: &Value) -> Result<HashMap<&str, usize>, BoxError> {
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

fn parse_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, BoxError> {
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

enum ClearTableOutcome {
    Cleared,
    SkippedMissing,
    PersistenceUnavailable,
}

async fn clear_table(
    client: &reqwest::Client,
    base_url: &str,
    table: &str,
) -> Result<ClearTableOutcome, String> {
    let statements = [
        format!("DELETE FROM {}", table),
        format!("TRUNCATE TABLE {}", table),
        format!("DELETE FROM {} WHERE true", table),
    ];

    let mut last_error = String::new();

    for sql in statements {
        let url = format!("{}/v1/sql?sql={}&db=public", base_url, urlencoding::encode(&sql));
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
        let body = resp.text().await.unwrap_or_default();
        if is_missing_table_error(status, &body) {
            return Ok(ClearTableOutcome::SkippedMissing);
        }

        last_error = format!("statement `{}` failed with {}: {}", sql, status, body);
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
