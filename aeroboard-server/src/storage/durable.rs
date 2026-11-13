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
use rusqlite::params;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    io::Cursor,
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
    fn get(&mut self, id: &str) -> Option<Arc<RecordBatch>> {
        let batch = self.entries.get(id)?.0.clone();
        self.order.retain(|key| key != id);
        self.order.push_back(id.to_owned());
        Some(batch)
    }

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
    history_cache: Mutex<BatchCache>,
    writer: Mutex<()>,
    payloads: Arc<dyn super::payload::PayloadBackend>,
}
impl DurableStore {
    pub fn new(base: Arc<dyn StorageEngine>, config: AppConfig, catalog: Catalog) -> Self {
        Self {
            payloads: super::payload::build(&config),
            base,
            config,
            catalog,
            cache: Mutex::new(BatchCache::default()),
            history_cache: Mutex::new(BatchCache::default()),
            writer: Mutex::new(()),
        }
    }
    async fn batch(&self, id: &str) -> Result<Arc<RecordBatch>, BoxError> {
        if let Some(batch) = self.cache.lock().await.get(id) {
            return Ok(batch);
        }
        let key = id.to_owned();
        let location = self
            .catalog
            .read(move |db| {
                use rusqlite::OptionalExtension;
                let body: Option<String> = db
                    .query_row(
                        "SELECT body FROM payload_locations WHERE id=?1",
                        [key],
                        |r| r.get(0),
                    )
                    .optional()?;
                body.map(|body| {
                    serde_json::from_str::<super::payload::Location>(&body)
                        .map_err(|e| CatalogError::invalid(e.to_string()))
                })
                .transpose()
            })
            .await?;
        let bytes = self.payloads.read(id, location).await?;
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
    async fn batches(&self, receipts: &[Value]) -> Result<Vec<Arc<RecordBatch>>, BoxError> {
        let ids = receipts
            .iter()
            .map(|receipt| {
                receipt["id"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| boxed_error("missing batch ID"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let keys = ids.clone();
        let locations=self.catalog.read(move |db| {
            let mut statement=db.prepare("SELECT id,body FROM payload_locations WHERE id IN (SELECT value FROM json_each(?1))")?;
            let mut locations=HashMap::new();
            for row in statement.query_map([json!(keys).to_string()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                let (id,body)=row?;locations.insert(id,serde_json::from_str::<super::payload::Location>(&body).map_err(|e|CatalogError::invalid(e.to_string()))?);
            }
            Ok(locations)
        }).await?;
        let items = ids
            .iter()
            .map(|id| (id.clone(), locations.get(id).cloned()))
            .collect();
        let bytes = self.payloads.read_group(items).await?;
        tokio::task::spawn_blocking(move || {
            bytes
                .into_iter()
                .map(|bytes| {
                    let mut reader = StreamReader::try_new(Cursor::new(bytes), None)?;
                    Ok(Arc::new(
                        reader
                            .next()
                            .ok_or_else(|| boxed_error("empty stored batch"))??,
                    ))
                })
                .collect()
        })
        .await?
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
                let mut statement = db.prepare("SELECT receipts.body FROM run_lookup r JOIN receipts ON receipts.run_uid=r.uid WHERE r.deleted=0 AND receipts.committed=1 AND (?1='[]' OR r.run_id IN (SELECT value FROM json_each(?1))) AND receipts.kind IN (SELECT value FROM json_each(?2)) ORDER BY receipts.rowid")?;
                let rows = statement.query_map(params![json!(runs).to_string(),json!(kinds).to_string()], |r| r.get::<_,Vec<u8>>(0))?;
                let mut result = vec![];
                for row in rows {
                    result.push(serde_json::from_slice(&row?).map_err(|e| CatalogError::http(axum::http::StatusCode::INTERNAL_SERVER_ERROR,e.to_string()))?);
                }
                Ok(result)
            })
            .await?)
    }
    async fn deleted(&self) -> Result<BTreeSet<String>, BoxError> {
        self.deleted_for(&BTreeSet::new()).await
    }
    async fn deleted_for(&self, runs: &BTreeSet<String>) -> Result<BTreeSet<String>, BoxError> {
        let runs = runs.clone();
        Ok(self.catalog.read(move|db|{
            if runs.is_empty(){return catalog::run_names(db,Some(true));}
            let mut statement=db.prepare("SELECT run_id FROM run_lookup WHERE deleted=1 AND run_id IN (SELECT value FROM json_each(?1))")?;
            Ok(statement.query_map([json!(runs).to_string()],|row|row.get(0))?.collect::<std::result::Result<_,_>>()?)
        }).await?)
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
    async fn initialize(&self) -> Result<(), BoxError> {
        self.catalog
            .import_runs(self.base.list_runs().await?.into_iter().collect())
            .await?;
        let old=self.catalog.read(|db| {
            let mut statement=db.prepare("SELECT receipts.body FROM receipts LEFT JOIN payload_locations p ON p.id=receipts.id JOIN run_lookup r ON r.uid=receipts.run_uid WHERE receipts.committed=1 AND p.id IS NULL AND r.deleted=0")?;
            statement.query_map([],|r|r.get::<_,Vec<u8>>(0))?.map(|row|{let row=row?;serde_json::from_slice::<Value>(&row).map_err(|e|CatalogError::invalid(e.to_string()))}).collect::<catalog::Result<Vec<_>>>()
        }).await?;
        for chunk in old.chunks(64) {
            let mut payloads = vec![];
            for receipt in chunk {
                let id = receipt["id"]
                    .as_str()
                    .ok_or_else(|| boxed_error("receipt ID missing"))?;
                let bytes = self.payloads.read(id, None).await?;
                payloads.push(super::payload::Payload {
                    id: id.to_owned(),
                    uid: receipt["run_uid"]
                        .as_str()
                        .ok_or_else(|| boxed_error("run UID missing"))?
                        .into(),
                    kind: receipt["kind"]
                        .as_str()
                        .ok_or_else(|| boxed_error("receipt kind missing"))?
                        .into(),
                    bytes,
                });
            }
            let locations = self.payloads.put_group(payloads).await?;
            self.catalog
                .quiet(move |db| {
                    for (id, location) in locations {
                        if let Some(location) = location {
                            db.execute(
                                "INSERT INTO payload_locations VALUES(?1,?2)",
                                params![
                                    id,
                                    serde_json::to_string(&location)
                                        .map_err(|e| CatalogError::invalid(e.to_string()))?
                                ],
                            )?;
                        }
                    }
                    Ok(())
                })
                .await?;
            // Old blobs can be removed only after their new locations are durable.
            for receipt in chunk {
                self.payloads
                    .delete_legacy(receipt["id"].as_str().unwrap())
                    .await?;
            }
        }
        let migrated = self
            .catalog
            .read(|db| {
                Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM settings WHERE key='summaries_migrated')",
                    [],
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .await?;
        if !migrated {
            let points = self
                .query_history(&HistoryQueryOptions::default())
                .await?
                .rows;
            let hparams = self.query_hyperparameters(&BTreeSet::new()).await?;
            let accumulated = crate::summaries::accumulate(&points, hparams);
            self.catalog.quiet(move |db| {
                db.execute("DELETE FROM run_summaries",[])?;
                for (run,accumulator) in accumulated {crate::summaries::save_accumulator(db,&run,&accumulator)?;}
                db.execute("INSERT OR IGNORE INTO summary_batches SELECT id FROM receipts WHERE committed=1",[])?;
                db.execute("INSERT INTO settings VALUES('summaries_migrated','1')",[])?;
                Ok(())
            }).await?;
        }
        Ok(())
    }
    async fn persisted_summaries(
        &self,
        runs: &BTreeSet<String>,
    ) -> Result<Option<Vec<crate::history::RunSummary>>, BoxError> {
        let runs = runs.clone();
        Ok(self.catalog.read(move |db| {
            let ready:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM settings WHERE key='summaries_migrated')",[],|r|r.get(0))?;
            if !ready {return Ok(None);}
            let mut stmt=db.prepare("SELECT s.run_id,s.body FROM run_summaries s JOIN run_lookup r ON s.run_id=r.run_id WHERE r.deleted=0 AND (?1='[]' OR s.run_id IN (SELECT value FROM json_each(?1)))")?;
            let mut result=vec![];
            for row in stmt.query_map([json!(runs).to_string()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?)))? {
                let (run,body)=row?;
                let accumulator:crate::summaries::RunAccumulator=serde_json::from_slice(&body).map_err(|e|CatalogError::invalid(e.to_string()))?;
                result.push(accumulator.to_summary(&run));
            }
            Ok(Some(result))
        }).await?)
    }

    async fn append_batch(&self, telemetry: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        self.append_group(vec![telemetry])
            .await
            .pop()
            .unwrap()
            .map(|_| ())
            .map_err(boxed_error)
    }
    async fn append_group(&self, batches: Vec<Arc<TelemetryBatch>>) -> Vec<Result<bool, String>> {
        let _guard = self.writer.lock().await;
        let identified: Vec<_> = batches
            .iter()
            .filter(|batch| batch.envelope.is_some())
            .cloned()
            .collect();
        let reserve = self.catalog.quiet(move |db| {
            let mut outcomes = HashMap::new();
            for telemetry in identified {
                let envelope = telemetry.envelope.as_ref().unwrap();
                if outcomes.contains_key(&envelope.batch_id) {
                    if let Err(error)=catalog::check_batch(db,envelope,&[]) {outcomes.insert(envelope.batch_id.clone(),Err(error.to_string()));}
                    continue;
                }
                let assets = Self::media_rows(&telemetry);
                let ids = assets.iter().map(|(id,_)|id.clone()).collect::<Vec<_>>();
                let result = (|| {
                    let (_,ready) = catalog::check_batch(db,envelope,&ids)?;
                    if ready { return Ok(false); }
                    let timestamps=telemetry.batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
                    let min_ts=arrow::compute::min(timestamps);let max_ts=arrow::compute::max(timestamps);
                    let metrics=if telemetry.kind==crate::telemetry::TelemetryKind::Scalars {Some((0..telemetry.batch.num_rows()).map(|i|telemetry.batch.column(2).as_string::<i32>().value(i).to_owned()).collect::<BTreeSet<_>>())}else{None};
                    let manifest = json!({"id":envelope.batch_id,"run_uid":envelope.run_uid,"run_id":telemetry.batch.column(1).as_string::<i32>().value(0),"kind":telemetry.kind.as_dir_name(),"envelope":envelope,"min_ts":min_ts,"max_ts":max_ts,"metrics":metrics});
                    db.execute("INSERT OR IGNORE INTO receipts(id,checksum,run_uid,sequence,body,session_id,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![envelope.batch_id,envelope.checksum,envelope.run_uid,envelope.sequence,manifest.to_string().as_bytes(),envelope.session_id,telemetry.kind.as_dir_name()]).map_err(|error| if matches!(error,rusqlite::Error::SqliteFailure(ref code,_) if code.code==rusqlite::ErrorCode::ConstraintViolation) { CatalogError::conflict("session sequence was reused") } else { error.into() })?;
                    // INSERT OR IGNORE must never mask a unique sequence conflict.
                    if !db.query_row("SELECT EXISTS(SELECT 1 FROM receipts WHERE id=?1)",[&envelope.batch_id],|r|r.get::<_,bool>(0))? { return Err(CatalogError::conflict("session sequence was reused")); }
                    Ok(true)
                })();
                outcomes.insert(envelope.batch_id.clone(),result.map_err(|e:CatalogError|e.to_string()));
            }
            Ok(outcomes)
        }).await;
        let mut reserved = match reserve {
            Ok(value) => value,
            Err(error) => return batches.iter().map(|_| Err(error.to_string())).collect(),
        };
        let mut outcomes = Vec::with_capacity(batches.len());
        let mut commit = Vec::new();
        let mut payloads = Vec::new();
        for telemetry in &batches {
            let Some(envelope) = &telemetry.envelope else {
                let names: BTreeSet<_> = (0..telemetry.batch.num_rows())
                    .map(|row| {
                        telemetry
                            .batch
                            .column(1)
                            .as_string::<i32>()
                            .value(row)
                            .to_owned()
                    })
                    .collect();
                let result = match self.deleted_for(&names).await {
                    Ok(deleted) if deleted.is_disjoint(&names) => {
                        let result = self.base.append_batch(telemetry.clone()).await;
                        if result.is_ok() {
                            let telemetry = telemetry.clone();
                            self.catalog.import_runs(names).await.ok();
                            self.catalog
                                .quiet(move |db| crate::summaries::persist_batch(db, &telemetry))
                                .await
                                .map(|()| true)
                                .map_err(|e| e.to_string())
                        } else {
                            result.map(|()| true).map_err(|e| e.to_string())
                        }
                    }
                    Ok(_) => Err("run has been deleted".into()),
                    Err(error) => Err(error.to_string()),
                };
                outcomes.push(result);
                continue;
            };
            let result = reserved.remove(&envelope.batch_id).unwrap_or(Ok(false));
            if !matches!(result, Ok(true)) {
                outcomes.push(result);
                continue;
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
            .await;
            let result = match bytes {
                Ok(Ok(bytes)) => {
                    payloads.push(super::payload::Payload {
                        id: envelope.batch_id.clone(),
                        uid: envelope.run_uid.clone(),
                        kind: telemetry.kind.as_dir_name().unwrap_or("unknown").into(),
                        bytes,
                    });
                    Ok(true)
                }
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            if result.is_ok() {
                commit.push((outcomes.len(), telemetry.clone()));
            }
            outcomes.push(result);
        }
        let locations = match self.payloads.put_group(payloads).await {
            Ok(locations) => locations,
            Err(error) => {
                for (index, _) in &commit {
                    outcomes[*index] = Err(error.to_string());
                }
                return outcomes;
            }
        };
        let to_commit = commit.clone();
        let committed = self.catalog.quiet(move |db| {
            for (id,location) in locations {
                if let Some(location)=location {
                    db.execute("INSERT INTO payload_locations VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body",params![id,serde_json::to_string(&location).map_err(|e|CatalogError::invalid(e.to_string()))?])?;
                }
            }
            for (_,telemetry) in to_commit {
                let envelope = telemetry.envelope.as_ref().unwrap();
                // Recheck tombstones after the backend write, before visibility.
                catalog::run_by_id(db,&envelope.run_uid)?;
                for (id,data) in Self::media_rows(&telemetry) {
                    if let Some(existing) = catalog::get(db,"asset",&id)? {
                        if existing.data != data { return Err(CatalogError::conflict("asset ID has different contents")); }
                    } else { catalog::save(db,"asset",&id,&envelope.run_uid,&data,Some(0))?; }
                }
                if db.execute("INSERT OR IGNORE INTO summary_batches VALUES(?1)",[&envelope.batch_id])?==1 {crate::summaries::persist_batch(db,&telemetry)?;}
                db.execute("UPDATE receipts SET committed=1 WHERE id=?1",[&envelope.batch_id])?;
            }
            Ok(())
        }).await;
        for (index, telemetry) in commit {
            if let Err(error) = &committed {
                outcomes[index] = Err(error.to_string());
            } else {
                self.cache.lock().await.put(
                    telemetry.envelope.as_ref().unwrap().batch_id.clone(),
                    telemetry.batch.clone(),
                );
            }
        }
        outcomes
    }
    async fn query_history_batches(
        &self,
        query: &HistoryQueryOptions,
    ) -> Result<QueryPage<RecordBatch>, BoxError> {
        if self.config.storage_mode == StorageMode::LiveOnly {
            return self.base.query_history_batches(query).await;
        }
        let runs = query.run_ids.clone();
        let revisions=self.catalog.read(move|db|{
            let mut statement=db.prepare("SELECT r.uid,COALESCE(d.revision,0),r.deleted FROM run_lookup r LEFT JOIN data_revisions d ON d.run_id=r.run_id WHERE (?1='[]' OR r.run_id IN (SELECT value FROM json_each(?1))) ORDER BY r.uid")?;
            Ok(statement.query_map([json!(runs).to_string()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,bool>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        }).await?;
        let key = json!([
            query.run_ids,
            query.metrics,
            query.start_ms,
            query.end_ms,
            query.max_points,
            revisions
        ])
        .to_string();
        if let Some(batch) = self.history_cache.lock().await.get(&key) {
            return Ok(history_page(&batch, query));
        }
        let mut unpaged = query.clone();
        unpaged.limit = None;
        unpaged.cursor = None;
        unpaged.max_points = None;
        let deleted = self.deleted_for(&query.run_ids).await?;
        // Legacy payloads are converted only at this compatibility boundary.
        let mut legacy = self.base.query_history(&unpaged).await?.rows;
        legacy.retain(|point| !deleted.contains(&point.run_id));
        let mut batches = vec![];
        if !legacy.is_empty() {
            let batch = crate::history::history_rows_to_batch(&legacy)?;
            let mut columns = batch.columns().to_vec();
            for index in [1, 2] {
                columns[index] =
                    arrow::compute::cast(&columns[index], &arrow::datatypes::DataType::Utf8)?;
            }
            batches.push(RecordBatch::try_new(history_schema(), columns)?);
        }
        let receipts = self
            .receipts(query.run_ids.clone(), &["scalars"])
            .await?
            .into_iter()
            .filter(|receipt| {
                !query
                    .start_ms
                    .zip(receipt["max_ts"].as_i64())
                    .is_some_and(|(start, max)| start > max)
                    && !query
                        .end_ms
                        .zip(receipt["min_ts"].as_i64())
                        .is_some_and(|(end, min)| end < min)
                    && (query.metrics.is_empty()
                        || receipt["metrics"].as_array().is_none_or(|names| {
                            names
                                .iter()
                                .filter_map(Value::as_str)
                                .any(|name| query.metrics.contains(name))
                        }))
            })
            .collect::<Vec<_>>();
        let stored = self.batches(&receipts).await?;
        for (receipt, batch) in receipts.into_iter().zip(stored) {
            let timestamps = batch
                .column(0)
                .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
            let names = batch.column(2).as_string::<i32>();
            let mask = arrow::array::BooleanArray::from(
                (0..batch.num_rows())
                    .map(|i| {
                        query
                            .start_ms
                            .is_none_or(|start| timestamps.value(i) >= start)
                            && query.end_ms.is_none_or(|end| timestamps.value(i) <= end)
                            && (query.metrics.is_empty() || query.metrics.contains(names.value(i)))
                    })
                    .collect::<Vec<_>>(),
            );
            let filtered = arrow::compute::filter_record_batch(&batch, &mask)?;
            if filtered.num_rows() == 0 {
                continue;
            }
            let mut columns = filtered.columns()[..5].to_vec();
            columns.push(Arc::new(arrow::array::StringArray::from(
                vec![receipt["envelope"]["segment"].as_str(); filtered.num_rows()],
            )));
            batches.push(RecordBatch::try_new(history_schema(), columns)?);
        }
        let selection = query.clone();
        let batch = Arc::new(
            tokio::task::spawn_blocking(move || columnar_batch(batches, &selection)).await??,
        );
        self.history_cache.lock().await.put(key, batch.clone());
        Ok(history_page(&batch, query))
    }
    async fn query_history(
        &self,
        query: &HistoryQueryOptions,
    ) -> Result<QueryPage<HistoryPoint>, BoxError> {
        let page = self.query_history_batches(query).await?;
        let mut rows = vec![];
        for batch in page.rows {
            let mut columns = batch.columns().to_vec();
            for index in [1, 2] {
                columns[index] =
                    arrow::compute::cast(&columns[index], &arrow::datatypes::DataType::Utf8)?;
            }
            let native = RecordBatch::try_new(history_schema(), columns)?;
            let mut points = super::codec::parse_scalar_batch(&native);
            let segments = native.column(5).as_string::<i32>();
            for (i, point) in points.iter_mut().enumerate() {
                point.segment = (!segments.is_null(i)).then(|| segments.value(i).to_owned());
            }
            rows.extend(points);
        }
        Ok(QueryPage {
            rows,
            next_cursor: page.next_cursor,
        })
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
        let deleted = self.deleted_for(runs).await?;
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
        let deleted = self.deleted_for(&query.run_ids).await?;
        let runs = query.run_ids.clone();
        let assets = self
            .catalog
            .read(move |db| catalog::assets(db, &runs, kind.dir_name(), None, None))
            .await?;
        for asset in assets {
            let data = asset.data;
            let run_id = data.run_id;
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
                ts_ms: data.ts_ms,
                run_id,
                name: data.name,
                format: data.format,
                image_base64,
                asset_id: Some(asset.id),
                step: data.step,
            });
        }
        rows.retain(|r| !deleted.contains(&r.run_id));
        Ok(super::query::apply_image_query(rows, query))
    }
    async fn load_media(
        &self,
        kind: MediaKind,
        query: &ImageFrameQuery,
    ) -> Result<Option<MediaPayload>, BoxError> {
        let runs = BTreeSet::from([query.run_id.clone()]);
        if self.deleted_for(&runs).await?.contains(&query.run_id) {
            return Ok(None);
        }
        let name = query.name.clone();
        let time = query.ts_ms;
        let asset = self
            .catalog
            .read(move |db| {
                Ok(catalog::assets(db, &runs, kind.dir_name(), Some(&name), Some(time))?.pop())
            })
            .await?;
        if let Some(asset) = asset {
            return self.load_asset(&asset.id).await;
        }
        self.base.load_media(kind, query).await
    }
    async fn load_asset(&self, id: &str) -> Result<Option<MediaPayload>, BoxError> {
        let id = id.to_owned();
        let asset = self
            .catalog
            .read(move |db| {
                let asset = catalog::get(db, "asset", &id)?
                    .map(|asset| asset.typed::<crate::models::Asset>())
                    .transpose()?;
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
        let batch = self.batch(&data.batch_id).await?;
        let row = data.row;
        if row >= batch.num_rows() {
            return Err(boxed_error("invalid asset row"));
        }
        Ok(Some(MediaPayload {
            format: data.format,
            source: MediaSource::Bytes(batch.column(3).as_binary::<i32>().value(row).to_vec()),
        }))
    }
    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        let mut runs: BTreeSet<String> = self.base.list_runs().await?.into_iter().collect();
        runs.extend(self.catalog.read(|db| catalog::run_names(db, None)).await?);
        let deleted = self.deleted().await?;
        runs.retain(|r| !deleted.contains(r));
        Ok(runs.into_iter().collect())
    }
    async fn delete_run(&self, run_id: &str) -> Result<(), BoxError> {
        let _guard = self.writer.lock().await;
        self.base.delete_run(run_id).await?;
        let run_id = run_id.to_owned();
        let lookup_id = run_id.clone();
        let ids = self
            .catalog
            .read(move |db| {
                use rusqlite::OptionalExtension;
                let uid = db
                    .query_row(
                        "SELECT uid FROM run_lookup WHERE run_id=?1",
                        [lookup_id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?
                    .ok_or_else(CatalogError::missing)?;
                let mut statement = db.prepare("SELECT id FROM receipts WHERE run_uid=?1")?;
                Ok(statement
                    .query_map([uid], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .await?;
        for id in ids {
            self.payloads.delete(&id).await?;
        }
        let name = run_id.to_owned();
        let uid = self
            .catalog
            .read(move |db| {
                use rusqlite::OptionalExtension;
                Ok(db
                    .query_row("SELECT uid FROM run_lookup WHERE run_id=?1", [name], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?)
            })
            .await?;
        if let Some(uid) = uid {
            self.payloads.delete_run(&uid).await?;
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
        self.payloads.clear().await?;
        *self.history_cache.lock().await = BatchCache::default();
        *self.cache.lock().await = BatchCache::default();
        Ok(())
    }
}

fn history_schema() -> Arc<arrow::datatypes::Schema> {
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    Arc::new(Schema::new(vec![
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
        Field::new("step", DataType::Float64, true),
        Field::new("segment", DataType::Utf8, true),
    ]))
}
fn columnar_batch(
    batches: Vec<RecordBatch>,
    query: &HistoryQueryOptions,
) -> Result<RecordBatch, BoxError> {
    use arrow::{
        array::UInt32Array,
        compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take},
    };
    let batch = concat_batches(&history_schema(), &batches)?;
    let timestamps = batch
        .column(0)
        .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
    let mut columns = if timestamps
        .values()
        .windows(2)
        .all(|pair| pair[0] <= pair[1])
    {
        batch.columns().to_vec()
    } else {
        let indices = lexsort_to_indices(
            &[SortColumn {
                values: batch.column(0).clone(),
                options: Some(SortOptions::default()),
            }],
            None,
        )?;
        batch
            .columns()
            .iter()
            .map(|column| take(column, &indices, None))
            .collect::<Result<Vec<_>, _>>()?
    };
    let count = batch.num_rows();
    if let Some(max_points) = query.max_points {
        let runs = columns[1].as_string::<i32>();
        let names = columns[2].as_string::<i32>();
        let values = columns[3].as_primitive::<arrow::datatypes::Float64Type>();
        let mut groups: std::collections::BTreeMap<(&str, &str), Vec<u32>> = Default::default();
        for i in 0..count {
            groups
                .entry((runs.value(i), names.value(i)))
                .or_default()
                .push(i as u32);
        }
        let mut selected = vec![];
        for rows in groups.values() {
            if rows.len() <= max_points {
                selected.extend_from_slice(rows);
                continue;
            }
            selected.push(rows[0]);
            let preserve_gap = max_points >= 5
                && rows
                    .iter()
                    .any(|&row| !values.value(row as usize).is_finite());
            let buckets = ((max_points - 2) / if preserve_gap { 3 } else { 2 }).max(1);
            for bucket in 0..buckets {
                let low = 1 + (rows.len() - 2) * bucket / buckets;
                let high = 1 + (rows.len() - 2) * (bucket + 1) / buckets;
                let mut min: Option<u32> = None;
                let mut max: Option<u32> = None;
                let mut gap = None;
                for &row in &rows[low..high] {
                    let value = values.value(row as usize);
                    if !value.is_finite() {
                        gap = Some(row);
                        continue;
                    }
                    if min.is_none_or(|i| value < values.value(i as usize)) {
                        min = Some(row);
                    }
                    if max.is_none_or(|i| value > values.value(i as usize)) {
                        max = Some(row);
                    }
                }
                if let Some(min) = min {
                    selected.push(min);
                }
                if let Some(max) = max
                    && Some(max) != min
                {
                    selected.push(max);
                }
                if preserve_gap && let Some(gap) = gap {
                    selected.push(gap);
                }
            }
            selected.push(*rows.last().unwrap());
        }
        selected.sort_unstable();
        let indices = UInt32Array::from(selected);
        columns = columns
            .iter()
            .map(|column| take(column, &indices, None))
            .collect::<Result<Vec<_>, _>>()?;
    }
    let batch = RecordBatch::try_new(history_schema(), columns)?;
    let mut columns = batch.columns().to_vec();
    let mut fields = batch.schema().fields().to_vec();
    for index in [1, 2] {
        let kind = arrow::datatypes::DataType::Dictionary(
            Box::new(arrow::datatypes::DataType::Int32),
            Box::new(arrow::datatypes::DataType::Utf8),
        );
        columns[index] = arrow::compute::cast(&columns[index], &kind)?;
        fields[index] = Arc::new(arrow::datatypes::Field::new(
            fields[index].name(),
            kind,
            false,
        ));
    }
    Ok(RecordBatch::try_new(
        Arc::new(arrow::datatypes::Schema::new(fields)),
        columns,
    )?)
}
fn history_page(batch: &RecordBatch, query: &HistoryQueryOptions) -> QueryPage<RecordBatch> {
    let count = batch.num_rows();
    let start = query.cursor.unwrap_or(0).min(count);
    let length = query.limit.unwrap_or(count).min(count - start);
    QueryPage {
        rows: vec![batch.slice(start, length)],
        next_cursor: (start + length < count).then_some(start + length),
    }
}

#[cfg(test)]
mod durable_tests {
    use super::*;
    use crate::{
        app_state::AppState,
        catalog::RegisterRun,
        schema::scalar_schema,
        telemetry::{BatchEnvelope, ENVELOPE_KEY, TelemetryKind},
    };
    use arrow::array::{Float64Array, StringArray, TimestampMillisecondArray};
    #[test]
    fn overview_preserves_finite_extrema_and_gaps_within_its_budget() {
        let mut values = vec![1.; 50];
        values[5] = f64::NAN;
        values[20] = 1000.;
        values[30] = -1000.;
        let batch = RecordBatch::try_new(
            history_schema(),
            vec![
                Arc::new(TimestampMillisecondArray::from_iter_values(0..50)),
                Arc::new(StringArray::from(vec!["r"; 50])),
                Arc::new(StringArray::from(vec!["loss"; 50])),
                Arc::new(Float64Array::from(values)),
                Arc::new(Float64Array::from(vec![None; 50])),
                Arc::new(StringArray::from(vec![None::<&str>; 50])),
            ],
        )
        .unwrap();
        let reduced = columnar_batch(
            vec![batch],
            &HistoryQueryOptions {
                max_points: Some(8),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(reduced.num_rows() <= 8);
        let values = reduced
            .column(3)
            .as_primitive::<arrow::datatypes::Float64Type>();
        assert!(values.values().iter().any(|v| v.is_nan()));
        assert!(values.values().contains(&1000.));
        assert!(values.values().contains(&-1000.));
        let times = reduced
            .column(0)
            .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
        assert_eq!(times.value(0), 0);
        assert_eq!(times.value(times.len() - 1), 49);
    }

    #[tokio::test]
    async fn group_commit_segments_retries_restarts_and_decimation() {
        let root =
            std::env::temp_dir().join(format!("aeroboard-segments-{}", uuid::Uuid::new_v4()));
        let config = AppConfig::for_tests(root.join("data"), root.join("web"));
        let state = AppState::new(
            config.clone(),
            Arc::new(super::super::FilesystemStore::new(config.data_dir.clone()).unwrap()),
        )
        .unwrap()
        .0;
        state.storage.initialize().await.unwrap();
        let uid = uuid::Uuid::new_v4().to_string();
        let sid = uuid::Uuid::new_v4().to_string();
        let register_uid = uid.clone();
        let start_uid = uid.clone();
        let start_sid = sid.clone();
        state
            .catalog
            .write(move |db| {
                catalog::register(
                    db,
                    RegisterRun {
                        run_uid: register_uid,
                        name: "segment-run".into(),
                        project_id: "default".into(),
                        tags: vec![],
                        provenance: json!({}),
                    },
                    BTreeSet::new(),
                )?;
                catalog::start_session(
                    db,
                    &start_uid,
                    &json!({"session_id":start_sid,"historical":true}),
                )?;
                Ok(())
            })
            .await
            .unwrap();
        // A dashboard/hyperparameter batch can precede the first scalar batch.
        state
            .catalog
            .quiet(|db| {
                let mut accumulator = crate::summaries::RunAccumulator::default();
                accumulator.set_hyperparameter(HyperparameterEntry {
                    key: "lr".into(),
                    value: "0.1".into(),
                    value_type: "float".into(),
                });
                crate::summaries::save_accumulator(db, "segment-run", &accumulator)?;
                db.execute("INSERT INTO data_revisions VALUES('segment-run',1)", [])?;
                Ok(())
            })
            .await
            .unwrap();
        let mut batches = vec![];
        for index in 0..20 {
            let batch = RecordBatch::try_new(
                scalar_schema(),
                vec![
                    Arc::new(TimestampMillisecondArray::from(vec![
                        index * 2,
                        index * 2 + 1,
                    ])),
                    Arc::new(StringArray::from(vec!["segment-run"; 2])),
                    Arc::new(StringArray::from(vec!["loss"; 2])),
                    Arc::new(Float64Array::from(vec![
                        index as f64,
                        if index == 10 { 1000. } else { index as f64 },
                    ])),
                    Arc::new(Float64Array::from(vec![
                        Some((index * 2) as f64),
                        Some((index * 2 + 1) as f64),
                    ])),
                ],
            )
            .unwrap();
            let envelope = BatchEnvelope {
                version: 2,
                batch_id: uuid::Uuid::new_v4().to_string(),
                run_uid: uid.clone(),
                session_id: sid.clone(),
                server_id: state.catalog.store_id.clone(),
                sequence: index + 1,
                checksum: crate::telemetry::batch_checksum_v2(&batch).unwrap(),
                captured_at_ms: 0,
                segment: None,
            };
            let schema = Arc::new(
                batch
                    .schema()
                    .as_ref()
                    .clone()
                    .with_metadata(HashMap::from([(
                        ENVELOPE_KEY.into(),
                        serde_json::to_string(&envelope).unwrap(),
                    )])),
            );
            batches.push(Arc::new(TelemetryBatch::new(
                TelemetryKind::Scalars,
                Arc::new(RecordBatch::try_new(schema, batch.columns().to_vec()).unwrap()),
            )));
        }
        assert!(
            state
                .storage
                .append_group(batches.clone())
                .await
                .iter()
                .all(|r| matches!(r, Ok(true)))
        );
        assert!(
            state
                .storage
                .append_group(batches.clone())
                .await
                .iter()
                .all(|r| matches!(r, Ok(false)))
        );
        let files = std::fs::read_dir(config.data_dir.join("segments").join(&uid).join("scalars"))
            .unwrap()
            .count();
        assert_eq!(files, 1);
        let query = HistoryQueryOptions {
            run_ids: BTreeSet::from(["segment-run".into()]),
            ..Default::default()
        };
        assert_eq!(
            state
                .storage
                .query_history(&query)
                .await
                .unwrap()
                .rows
                .len(),
            40
        );
        let segment =
            std::fs::read_dir(config.data_dir.join("segments").join(&uid).join("scalars"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
        let backup = segment.with_extension("test-backup");
        std::fs::rename(&segment, &backup).unwrap();
        // Finished-run pages reuse one columnar snapshot instead of opening payloads again.
        let cached = state
            .storage
            .query_history(&HistoryQueryOptions {
                cursor: Some(1),
                limit: Some(2),
                ..query.clone()
            })
            .await
            .unwrap();
        assert_eq!(cached.rows[0].ts_ms, 1);
        state
            .catalog
            .quiet(|db| {
                db.execute(
                    "UPDATE data_revisions SET revision=revision+1 WHERE run_id='segment-run'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(state.storage.query_history(&query).await.is_err());
        std::fs::rename(&backup, &segment).unwrap();
        let reduced = state
            .storage
            .query_history(&HistoryQueryOptions {
                max_points: Some(8),
                ..query.clone()
            })
            .await
            .unwrap()
            .rows;
        assert!(reduced.len() <= 8);
        assert_eq!(reduced.first().unwrap().ts_ms, 0);
        assert_eq!(reduced.last().unwrap().ts_ms, 39);
        assert!(reduced.iter().any(|p| p.value == 1000.));
        let page = state
            .storage
            .query_history(&HistoryQueryOptions {
                limit: Some(3),
                cursor: Some(1),
                ..query.clone()
            })
            .await
            .unwrap();
        assert_eq!(page.rows[0].ts_ms, 1);
        assert_eq!(page.next_cursor, Some(4));
        let conflict = batches[0].as_ref();
        let mut envelope = conflict.envelope.clone().unwrap();
        envelope.batch_id = uuid::Uuid::new_v4().to_string();
        let schema = Arc::new(conflict.batch.schema().as_ref().clone().with_metadata(
            HashMap::from([(
                ENVELOPE_KEY.into(),
                serde_json::to_string(&envelope).unwrap(),
            )]),
        ));
        let conflict = Arc::new(TelemetryBatch::new(
            TelemetryKind::Scalars,
            Arc::new(RecordBatch::try_new(schema, conflict.batch.columns().to_vec()).unwrap()),
        ));
        assert!(state.storage.append_group(vec![conflict]).await[0].is_err());
        let scores = state
            .catalog
            .read(|db| {
                let body: Vec<u8> = db.query_row(
                    "SELECT body FROM run_scores WHERE run_id='segment-run'",
                    [],
                    |r| r.get(0),
                )?;
                Ok(serde_json::from_slice::<Value>(&body).unwrap())
            })
            .await
            .unwrap();
        assert_eq!(scores["metrics"]["loss"]["score"]["finite_count"], 40);
        drop(state);
        let state = AppState::new(
            config.clone(),
            Arc::new(super::super::FilesystemStore::new(config.data_dir.clone()).unwrap()),
        )
        .unwrap()
        .0;
        state.storage.initialize().await.unwrap();
        assert_eq!(
            state
                .storage
                .query_history(&query)
                .await
                .unwrap()
                .rows
                .len(),
            40
        );
        let summaries = state
            .storage
            .persisted_summaries(&query.run_ids)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summaries[0].metric_summaries[0].max, 1000.);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
