use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use arrow::{
    ipc::{reader::FileReader, writer::FileWriter},
    record_batch::RecordBatch,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    history::{HistoryPoint, HyperparameterEntry, ImageHistoryPoint},
    telemetry::{TelemetryBatch, TelemetryKind},
};

use super::{
    codec::{
        extract_first_run_id, now_ms, parse_hparam_batch, parse_image_batch, parse_scalar_batch,
        sanitize_extension,
    },
    query::{apply_history_query, apply_image_query},
    BoxError, HistoryQueryOptions, ImageFramePayload, ImageFrameQuery, ImageQueryOptions,
    QueryPage, StorageEngine,
};

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

#[derive(Serialize, Deserialize)]
struct ScalarIndexEntry {
    ts_ms: i64,
    run_id: String,
    name: String,
    value: f64,
}

#[derive(Serialize, Deserialize)]
struct HyperparameterIndexEntry {
    run_id: String,
    key: String,
    value: String,
    value_type: String,
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
        self.runs_dir()
            .join(urlencoding::encode(run_id).into_owned())
    }

    fn image_metric_dir(&self, run_id: &str, metric_name: &str) -> PathBuf {
        self.run_dir_for(run_id)
            .join("images")
            .join(urlencoding::encode(metric_name).into_owned())
    }

    fn kind_dir_for(&self, run_id: &str, kind_dir: &str) -> PathBuf {
        self.run_dir_for(run_id).join(kind_dir)
    }

    fn kind_index_path(&self, run_id: &str, kind_dir: &str) -> PathBuf {
        self.kind_dir_for(run_id, kind_dir).join("index.json")
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

    fn run_dirs(&self, run_ids: &BTreeSet<String>) -> Result<Vec<PathBuf>, BoxError> {
        let runs_dir = self.runs_dir();
        if !runs_dir.exists() {
            return Ok(vec![]);
        }

        if run_ids.is_empty() {
            let mut dirs = Vec::new();
            for entry in fs::read_dir(runs_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    dirs.push(entry.path());
                }
            }
            return Ok(dirs);
        }

        Ok(run_ids
            .iter()
            .map(|run_id| self.run_dir_for(run_id))
            .filter(|path| path.exists())
            .collect())
    }

    fn list_runs_from_manifests(&self) -> Result<Vec<String>, BoxError> {
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

    fn candidate_run_ids(&self, run_ids: &BTreeSet<String>) -> Result<Vec<String>, BoxError> {
        if run_ids.is_empty() {
            return self.list_runs_from_manifests();
        }

        Ok(run_ids
            .iter()
            .filter(|run_id| self.run_dir_for(run_id).exists())
            .cloned()
            .collect())
    }

    fn partition_indexed_runs(
        &self,
        kind_dir: &str,
        run_ids: &BTreeSet<String>,
    ) -> Result<(Vec<String>, BTreeSet<String>), BoxError> {
        let mut indexed = Vec::new();
        let mut fallback = BTreeSet::new();

        for run_id in self.candidate_run_ids(run_ids)? {
            if self.kind_index_path(&run_id, kind_dir).exists() {
                indexed.push(run_id);
            } else {
                fallback.insert(run_id);
            }
        }

        Ok((indexed, fallback))
    }

    fn batch_file_paths(&self, kind_dir: &str, run_ids: &BTreeSet<String>) -> Result<Vec<PathBuf>, BoxError> {
        let mut files = Vec::new();

        for run_dir in self.run_dirs(run_ids)? {
            let kind_path = run_dir.join(kind_dir);
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

    fn read_batches(&self, kind_dir: &str, run_ids: &BTreeSet<String>) -> Result<Vec<RecordBatch>, BoxError> {
        let mut batches = Vec::new();
        for path in self.batch_file_paths(kind_dir, run_ids)? {
            let file = fs::File::open(&path)?;
            let reader = FileReader::try_new(file, None)?;
            for batch in reader {
                batches.push(batch?);
            }
        }
        Ok(batches)
    }

    fn read_index<T: DeserializeOwned>(&self, path: &Path) -> Result<Vec<T>, BoxError> {
        if !path.exists() {
            return Ok(vec![]);
        }

        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn write_index<T: Serialize>(&self, path: &Path, rows: &[T]) -> Result<(), BoxError> {
        fs::write(path, serde_json::to_vec_pretty(rows)?)?;
        Ok(())
    }

    fn append_scalar_index(&self, batch: &RecordBatch) -> Result<(), BoxError> {
        use arrow::array::{Array, AsArray};

        let timestamp_col = batch
            .column(0)
            .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
        let run_id_col = batch.column(1).as_string::<i32>();
        let name_col = batch.column(2).as_string::<i32>();
        let value_col = batch
            .column(3)
            .as_primitive::<arrow::datatypes::Float64Type>();
        let mut pending: BTreeMap<String, Vec<ScalarIndexEntry>> = BTreeMap::new();

        for i in 0..batch.num_rows() {
            if timestamp_col.is_null(i)
                || run_id_col.is_null(i)
                || name_col.is_null(i)
                || value_col.is_null(i)
            {
                continue;
            }

            let run_id = run_id_col.value(i).to_string();
            pending.entry(run_id.clone()).or_default().push(ScalarIndexEntry {
                ts_ms: timestamp_col.value(i),
                run_id,
                name: name_col.value(i).to_string(),
                value: value_col.value(i),
            });
        }

        for (run_id, mut entries) in pending {
            self.write_manifest(&run_id)?;
            let output_dir = self.kind_dir_for(&run_id, "scalars");
            fs::create_dir_all(&output_dir)?;
            let index_path = output_dir.join("index.json");
            let mut index = self.read_index::<ScalarIndexEntry>(&index_path)?;
            index.append(&mut entries);
            index.sort_by_key(|entry| entry.ts_ms);
            self.write_index(&index_path, &index)?;
        }

        Ok(())
    }

    fn append_hyperparameter_index(&self, batch: &RecordBatch) -> Result<(), BoxError> {
        use arrow::array::{Array, AsArray};

        let run_id_col = batch.column(1).as_string::<i32>();
        let key_col = batch.column(2).as_string::<i32>();
        let value_col = batch.column(3).as_string::<i32>();
        let value_type_col = batch.column(4).as_string::<i32>();
        let mut pending: BTreeMap<String, Vec<HyperparameterIndexEntry>> = BTreeMap::new();

        for i in 0..batch.num_rows() {
            if run_id_col.is_null(i)
                || key_col.is_null(i)
                || value_col.is_null(i)
                || value_type_col.is_null(i)
            {
                continue;
            }

            let run_id = run_id_col.value(i).to_string();
            pending
                .entry(run_id.clone())
                .or_default()
                .push(HyperparameterIndexEntry {
                    run_id,
                    key: key_col.value(i).to_string(),
                    value: value_col.value(i).to_string(),
                    value_type: value_type_col.value(i).to_string(),
                });
        }

        for (run_id, mut entries) in pending {
            self.write_manifest(&run_id)?;
            let output_dir = self.kind_dir_for(&run_id, "hyperparameters");
            fs::create_dir_all(&output_dir)?;
            let index_path = output_dir.join("index.json");
            let mut index = self.read_index::<HyperparameterIndexEntry>(&index_path)?;
            index.append(&mut entries);
            self.write_index(&index_path, &index)?;
        }

        Ok(())
    }

    fn append_image_batch(&self, batch: &RecordBatch) -> Result<(), BoxError> {
        use arrow::array::{Array, AsArray};

        let timestamp_col = batch
            .column(0)
            .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
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

    fn read_image_index(&self, path: &Path) -> Result<Vec<ImageIndexEntry>, BoxError> {
        self.read_index(path)
    }

    fn query_history_from_indexes(
        &self,
        query: &HistoryQueryOptions,
    ) -> Result<(Vec<HistoryPoint>, BTreeSet<String>), BoxError> {
        let (indexed_runs, fallback_runs) = self.partition_indexed_runs("scalars", &query.run_ids)?;
        let mut rows = Vec::new();

        for run_id in indexed_runs {
            let index_path = self.kind_index_path(&run_id, "scalars");
            rows.extend(
                self.read_index::<ScalarIndexEntry>(&index_path)?
                    .into_iter()
                    .map(|entry| HistoryPoint {
                        ts_ms: entry.ts_ms,
                        run_id: entry.run_id,
                        name: entry.name,
                        value: entry.value,
                    }),
            );
        }

        Ok((rows, fallback_runs))
    }

    fn query_hyperparameters_from_indexes(
        &self,
        run_ids: &BTreeSet<String>,
    ) -> Result<(Vec<(String, HyperparameterEntry)>, BTreeSet<String>), BoxError> {
        let (indexed_runs, fallback_runs) =
            self.partition_indexed_runs("hyperparameters", run_ids)?;
        let mut rows = Vec::new();

        for run_id in indexed_runs {
            let index_path = self.kind_index_path(&run_id, "hyperparameters");
            rows.extend(
                self.read_index::<HyperparameterIndexEntry>(&index_path)?
                    .into_iter()
                    .map(|entry| {
                        (
                            entry.run_id,
                            HyperparameterEntry {
                                key: entry.key,
                                value: entry.value,
                                value_type: entry.value_type,
                            },
                        )
                    }),
            );
        }

        Ok((rows, fallback_runs))
    }

    fn query_image_history_from_files(&self, query: &ImageQueryOptions) -> Result<Vec<ImageHistoryPoint>, BoxError> {
        let mut rows = Vec::new();
        for run_dir in self.run_dirs(&query.run_ids)? {
            let images_dir = run_dir.join("images");
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
                    rows.push(ImageHistoryPoint {
                        ts_ms: entry.ts_ms,
                        run_id: entry.run_id,
                        name: entry.name,
                        format: entry.format,
                        image_base64: if query.include_data {
                            let file_path = metric_dir.join(&entry.file_name);
                            if file_path.exists() {
                                Some(BASE64_STANDARD.encode(fs::read(file_path)?))
                            } else {
                                None
                            }
                        } else {
                            None
                        },
                    });
                }
            }
        }

        rows.sort_by_key(|row| row.ts_ms);
        Ok(rows)
    }

    fn load_image_frame_from_files(&self, query: &ImageFrameQuery) -> Result<Option<ImageFramePayload>, BoxError> {
        let metric_dir = self.image_metric_dir(&query.run_id, &query.name);
        let index_path = metric_dir.join("index.json");
        for entry in self.read_image_index(&index_path)? {
            if entry.ts_ms != query.ts_ms || entry.run_id != query.run_id || entry.name != query.name {
                continue;
            }

            let file_path = metric_dir.join(&entry.file_name);
            if !file_path.exists() {
                return Ok(None);
            }

            let bytes = fs::read(file_path)?;
            return Ok(Some(ImageFramePayload {
                format: entry.format,
                image_base64: BASE64_STANDARD.encode(bytes),
            }));
        }

        Ok(None)
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

        if *kind == TelemetryKind::Scalars {
            self.append_scalar_index(&batch.batch)?;
        } else if *kind == TelemetryKind::Hyperparameters {
            self.append_hyperparameter_index(&batch.batch)?;
        }

        let run_id =
            extract_first_run_id(&batch.batch).unwrap_or_else(|| "unknown_run".to_string());
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

    async fn query_history(&self, query: &HistoryQueryOptions) -> Result<QueryPage<HistoryPoint>, BoxError> {
        let (mut rows, fallback_runs) = self.query_history_from_indexes(query)?;
        if !fallback_runs.is_empty() || (query.run_ids.is_empty() && rows.is_empty()) {
            for batch in self.read_batches("scalars", &fallback_runs)? {
                rows.extend(parse_scalar_batch(&batch));
            }
        }
        Ok(apply_history_query(rows, query))
    }

    async fn query_hyperparameters(&self, run_ids: &BTreeSet<String>) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        let (mut rows, fallback_runs) = self.query_hyperparameters_from_indexes(run_ids)?;
        if !fallback_runs.is_empty() || (run_ids.is_empty() && rows.is_empty()) {
            for batch in self.read_batches("hyperparameters", &fallback_runs)? {
                rows.extend(parse_hparam_batch(&batch));
            }
        }
        if !run_ids.is_empty() {
            rows.retain(|(run_id, _)| run_ids.contains(run_id));
        }
        Ok(rows)
    }

    async fn query_image_history(&self, query: &ImageQueryOptions) -> Result<QueryPage<ImageHistoryPoint>, BoxError> {
        let mut rows = self.query_image_history_from_files(query)?;
        for batch in self.read_batches("images", &query.run_ids)? {
            rows.extend(parse_image_batch(&batch, query.include_data));
        }
        Ok(apply_image_query(rows, query))
    }

    async fn load_image_frame(&self, query: &ImageFrameQuery) -> Result<Option<ImageFramePayload>, BoxError> {
        self.load_image_frame_from_files(query)
    }

    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        self.list_runs_from_manifests()
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
