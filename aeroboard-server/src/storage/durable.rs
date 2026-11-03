//! Idempotent, batch-addressed storage layered alongside legacy storage.
//! A batch is visible only after its durable backend write and SQLite receipt commit.
use super::{
    BoxError, HistoryQueryOptions, ImageFrameQuery, ImageQueryOptions, MediaKind, MediaPayload,
    MediaSource, QueryPage, StorageEngine, boxed_error,
};
use crate::{
    catalog::{self, Catalog, CatalogError},
    config::{AppConfig, StorageMode},
    history::{HistoryPoint, HyperparameterEntry, ImageHistoryPoint},
    telemetry::TelemetryBatch,
};
use arrow::{
    array::{Array, AsArray},
    ipc::{reader::StreamReader, writer::StreamWriter},
    record_batch::RecordBatch,
};
use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    io::{Cursor, Write},
    sync::Arc,
};
use tokio::sync::Mutex;

const CACHE_BYTES: usize = 128 * 1024 * 1024;
#[derive(Default)]
struct BatchCache {
    entries: HashMap<String, (Arc<RecordBatch>, usize)>,
    order: VecDeque<String>,
    bytes: usize,
}
impl BatchCache {
    fn put(&mut self, id: String, batch: Arc<RecordBatch>) {
        if self.entries.contains_key(&id) {
            return;
        }
        let bytes = batch.get_array_memory_size();
        if bytes > CACHE_BYTES {
            return;
        }
        while self.bytes + bytes > CACHE_BYTES {
            if let Some(id) = self.order.pop_front() {
                if let Some((_, bytes)) = self.entries.remove(&id) {
                    self.bytes -= bytes;
                }
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.order.push_back(id.clone());
        self.entries.insert(id, (batch, bytes));
    }
}
pub struct DurableStore {
    base: Arc<dyn StorageEngine>,
    config: AppConfig,
    catalog: Catalog,
    cache: Mutex<BatchCache>,
    writer: Mutex<()>,
    http: reqwest::Client,
}
impl DurableStore {
    pub fn new(base: Arc<dyn StorageEngine>, config: AppConfig, catalog: Catalog) -> Self {
        Self {
            base,
            config,
            catalog,
            cache: Mutex::new(BatchCache::default()),
            writer: Mutex::new(()),
            http: reqwest::Client::new(),
        }
    }
    async fn greptime_sql(&self, sql: String) -> Result<Value, BoxError> {
        let response = self
            .http
            .post(format!(
                "{}/v1/sql?db=public",
                self.config.greptime_url.trim_end_matches('/')
            ))
            .timeout(std::time::Duration::from_secs(30))
            .form(&[("sql", sql)])
            .send()
            .await?;
        let status = response.status();
        let body: Value = response.json().await?;
        if !status.is_success() || body.get("error").is_some() {
            return Err(boxed_error(format!("Greptime batch storage: {body}")));
        }
        Ok(body)
    }
    async fn put_bytes(
        &self,
        id: &str,
        uid: &str,
        time: i64,
        bytes: Vec<u8>,
    ) -> Result<(), BoxError> {
        match self.config.storage_mode {
            StorageMode::Filesystem => {
                let root = self.config.data_dir.join("batches");
                let id = id.to_owned();
                tokio::task::spawn_blocking(move || -> Result<(), BoxError> {
                    std::fs::create_dir_all(&root)?;
                    let target = root.join(format!("{id}.arrow"));
                    if target.is_file() {
                        if std::fs::read(&target)? != bytes {
                            return Err(boxed_error("stored immutable batch is corrupt"));
                        }
                        return Ok(());
                    }
                    let temporary = root.join(format!(".{id}-{}.tmp", uuid::Uuid::new_v4()));
                    let mut file = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&temporary)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    std::fs::rename(&temporary, &target)?;
                    #[cfg(unix)]
                    std::fs::File::open(root)?.sync_all()?;
                    Ok(())
                })
                .await??;
            }
            StorageMode::Greptime => {
                self.greptime_sql("CREATE TABLE IF NOT EXISTS aeroboard_batches (ts TIMESTAMP(9) NOT NULL, batch_id STRING, run_uid STRING, payload STRING, TIME INDEX(ts), PRIMARY KEY(batch_id, run_uid)) WITH (append_mode='false')".into()).await?;
                let mut hex = String::with_capacity(bytes.len() * 2);
                const DIGITS: &[u8; 16] = b"0123456789abcdef";
                for byte in bytes {
                    hex.push(DIGITS[(byte >> 4) as usize] as char);
                    hex.push(DIGITS[(byte & 15) as usize] as char);
                }
                let body = format!(
                    "aeroboard_batches,batch_id={id},run_uid={uid} payload=\"{hex}\" {}\n",
                    time.saturating_mul(1_000_000)
                );
                let response = self
                    .http
                    .post(format!(
                        "{}/v1/influxdb/write?db=public&precision=ns",
                        self.config.greptime_url.trim_end_matches('/')
                    ))
                    .timeout(std::time::Duration::from_secs(60))
                    .body(body)
                    .send()
                    .await?;
                if !response.status().is_success() {
                    return Err(boxed_error(format!(
                        "Greptime batch write: {}",
                        response.text().await?
                    )));
                }
            }
            StorageMode::LiveOnly => {}
        }
        Ok(())
    }
    async fn batch(&self, id: &str) -> Result<Arc<RecordBatch>, BoxError> {
        if let Some((batch, _)) = self.cache.lock().await.entries.get(id) {
            return Ok(batch.clone());
        }
        let bytes = match self.config.storage_mode {
            StorageMode::Filesystem => {
                tokio::fs::read(
                    self.config
                        .data_dir
                        .join("batches")
                        .join(format!("{id}.arrow")),
                )
                .await?
            }
            StorageMode::Greptime => {
                let body = self
                    .greptime_sql(format!(
                        "SELECT payload FROM aeroboard_batches WHERE batch_id='{}'",
                        id.replace('\'', "''")
                    ))
                    .await?;
                let text = body
                    .pointer("/output/0/records/rows/0/0")
                    .and_then(Value::as_str)
                    .ok_or_else(|| boxed_error("batch is missing from Greptime"))?;
                super::codec::decode_hex(text)?
            }
            StorageMode::LiveOnly => return Err(boxed_error("live-only payload has expired")),
        };
        let batch = tokio::task::spawn_blocking(move || -> Result<Arc<RecordBatch>, BoxError> {
            let mut reader = StreamReader::try_new(Cursor::new(bytes), None)?;
            Ok(Arc::new(
                reader
                    .next()
                    .ok_or_else(|| boxed_error("empty stored batch"))??,
            ))
        })
        .await??;
        self.cache.lock().await.put(id.into(), batch.clone());
        Ok(batch)
    }
    async fn receipts(
        &self,
        runs: BTreeSet<String>,
        kinds: &[&str],
    ) -> Result<Vec<Value>, BoxError> {
        let kinds: Vec<String> = kinds.iter().map(|v| (*v).into()).collect();
        Ok(self
            .catalog
            .read(move |db| {
                let ids: BTreeSet<String> = catalog::list(db, "run", None)?
                    .into_iter()
                    .filter(|r| {
                        r.data["deleted"] != true
                            && (runs.is_empty()
                                || r.data["run_id"]
                                    .as_str()
                                    .is_some_and(|id| runs.contains(id)))
                    })
                    .map(|r| r.id)
                    .collect();
                let mut statement =
                    db.prepare("SELECT body FROM receipts WHERE committed=1 ORDER BY rowid")?;
                let rows = statement.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
                let mut result = vec![];
                for row in rows {
                    let value: Value = serde_json::from_slice(&row?).map_err(|e| {
                        CatalogError(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                    })?;
                    if value["run_uid"].as_str().is_some_and(|id| ids.contains(id))
                        && value["kind"]
                            .as_str()
                            .is_some_and(|kind| kinds.iter().any(|k| k == kind))
                    {
                        result.push(value);
                    }
                }
                Ok(result)
            })
            .await?)
    }
    async fn deleted(&self) -> Result<BTreeSet<String>, BoxError> {
        Ok(self
            .catalog
            .read(|db| {
                Ok(catalog::list(db, "run", None)?
                    .into_iter()
                    .filter(|r| r.data["deleted"] == true)
                    .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned))
                    .collect())
            })
            .await?)
    }
    fn media_rows(telemetry: &TelemetryBatch) -> Vec<(String, Value)> {
        let Some(kind) = MediaKind::from_telemetry(&telemetry.kind) else {
            return vec![];
        };
        let Some(envelope) = &telemetry.envelope else {
            return vec![];
        };
        let batch = &telemetry.batch;
        let mut result = vec![];
        let ids = batch.schema().index_of("asset_id").ok();
        let steps = batch.schema().index_of("step").ok();
        for row in 0..batch.num_rows() {
            let id = ids
                .filter(|i| !batch.column(*i).is_null(row))
                .map(|i| batch.column(i).as_string::<i32>().value(row).to_owned())
                .unwrap_or_else(|| format!("{}.{}", envelope.batch_id, row));
            let step = steps.filter(|i| !batch.column(*i).is_null(row)).map(|i| {
                batch
                    .column(i)
                    .as_primitive::<arrow::datatypes::Float64Type>()
                    .value(row)
            });
            result.push((id,json!({"batch_id":envelope.batch_id,"row":row,"kind":kind.dir_name(),"run_uid":envelope.run_uid,
                "run_id":batch.column(1).as_string::<i32>().value(row),"name":batch.column(2).as_string::<i32>().value(row),
                "format":batch.column(4).as_string::<i32>().value(row),"ts_ms":batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>().value(row),"step":step})));
        }
        result
    }
}

#[async_trait]
impl StorageEngine for DurableStore {
    async fn append_batch(&self, telemetry: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        let _guard = self.writer.lock().await;
        let Some(envelope) = telemetry.envelope.clone() else {
            let deleted = self.deleted().await?;
            if (0..telemetry.batch.num_rows())
                .any(|i| deleted.contains(telemetry.batch.column(1).as_string::<i32>().value(i)))
            {
                return Err(boxed_error("run has been deleted"));
            }
            return self.base.append_batch(telemetry).await;
        };
        let id = envelope.batch_id.clone();
        let checksum = envelope.checksum.clone();
        let uid = envelope.run_uid.clone();
        let sequence = envelope.sequence;
        let canonical = telemetry
            .batch
            .column(1)
            .as_string::<i32>()
            .value(0)
            .to_owned();
        let manifest = json!({"id":id,"run_uid":uid,"run_id":canonical,"kind":telemetry.kind.as_dir_name(),"envelope":envelope});
        let ready=self.catalog.quiet(move|db|{
            catalog::run_by_id(db,&uid)?;
            let receipt:Option<(String,bool,Vec<u8>)>=db.query_row("SELECT checksum,committed,body FROM receipts WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((previous,ready,body))=receipt{
                let stored:Value=serde_json::from_slice(&body).map_err(|_|CatalogError::invalid("invalid batch receipt"))?;
                if previous!=checksum||stored["envelope"]!=manifest["envelope"]{return Err(CatalogError::conflict("batch identity mismatch"));}return Ok(ready);
            }
            let occupied:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM receipts WHERE run_uid=?1 AND sequence=?2 AND json_extract(CAST(body AS TEXT),'$.envelope.session_id')=?3)",params![uid,sequence,manifest["envelope"]["session_id"].as_str().unwrap_or("")],|r|r.get(0))?;
            if occupied{return Err(CatalogError::conflict("session sequence was reused for another batch"));}
            db.execute("INSERT INTO receipts(id,checksum,run_uid,sequence,body) VALUES(?1,?2,?3,?4,?5)",params![id,checksum,uid,sequence,manifest.to_string().as_bytes()])?;
            Ok(false)
        }).await?;
        if ready {
            return Ok(());
        }
        let batch = telemetry.batch.clone();
        let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, BoxError> {
            let mut bytes = vec![];
            let mut writer = StreamWriter::try_new(&mut bytes, &batch.schema())?;
            writer.write(&batch)?;
            writer.finish()?;
            drop(writer);
            Ok(bytes)
        })
        .await??;
        self.put_bytes(
            &envelope.batch_id,
            &envelope.run_uid,
            envelope.captured_at_ms,
            bytes,
        )
        .await?;
        let id = envelope.batch_id.clone();
        let assets = Self::media_rows(&telemetry);
        self.catalog
            .quiet(move |db| {
                for (asset_id, data) in assets {
                    if let Some(existing) = catalog::get(db, "asset", &asset_id)? {
                        if existing.data != data {
                            return Err(CatalogError::conflict("asset ID has different contents"));
                        }
                    } else {
                        catalog::save(db, "asset", &asset_id, &envelope.run_uid, &data, Some(0))?;
                    }
                }
                db.execute("UPDATE receipts SET committed=1 WHERE id=?1", [id])?;
                Ok(())
            })
            .await?;
        self.cache.lock().await.put(
            telemetry.envelope.as_ref().unwrap().batch_id.clone(),
            telemetry.batch.clone(),
        );
        Ok(())
    }
    async fn query_history(
        &self,
        query: &HistoryQueryOptions,
    ) -> Result<QueryPage<HistoryPoint>, BoxError> {
        if self.config.storage_mode == StorageMode::LiveOnly {
            return self.base.query_history(query).await;
        }
        let mut unpaged = query.clone();
        let mut selection = super::query::HistorySelection::new(query);
        unpaged.limit = selection.prefix();
        unpaged.cursor = None;
        let deleted = self.deleted().await?;
        if !deleted.is_empty() {
            unpaged.run_ids = self
                .base
                .list_runs()
                .await?
                .into_iter()
                .filter(|id| {
                    !deleted.contains(id)
                        && (query.run_ids.is_empty() || query.run_ids.contains(id))
                })
                .collect();
        }
        if deleted.is_empty() || !unpaged.run_ids.is_empty() {
            for point in self.base.query_history(&unpaged).await?.rows {
                selection.push(point);
            }
        }
        for receipt in self.receipts(query.run_ids.clone(), &["scalars"]).await? {
            let batch = self.batch(receipt["id"].as_str().unwrap_or("")).await?;
            for mut point in super::codec::parse_scalar_batch(&batch) {
                if (!query.metrics.is_empty() && !query.metrics.contains(&point.name))
                    || query.start_ms.is_some_and(|t| point.ts_ms < t)
                    || query.end_ms.is_some_and(|t| point.ts_ms > t)
                {
                    continue;
                }
                point.segment = receipt["envelope"]["segment"].as_str().map(str::to_owned);
                selection.push(point);
            }
        }
        let mut rows = selection.finish();
        let deleted = self.deleted().await?;
        rows.retain(|r| !deleted.contains(&r.run_id));
        Ok(super::query::apply_history_query(rows, query))
    }
    async fn query_hyperparameters(
        &self,
        runs: &BTreeSet<String>,
    ) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        if self.config.storage_mode == StorageMode::LiveOnly {
            return self.base.query_hyperparameters(runs).await;
        }
        let mut rows = self.base.query_hyperparameters(runs).await?;
        for receipt in self.receipts(runs.clone(), &["hyperparameters"]).await? {
            let batch = self.batch(receipt["id"].as_str().unwrap_or("")).await?;
            rows.extend(super::codec::parse_hparam_batch(&batch));
        }
        let deleted = self.deleted().await?;
        rows.retain(|(r, _)| !deleted.contains(r));
        Ok(rows)
    }
    async fn query_media_history(
        &self,
        kind: MediaKind,
        query: &ImageQueryOptions,
    ) -> Result<QueryPage<ImageHistoryPoint>, BoxError> {
        if self.config.storage_mode == StorageMode::LiveOnly {
            return self.base.query_media_history(kind, query).await;
        }
        let mut unpaged = query.clone();
        unpaged.limit = None;
        unpaged.cursor = None;
        let mut rows = self.base.query_media_history(kind, &unpaged).await?.rows;
        let deleted = self.deleted().await?;
        let assets = self
            .catalog
            .read(|db| catalog::list(db, "asset", None))
            .await?;
        for asset in assets {
            let data = asset.data;
            if data["kind"] != kind.dir_name() {
                continue;
            }
            let run_id = data["run_id"].as_str().unwrap_or("").to_owned();
            if deleted.contains(&run_id) {
                continue;
            }
            if !query.run_ids.is_empty() && !query.run_ids.contains(&run_id) {
                continue;
            }
            let image_base64 = if query.include_data {
                use base64::Engine;
                match self.load_asset(&asset.id).await? {
                    Some(MediaPayload {
                        source: MediaSource::Bytes(bytes),
                        ..
                    }) => Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                    _ => None,
                }
            } else {
                None
            };
            rows.push(ImageHistoryPoint {
                ts_ms: data["ts_ms"].as_i64().unwrap_or(0),
                run_id,
                name: data["name"].as_str().unwrap_or("").into(),
                format: data["format"].as_str().unwrap_or("").into(),
                image_base64,
                asset_id: Some(asset.id),
                step: data["step"].as_f64(),
            });
        }
        let deleted = self.deleted().await?;
        rows.retain(|r| !deleted.contains(&r.run_id));
        Ok(super::query::apply_image_query(rows, query))
    }
    async fn load_media(
        &self,
        kind: MediaKind,
        query: &ImageFrameQuery,
    ) -> Result<Option<MediaPayload>, BoxError> {
        if self.deleted().await?.contains(&query.run_id) {
            return Ok(None);
        }
        let assets = self
            .catalog
            .read(|db| catalog::list(db, "asset", None))
            .await?;
        if let Some(asset) = assets.iter().rev().find(|e| {
            e.data["kind"] == kind.dir_name()
                && e.data["run_id"] == query.run_id
                && e.data["name"] == query.name
                && e.data["ts_ms"] == query.ts_ms
        }) {
            return self.load_asset(&asset.id).await;
        }
        self.base.load_media(kind, query).await
    }
    async fn load_asset(&self, id: &str) -> Result<Option<MediaPayload>, BoxError> {
        let id = id.to_owned();
        let asset = self
            .catalog
            .read(move |db| {
                let asset = catalog::get(db, "asset", &id)?;
                if let Some(asset) = &asset {
                    catalog::run_by_id(db, &asset.parent)?;
                }
                Ok(asset)
            })
            .await?;
        let Some(asset) = asset else {
            return Ok(None);
        };
        let data = asset.data;
        let batch = self.batch(data["batch_id"].as_str().unwrap_or("")).await?;
        let row = data["row"].as_u64().unwrap_or(0) as usize;
        if row >= batch.num_rows() {
            return Err(boxed_error("invalid asset row"));
        }
        Ok(Some(MediaPayload {
            format: data["format"].as_str().unwrap_or("").into(),
            source: MediaSource::Bytes(batch.column(3).as_binary::<i32>().value(row).to_vec()),
        }))
    }
    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        let mut runs: BTreeSet<String> = self.base.list_runs().await?.into_iter().collect();
        runs.extend(
            self.catalog
                .read(|db| {
                    Ok(catalog::list(db, "run", None)?
                        .iter()
                        .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>())
                })
                .await?,
        );
        let deleted = self.deleted().await?;
        runs.retain(|r| !deleted.contains(r));
        Ok(runs.into_iter().collect())
    }
    async fn delete_run(&self, run_id: &str) -> Result<(), BoxError> {
        let _guard = self.writer.lock().await;
        self.base.delete_run(run_id).await?;
        let run_id = run_id.to_owned();
        let ids = self
            .catalog
            .read(move |db| {
                let run = catalog::list(db, "run", None)?
                    .into_iter()
                    .find(|r| r.data["run_id"] == run_id)
                    .ok_or_else(CatalogError::missing)?;
                let mut statement = db.prepare("SELECT id FROM receipts WHERE run_uid=?1")?;
                Ok(statement
                    .query_map([run.id], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .await?;
        for id in ids {
            match self.config.storage_mode {
                StorageMode::Filesystem => {
                    let path = self
                        .config
                        .data_dir
                        .join("batches")
                        .join(format!("{id}.arrow"));
                    if let Err(error) = tokio::fs::remove_file(path).await
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        return Err(error.into());
                    }
                }
                StorageMode::Greptime => {
                    self.greptime_sql(format!(
                        "DELETE FROM aeroboard_batches WHERE batch_id='{id}'"
                    ))
                    .await?;
                }
                StorageMode::LiveOnly => return Err(boxed_error("no persistent data to delete")),
            }
        }
        *self.cache.lock().await = BatchCache::default();
        Ok(())
    }
    async fn clear_history(&self) -> Result<(), BoxError> {
        let _guard = self.writer.lock().await;
        self.base.clear_history().await?;
        self.catalog
            .write(|db| {
                for mut run in catalog::list(db, "run", None)? {
                    run.data["deleted"] = json!(true);
                    catalog::save(
                        db,
                        "run",
                        &run.id,
                        &run.parent,
                        &run.data,
                        Some(run.revision),
                    )?;
                }
                Ok(())
            })
            .await?;
        if self.config.storage_mode == StorageMode::Filesystem {
            let root = self.config.data_dir.join("batches");
            if root.exists() {
                tokio::fs::remove_dir_all(root).await?;
            }
        }
        if self.config.storage_mode == StorageMode::Greptime {
            self.greptime_sql("DROP TABLE IF EXISTS aeroboard_batches".into())
                .await?;
        }
        *self.cache.lock().await = BatchCache::default();
        Ok(())
    }
}
