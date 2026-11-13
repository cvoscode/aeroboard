//! Payload layout is independent of receipt identity and visibility.
use super::{BoxError, boxed_error};
use crate::config::{AppConfig, StorageMode};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, Write},
    path::PathBuf,
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    pub offset: u64,
    pub length: u64,
}
pub struct Payload {
    pub id: String,
    pub uid: String,
    pub kind: String,
    pub bytes: Vec<u8>,
}
#[async_trait]
pub trait PayloadBackend: Send + Sync {
    async fn put_group(
        &self,
        items: Vec<Payload>,
    ) -> Result<Vec<(String, Option<Location>)>, BoxError>;
    async fn read(&self, id: &str, location: Option<Location>) -> Result<Vec<u8>, BoxError>;
    async fn read_group(
        &self,
        items: Vec<(String, Option<Location>)>,
    ) -> Result<Vec<Vec<u8>>, BoxError> {
        let mut result = Vec::with_capacity(items.len());
        for (id, location) in items {
            result.push(self.read(&id, location).await?);
        }
        Ok(result)
    }
    async fn delete(&self, id: &str) -> Result<(), BoxError>;
    async fn delete_legacy(&self, id: &str) -> Result<(), BoxError> {
        self.delete(id).await
    }
    async fn delete_run(&self, _uid: &str) -> Result<(), BoxError> {
        Ok(())
    }
    async fn clear(&self) -> Result<(), BoxError>;
}
pub fn build(config: &AppConfig) -> Arc<dyn PayloadBackend> {
    match config.storage_mode {
        StorageMode::LiveOnly => Arc::new(LivePayload),
        StorageMode::Greptime => Arc::new(GreptimePayload {
            store: super::GreptimeStore::new(config.greptime_url.clone())
                .expect("valid HTTP configuration"),
            ready: tokio::sync::Mutex::new(false),
            media: Segments {
                root: config.data_dir.clone(),
            },
        }),
        _ => Arc::new(Segments {
            root: config.data_dir.clone(),
        }),
    }
}
struct LivePayload;
#[async_trait]
impl PayloadBackend for LivePayload {
    async fn put_group(
        &self,
        items: Vec<Payload>,
    ) -> Result<Vec<(String, Option<Location>)>, BoxError> {
        Ok(items.into_iter().map(|item| (item.id, None)).collect())
    }
    async fn read(&self, _id: &str, _location: Option<Location>) -> Result<Vec<u8>, BoxError> {
        Err(boxed_error("live-only payload has expired"))
    }
    async fn delete(&self, _id: &str) -> Result<(), BoxError> {
        Ok(())
    }
    async fn clear(&self) -> Result<(), BoxError> {
        Ok(())
    }
}
struct Segments {
    root: PathBuf,
}
const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
#[async_trait]
impl PayloadBackend for Segments {
    async fn put_group(
        &self,
        items: Vec<Payload>,
    ) -> Result<Vec<(String, Option<Location>)>, BoxError> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let mut groups: BTreeMap<(String, String), Vec<Payload>> = BTreeMap::new();
            for item in items {
                groups
                    .entry((item.uid.clone(), item.kind.clone()))
                    .or_default()
                    .push(item);
            }
            let mut locations = vec![];
            for ((uid, kind), items) in groups {
                let directory = root.join("segments").join(&uid).join(&kind);
                std::fs::create_dir_all(&directory)?;
                let mut number = std::fs::read_dir(&directory)?
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.path().file_stem()?.to_str()?.parse::<u64>().ok())
                    .max()
                    .unwrap_or(0);
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(directory.join(format!("{number:08}.segment")))?;
                let mut offset = file.metadata()?.len();
                for item in items {
                    if offset > 0 && offset + item.bytes.len() as u64 + 8 > SEGMENT_BYTES {
                        file.sync_all()?;
                        number += 1;
                        file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(directory.join(format!("{number:08}.segment")))?;
                        offset = 0;
                    }
                    let length = item.bytes.len() as u64;
                    file.write_all(&length.to_le_bytes())?;
                    file.write_all(&item.bytes)?;
                    locations.push((
                        item.id,
                        Some(Location {
                            path: format!("segments/{uid}/{kind}/{number:08}.segment"),
                            offset: offset + 8,
                            length,
                        }),
                    ));
                    offset += length + 8;
                }
                file.sync_all()?;
                // Persist each newly created directory entry before committing its receipt.
                #[cfg(unix)]
                for path in [
                    &directory,
                    &root.join("segments").join(&uid),
                    &root.join("segments"),
                    &root,
                ] {
                    std::fs::File::open(path)?.sync_all()?;
                }
            }
            Ok::<_, BoxError>(locations)
        })
        .await?
    }
    async fn read_group(
        &self,
        items: Vec<(String, Option<Location>)>,
    ) -> Result<Vec<Vec<u8>>, BoxError> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let mut files: BTreeMap<String, std::fs::File> = BTreeMap::new();
            let mut result = Vec::with_capacity(items.len());
            for (id, location) in items {
                if let Some(location) = location {
                    if !files.contains_key(&location.path) {
                        files.insert(
                            location.path.clone(),
                            std::fs::File::open(root.join(&location.path))?,
                        );
                    }
                    let file = files.get_mut(&location.path).unwrap();
                    file.seek(std::io::SeekFrom::Start(location.offset))?;
                    let mut bytes = vec![0; usize::try_from(location.length)?];
                    file.read_exact(&mut bytes)?;
                    result.push(bytes);
                } else {
                    result.push(std::fs::read(
                        root.join("batches").join(format!("{id}.arrow")),
                    )?);
                }
            }
            Ok::<_, BoxError>(result)
        })
        .await?
    }
    async fn read(&self, id: &str, location: Option<Location>) -> Result<Vec<u8>, BoxError> {
        let root = self.root.clone();
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            if let Some(location) = location {
                let mut file = std::fs::File::open(root.join(location.path))?;
                file.seek(std::io::SeekFrom::Start(location.offset))?;
                let mut bytes = vec![0; usize::try_from(location.length)?];
                file.read_exact(&mut bytes)?;
                Ok(bytes)
            } else {
                Ok(std::fs::read(
                    root.join("batches").join(format!("{id}.arrow")),
                )?)
            }
        })
        .await?
    }
    async fn delete(&self, id: &str) -> Result<(), BoxError> {
        match tokio::fs::remove_file(self.root.join("batches").join(format!("{id}.arrow"))).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    async fn delete_run(&self, uid: &str) -> Result<(), BoxError> {
        match tokio::fs::remove_dir_all(self.root.join("segments").join(uid)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    async fn clear(&self) -> Result<(), BoxError> {
        for name in ["segments", "batches"] {
            match tokio::fs::remove_dir_all(self.root.join(name)).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
struct GreptimePayload {
    store: super::GreptimeStore,
    ready: tokio::sync::Mutex<bool>,
    media: Segments,
}
#[async_trait]
impl PayloadBackend for GreptimePayload {
    async fn put_group(
        &self,
        items: Vec<Payload>,
    ) -> Result<Vec<(String, Option<Location>)>, BoxError> {
        use arrow::{
            array::{Array, AsArray},
            ipc::reader::StreamReader,
        };
        let mut ready = self.ready.lock().await;
        if !*ready {
            self.store.sql(r#"CREATE TABLE IF NOT EXISTS aeroboard_scalars ("ts" TIMESTAMP(9) NOT NULL, "batch_id" STRING, "row_index" STRING, "run_uid" STRING, "run_id" STRING, "name" STRING, "value" DOUBLE, "value_bits" STRING, "step" DOUBLE, "segment" STRING, TIME INDEX("ts"), PRIMARY KEY("batch_id","row_index","run_uid","run_id","name")) WITH (append_mode='false')"#).await?;
            self.store.sql(r#"CREATE TABLE IF NOT EXISTS aeroboard_hyperparameters ("ts" TIMESTAMP(9) NOT NULL, "batch_id" STRING, "row_index" STRING, "run_uid" STRING, "run_id" STRING, "name" STRING, "value" STRING, "value_type" STRING, TIME INDEX("ts"), PRIMARY KEY("batch_id","row_index","run_uid","run_id","name")) WITH (append_mode='false')"#).await?;
            *ready = true;
        }
        drop(ready);
        let mut body = String::new();
        let mut result = vec![];
        let mut media = vec![];
        for item in items {
            if !["scalars", "hyperparameters"].contains(&item.kind.as_str()) {
                media.push(item);
                continue;
            }
            let mut reader = StreamReader::try_new(std::io::Cursor::new(item.bytes), None)?;
            let batch = reader
                .next()
                .ok_or_else(|| boxed_error("empty native batch"))??;
            let ts = batch
                .column(0)
                .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
            let runs = batch.column(1).as_string::<i32>();
            let names = batch.column(2).as_string::<i32>();
            let envelope = batch
                .schema()
                .metadata()
                .get(crate::telemetry::ENVELOPE_KEY)
                .cloned()
                .and_then(|raw| serde_json::from_str::<crate::telemetry::BatchEnvelope>(&raw).ok());
            use std::fmt::Write;
            for row in 0..batch.num_rows() {
                let tags = format!(
                    "batch_id={},row_index={},run_uid={},run_id={},name={}",
                    item.id,
                    row,
                    item.uid,
                    escape_tag(runs.value(row)),
                    escape_tag(names.value(row))
                );
                if item.kind == "scalars" {
                    let value = batch
                        .column(3)
                        .as_primitive::<arrow::datatypes::Float64Type>()
                        .value(row);
                    let steps = batch
                        .column(4)
                        .as_primitive::<arrow::datatypes::Float64Type>();
                    let step = if steps.is_null(row) {
                        String::new()
                    } else {
                        format!(",step={}", steps.value(row))
                    };
                    let segment = envelope
                        .as_ref()
                        .and_then(|e| e.segment.as_deref())
                        .map(|s| format!(",segment={}", escape_field(s)))
                        .unwrap_or_default();
                    let numeric = if value.is_finite() {
                        format!(",value={value}")
                    } else {
                        String::new()
                    };
                    writeln!(
                        &mut body,
                        "aeroboard_scalars,{tags} value_bits=\"{:016x}\"{numeric}{step}{segment} {}",
                        value.to_bits(),
                        ts.value(row).saturating_mul(1_000_000)
                    )?;
                } else {
                    writeln!(
                        &mut body,
                        "aeroboard_hyperparameters,{tags} value={},value_type={} {}",
                        escape_field(batch.column(3).as_string::<i32>().value(row)),
                        escape_field(batch.column(4).as_string::<i32>().value(row)),
                        ts.value(row).saturating_mul(1_000_000)
                    )?;
                }
            }
            result.push((
                item.id,
                Some(Location {
                    path: format!("greptime/{}", item.kind),
                    offset: 0,
                    length: 0,
                }),
            ));
        }
        if !body.is_empty() {
            self.store.write_payload(body).await?;
        }
        result.extend(self.media.put_group(media).await?);
        Ok(result)
    }
    async fn read_group(
        &self,
        items: Vec<(String, Option<Location>)>,
    ) -> Result<Vec<Vec<u8>>, BoxError> {
        let mut native: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut result: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut media = vec![];
        let order: Vec<_> = items.iter().map(|(id, _)| id.clone()).collect();
        for (id, location) in items {
            if let Some(kind) = location
                .as_ref()
                .and_then(|location| location.path.strip_prefix("greptime/"))
            {
                native.entry(kind.to_owned()).or_default().push(id);
            } else if location.is_some() {
                media.push((id, location));
            } else {
                result.insert(id.clone(), self.read(&id, None).await?);
            }
        }
        let media_ids: Vec<_> = media.iter().map(|(id, _)| id.clone()).collect();
        for (id, bytes) in media_ids
            .into_iter()
            .zip(self.media.read_group(media).await?)
        {
            result.insert(id, bytes);
        }
        for (kind, ids) in native {
            let table = if kind == "scalars" {
                "aeroboard_scalars"
            } else {
                "aeroboard_hyperparameters"
            };
            let columns = if kind == "scalars" {
                r#""value_bits","step""#
            } else {
                r#""value","value_type""#
            };
            for chunk in ids.chunks(256) {
                let selected = chunk
                    .iter()
                    .map(|id| format!("'{}'", id.replace('\'', "''")))
                    .collect::<Vec<_>>()
                    .join(",");
                let body=self.store.sql(&format!("SELECT batch_id,CAST(ts AS BIGINT),run_id,\"name\",{columns} FROM {table} WHERE batch_id IN ({selected}) ORDER BY batch_id,CAST(row_index AS BIGINT)")).await?.ok_or_else(||boxed_error("native table missing"))?;
                let mut rows: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
                for row in super::codec::extract_rows(&body)? {
                    let row = row
                        .as_array()
                        .ok_or_else(|| boxed_error("invalid native row"))?;
                    let id = row[0]
                        .as_str()
                        .ok_or_else(|| boxed_error("missing native batch ID"))?;
                    rows.entry(id.to_owned())
                        .or_default()
                        .push(serde_json::Value::Array(row[1..].to_vec()));
                }
                for id in chunk {
                    let batch = native_batch(
                        &kind,
                        rows.get(id)
                            .ok_or_else(|| boxed_error("native batch missing"))?,
                    )?;
                    let mut bytes = vec![];
                    let mut writer =
                        arrow::ipc::writer::StreamWriter::try_new(&mut bytes, &batch.schema())?;
                    writer.write(&batch)?;
                    writer.finish()?;
                    drop(writer);
                    result.insert(id.clone(), bytes);
                }
            }
        }
        order
            .into_iter()
            .map(|id| {
                result
                    .remove(&id)
                    .ok_or_else(|| boxed_error("batch missing"))
            })
            .collect()
    }
    async fn read(&self, id: &str, location: Option<Location>) -> Result<Vec<u8>, BoxError> {
        if let Some(location) = location {
            if let Some(kind) = location.path.strip_prefix("greptime/") {
                let table = if kind == "scalars" {
                    "aeroboard_scalars"
                } else {
                    "aeroboard_hyperparameters"
                };
                let columns = if kind == "scalars" {
                    r#""value_bits","step""#
                } else {
                    r#""value","value_type""#
                };
                let sql = format!(
                    "SELECT CAST(ts AS BIGINT),run_id,\"name\",{columns} FROM {table} WHERE batch_id='{}' ORDER BY CAST(row_index AS BIGINT)",
                    id.replace('\'', "''")
                );
                let body = self
                    .store
                    .sql(&sql)
                    .await?
                    .ok_or_else(|| boxed_error("native batch table missing"))?;
                let rows = super::codec::extract_rows(&body)?;
                if rows.is_empty() {
                    return Err(boxed_error("native batch missing"));
                }
                let batch = native_batch(kind, rows)?;
                let mut bytes = vec![];
                let mut writer =
                    arrow::ipc::writer::StreamWriter::try_new(&mut bytes, &batch.schema())?;
                writer.write(&batch)?;
                writer.finish()?;
                drop(writer);
                return Ok(bytes);
            }
            return self.media.read(id, Some(location)).await;
        }
        // Pre-migration immutable IPC blobs remain readable until explicitly migrated.
        let body = self
            .store
            .sql(&format!(
                "SELECT payload FROM aeroboard_batches WHERE batch_id='{}'",
                id.replace('\'', "''")
            ))
            .await?;
        let text = body
            .as_ref()
            .and_then(|body| body.pointer("/output/0/records/rows/0/0"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| boxed_error("batch missing from Greptime"))?;
        super::codec::decode_hex(text)
    }
    async fn delete_legacy(&self, id: &str) -> Result<(), BoxError> {
        self.store
            .sql(&format!(
                "DELETE FROM aeroboard_batches WHERE batch_id='{}'",
                id.replace('\'', "''")
            ))
            .await?;
        Ok(())
    }
    async fn delete_run(&self, uid: &str) -> Result<(), BoxError> {
        self.media.delete_run(uid).await
    }
    async fn delete(&self, id: &str) -> Result<(), BoxError> {
        for table in [
            "aeroboard_batches",
            "aeroboard_scalars",
            "aeroboard_hyperparameters",
        ] {
            self.store
                .sql(&format!(
                    "DELETE FROM {table} WHERE batch_id='{}'",
                    id.replace('\'', "''")
                ))
                .await?;
        }
        self.media.delete(id).await?;
        Ok(())
    }
    async fn clear(&self) -> Result<(), BoxError> {
        for table in [
            "aeroboard_batches",
            "aeroboard_scalars",
            "aeroboard_hyperparameters",
        ] {
            self.store
                .sql(&format!("DROP TABLE IF EXISTS {table}"))
                .await?;
        }
        self.media.clear().await?;
        *self.ready.lock().await = false;
        Ok(())
    }
}

fn escape_tag(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace('=', "\\=")
        .replace(' ', "\\ ")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
fn escape_field(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
fn native_batch(
    kind: &str,
    rows: &[serde_json::Value],
) -> Result<arrow::record_batch::RecordBatch, BoxError> {
    use arrow::{
        array::{ArrayRef, Float64Array, StringArray, TimestampMillisecondArray},
        datatypes::{DataType, Field, Schema, TimeUnit},
    };
    let mut fields = vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new("run_id", DataType::Utf8, false),
        Field::new(
            if kind == "scalars" { "name" } else { "key" },
            DataType::Utf8,
            false,
        ),
    ];
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(TimestampMillisecondArray::from_iter_values(
            rows.iter()
                .map(|r| super::codec::parse_i64(r.get(0)) / 1_000_000),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r[1].as_str().unwrap_or("")),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r[2].as_str().unwrap_or("")),
        )),
    ];
    if kind == "scalars" {
        fields.extend([
            Field::new("value", DataType::Float64, false),
            Field::new("step", DataType::Float64, true),
        ]);
        columns.extend([
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|r| {
                r[3].as_str()
                    .and_then(|s| u64::from_str_radix(s, 16).ok())
                    .map(f64::from_bits)
                    .unwrap_or(f64::NAN)
            }))) as ArrayRef,
            Arc::new(Float64Array::from_iter(rows.iter().map(|r| r[4].as_f64()))) as ArrayRef,
        ]);
    } else {
        fields.extend([
            Field::new("value", DataType::Utf8, false),
            Field::new("value_type", DataType::Utf8, false),
        ]);
        columns.extend([
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r[3].as_str().unwrap_or("")),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r[4].as_str().unwrap_or("")),
            )) as ArrayRef,
        ]);
    }
    Ok(arrow::record_batch::RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn live_only_payloads_never_create_a_directory() {
        let root =
            std::env::temp_dir().join(format!("aeroboard-live-payload-{}", uuid::Uuid::new_v4()));
        let mut config = AppConfig::for_tests(root.join("data"), root.join("web"));
        config.storage_mode = StorageMode::LiveOnly;
        let backend = build(&config);
        let locations = backend
            .put_group(vec![Payload {
                id: "batch".into(),
                uid: "run".into(),
                kind: "scalars".into(),
                bytes: vec![1, 2, 3],
            }])
            .await
            .unwrap();
        assert!(locations[0].1.is_none());
        assert!(!root.exists());
        assert!(backend.read("batch", None).await.is_err());
    }
}
