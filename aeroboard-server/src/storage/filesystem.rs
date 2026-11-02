//! Filesystem storage backend.
//!
//! Layout under `<root>/runs/<urlencoded run_id>/`:
//!
//! ```text
//! manifest.json                         {"run_id": "..."}
//! scalars/index.jsonl                   one JSON object per point
//! hyperparameters/index.jsonl           one JSON object per entry (last write wins)
//! <images|audio|video>/<name>/index.jsonl + one file per media item
//! ```
//!
//! Indexes are append-only JSON Lines, so persisting a batch costs O(batch) no
//! matter how long the run is. All disk I/O runs on tokio's blocking pool.
//!
//! Older layouts are still read: `index.json` arrays (rewritten on every batch by
//! earlier versions) and raw `*.arrow` batch files (from the first versions).
//! A torn final line after a crash is skipped with a warning rather than failing
//! the whole query.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use arrow::{
    array::{Array, AsArray},
    ipc::reader::FileReader,
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
    codec::{parse_hparam_batch, parse_image_batch, parse_scalar_batch, sanitize_extension},
    query::{apply_history_query, apply_image_query},
    BoxError, HistoryQueryOptions, ImageFrameQuery, ImageQueryOptions, MediaKind, MediaPayload,
    MediaSource, QueryPage, StorageEngine,
};

const INDEX_FILE: &str = "index.jsonl";
const LEGACY_INDEX_FILE: &str = "index.json";
const SCALARS_DIR: &str = "scalars";
const HPARAMS_DIR: &str = "hyperparameters";

#[derive(Clone)]
pub struct FilesystemStore {
    inner: Arc<FsInner>,
}

struct FsInner {
    root: PathBuf,
    /// Serialises writers with `clear_history`. Readers don't take it: appends are
    /// whole-line writes and readers tolerate a torn last line.
    write_lock: Mutex<()>,
    /// Runs whose manifest is known to exist, so appends skip the `stat`.
    known_manifests: Mutex<HashSet<String>>,
}

#[derive(Serialize, Deserialize)]
struct RunManifest {
    run_id: String,
}

/// One scalar point in `scalars/index.jsonl`. The run is implied by the directory.
#[derive(Serialize, Deserialize)]
struct ScalarLine {
    t: i64,
    n: String,
    #[serde(with = "lossless_f64")]
    v: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    s: Option<f64>,
}

/// Row of the legacy `scalars/index.json` array. Non-finite values were written as `null`.
#[derive(Deserialize)]
struct LegacyScalarEntry {
    ts_ms: i64,
    name: String,
    value: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone)]
struct MediaIndexEntry {
    ts_ms: i64,
    run_id: String,
    name: String,
    format: String,
    file_name: String,
}

#[derive(Serialize, Deserialize)]
struct HyperparameterIndexEntry {
    run_id: String,
    key: String,
    value: String,
    value_type: String,
}

/// JSON has no NaN/Infinity; store them as strings so diverged runs round-trip.
mod lossless_f64 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        if value.is_finite() {
            serializer.serialize_f64(*value)
        } else if value.is_nan() {
            serializer.serialize_str("NaN")
        } else if *value > 0.0 {
            serializer.serialize_str("inf")
        } else {
            serializer.serialize_str("-inf")
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Number(f64),
            Text(String),
        }
        match Repr::deserialize(deserializer)? {
            Repr::Number(value) => Ok(value),
            Repr::Text(text) => match text.as_str() {
                "NaN" => Ok(f64::NAN),
                "inf" => Ok(f64::INFINITY),
                "-inf" => Ok(f64::NEG_INFINITY),
                other => Err(D::Error::custom(format!("invalid number {other:?}"))),
            },
        }
    }
}

impl FilesystemStore {
    pub fn new(root: PathBuf) -> Result<Self, BoxError> {
        fs::create_dir_all(root.join("runs"))?;
        Ok(Self {
            inner: Arc::new(FsInner {
                root,
                write_lock: Mutex::new(()),
                known_manifests: Mutex::new(HashSet::new()),
            }),
        })
    }

    /// Run filesystem work on the blocking pool so it never stalls async workers.
    async fn blocking<T, F>(&self, work: F) -> Result<T, BoxError>
    where
        T: Send + 'static,
        F: FnOnce(&FsInner) -> Result<T, BoxError> + Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || work(&inner)).await?
    }
}

// ---------------------------------------------------------------------------
// Low-level file helpers
// ---------------------------------------------------------------------------

/// Append rows as JSON lines with a single write. If a previous crash left a
/// torn line without its newline, start on a fresh line so new rows stay parseable.
fn append_lines<T: Serialize>(path: &Path, rows: &[T]) -> Result<(), BoxError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut file = fs::OpenOptions::new().create(true).read(true).append(true).open(path)?;

    let mut buf = Vec::with_capacity(rows.len() * 64);
    let len = file.metadata()?.len();
    if len > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::Start(len - 1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            buf.push(b'\n');
        }
    }
    for row in rows {
        serde_json::to_writer(&mut buf, row)?;
        buf.push(b'\n');
    }
    file.write_all(&buf)?;
    Ok(())
}

/// Read a JSON Lines file, skipping (and reporting) lines that don't parse.
fn read_lines<T: DeserializeOwned>(path: &Path, mut visit: impl FnMut(T)) -> Result<(), BoxError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    let mut skipped = 0usize;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(&line) {
            Ok(row) => visit(row),
            Err(_) => skipped += 1,
        }
    }
    if skipped > 0 {
        tracing::warn!("Skipped {skipped} unreadable line(s) in {}", path.display());
    }
    Ok(())
}

/// Read a legacy JSON array index.
fn read_legacy_array<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, BoxError> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(error) => Err(error.into()),
    }
}

fn has_index(dir: &Path) -> bool {
    dir.join(INDEX_FILE).exists() || dir.join(LEGACY_INDEX_FILE).exists()
}

/// Read every `*.arrow` batch file directly inside `dir` (oldest storage format).
fn read_arrow_batches(dir: &Path) -> Result<Vec<RecordBatch>, BoxError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.into()),
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().and_then(|ext| ext.to_str()) == Some("arrow") {
            paths.push(entry.path());
        }
    }
    paths.sort();

    let mut batches = Vec::new();
    for path in paths {
        for batch in FileReader::try_new(fs::File::open(&path)?, None)? {
            batches.push(batch?);
        }
    }
    Ok(batches)
}

/// Write via a temp file + rename so readers never see a half-written file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BoxError> {
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Store implementation (runs on the blocking pool)
// ---------------------------------------------------------------------------

impl FsInner {
    fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.runs_dir().join(urlencoding::encode(run_id).into_owned())
    }

    fn media_metric_dir(&self, kind: MediaKind, run_id: &str, name: &str) -> PathBuf {
        self.run_dir(run_id).join(kind.dir_name()).join(urlencoding::encode(name).into_owned())
    }

    fn ensure_manifest(&self, run_id: &str) -> Result<(), BoxError> {
        if self.known_manifests.lock().expect("manifest cache poisoned").contains(run_id) {
            return Ok(());
        }
        let run_dir = self.run_dir(run_id);
        fs::create_dir_all(&run_dir)?;
        let manifest_path = run_dir.join("manifest.json");
        if !manifest_path.exists() {
            let manifest = RunManifest { run_id: run_id.to_string() };
            write_atomic(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
        }
        self.known_manifests.lock().expect("manifest cache poisoned").insert(run_id.to_string());
        Ok(())
    }

    fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        let entries = match fs::read_dir(self.runs_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(error.into()),
        };

        let mut runs = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = entry.path().join("manifest.json");
            match fs::read(&manifest_path) {
                Ok(bytes) => match serde_json::from_slice::<RunManifest>(&bytes) {
                    Ok(manifest) => runs.push(manifest.run_id),
                    Err(error) => tracing::warn!("Ignoring unreadable {}: {error}", manifest_path.display()),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        runs.sort();
        runs.dedup();
        Ok(runs)
    }

    /// Runs to scan for a query: every run when the filter is empty.
    fn candidate_runs(&self, run_ids: &BTreeSet<String>) -> Result<Vec<String>, BoxError> {
        if run_ids.is_empty() {
            return self.list_runs();
        }
        Ok(run_ids.iter().filter(|run_id| self.run_dir(run_id).exists()).cloned().collect())
    }

    // -- writes -------------------------------------------------------------

    fn append_batch(&self, telemetry: &TelemetryBatch) -> Result<(), BoxError> {
        let _guard = self.write_lock.lock().expect("write lock poisoned");
        let batch = &telemetry.batch;
        if let Some(kind) = MediaKind::from_telemetry(&telemetry.kind) {
            return self.append_media(kind, batch);
        }
        match telemetry.kind {
            TelemetryKind::Scalars => self.append_scalars(batch),
            TelemetryKind::Hyperparameters => self.append_hyperparameters(batch),
            ref other => {
                tracing::debug!("Filesystem storage does not persist {:?} batches", other);
                Ok(())
            }
        }
    }

    fn append_scalars(&self, batch: &RecordBatch) -> Result<(), BoxError> {
        let timestamps = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
        let run_ids = batch.column(1).as_string::<i32>();
        let names = batch.column(2).as_string::<i32>();
        let values = batch.column(3).as_primitive::<arrow::datatypes::Float64Type>();
        let steps = batch.column(4).as_primitive::<arrow::datatypes::Float64Type>();

        let mut per_run: BTreeMap<&str, Vec<ScalarLine>> = BTreeMap::new();
        for i in 0..batch.num_rows() {
            if timestamps.is_null(i) || run_ids.is_null(i) || names.is_null(i) || values.is_null(i) {
                continue;
            }
            per_run.entry(run_ids.value(i)).or_default().push(ScalarLine {
                t: timestamps.value(i),
                n: names.value(i).to_string(),
                v: values.value(i),
                s: (!steps.is_null(i)).then(|| steps.value(i)),
            });
        }

        for (run_id, lines) in per_run {
            self.ensure_manifest(run_id)?;
            let dir = self.run_dir(run_id).join(SCALARS_DIR);
            fs::create_dir_all(&dir)?;
            append_lines(&dir.join(INDEX_FILE), &lines)?;
        }
        Ok(())
    }

    fn append_hyperparameters(&self, batch: &RecordBatch) -> Result<(), BoxError> {
        let mut per_run: BTreeMap<String, Vec<HyperparameterIndexEntry>> = BTreeMap::new();
        for (run_id, entry) in parse_hparam_batch(batch) {
            per_run.entry(run_id.clone()).or_default().push(HyperparameterIndexEntry {
                run_id,
                key: entry.key,
                value: entry.value,
                value_type: entry.value_type,
            });
        }

        for (run_id, entries) in per_run {
            self.ensure_manifest(&run_id)?;
            let dir = self.run_dir(&run_id).join(HPARAMS_DIR);
            fs::create_dir_all(&dir)?;
            append_lines(&dir.join(INDEX_FILE), &entries)?;
        }
        Ok(())
    }

    /// Persist each media payload as its own file, then record it in the
    /// per-stream index (payload first, so the index never points at a missing file).
    fn append_media(&self, kind: MediaKind, batch: &RecordBatch) -> Result<(), BoxError> {
        let timestamps = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
        let run_ids = batch.column(1).as_string::<i32>();
        let names = batch.column(2).as_string::<i32>();
        let payloads = batch.column(3).as_binary::<i32>();
        let formats = batch.column(4).as_string::<i32>();
        let mut pending: BTreeMap<(String, String), Vec<MediaIndexEntry>> = BTreeMap::new();

        for i in 0..batch.num_rows() {
            if timestamps.is_null(i) || run_ids.is_null(i) || names.is_null(i) || payloads.is_null(i) || formats.is_null(i) {
                continue;
            }
            let (ts_ms, run_id, name) = (timestamps.value(i), run_ids.value(i), names.value(i));
            let format = sanitize_extension(formats.value(i));

            let metric_dir = self.media_metric_dir(kind, run_id, name);
            fs::create_dir_all(&metric_dir)?;
            let file_name = format!("{}-{}.{}", ts_ms, Uuid::new_v4(), format);
            fs::write(metric_dir.join(&file_name), payloads.value(i))?;

            pending.entry((run_id.to_string(), name.to_string())).or_default().push(MediaIndexEntry {
                ts_ms,
                run_id: run_id.to_string(),
                name: name.to_string(),
                format,
                file_name,
            });
        }

        for ((run_id, name), entries) in pending {
            self.ensure_manifest(&run_id)?;
            append_lines(&self.media_metric_dir(kind, &run_id, &name).join(INDEX_FILE), &entries)?;
        }
        Ok(())
    }

    fn clear(&self) -> Result<(), BoxError> {
        let _guard = self.write_lock.lock().expect("write lock poisoned");
        let runs_dir = self.runs_dir();
        if runs_dir.exists() {
            fs::remove_dir_all(&runs_dir)?;
        }
        fs::create_dir_all(runs_dir)?;
        self.known_manifests.lock().expect("manifest cache poisoned").clear();
        Ok(())
    }

    // -- reads --------------------------------------------------------------

    fn query_history(&self, query: &HistoryQueryOptions) -> Result<QueryPage<HistoryPoint>, BoxError> {
        let keep = |name: &str, ts_ms: i64| {
            (query.metrics.is_empty() || query.metrics.contains(name))
                && query.start_ms.is_none_or(|start| ts_ms >= start)
                && query.end_ms.is_none_or(|end| ts_ms <= end)
        };

        let mut rows = Vec::new();
        for run_id in self.candidate_runs(&query.run_ids)? {
            let dir = self.run_dir(&run_id).join(SCALARS_DIR);
            if !has_index(&dir) {
                // Oldest format: raw Arrow batches only.
                for batch in read_arrow_batches(&dir)? {
                    rows.extend(parse_scalar_batch(&batch).into_iter().filter(|row| keep(&row.name, row.ts_ms)));
                }
                continue;
            }

            for entry in read_legacy_array::<LegacyScalarEntry>(&dir.join(LEGACY_INDEX_FILE))? {
                if let Some(value) = entry.value
                    && keep(&entry.name, entry.ts_ms) {
                        rows.push(HistoryPoint { ts_ms: entry.ts_ms, run_id: run_id.clone(), name: entry.name, value, step: None });
                    }
            }
            read_lines::<ScalarLine>(&dir.join(INDEX_FILE), |line| {
                if keep(&line.n, line.t) {
                    rows.push(HistoryPoint { ts_ms: line.t, run_id: run_id.clone(), name: line.n, value: line.v, step: line.s });
                }
            })?;
        }
        Ok(apply_history_query(rows, query))
    }

    fn query_hyperparameters(&self, run_ids: &BTreeSet<String>) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        let mut rows = Vec::new();
        for run_id in self.candidate_runs(run_ids)? {
            let dir = self.run_dir(&run_id).join(HPARAMS_DIR);
            if !has_index(&dir) {
                for batch in read_arrow_batches(&dir)? {
                    rows.extend(parse_hparam_batch(&batch));
                }
                continue;
            }

            let to_row = |entry: HyperparameterIndexEntry| {
                (entry.run_id, HyperparameterEntry { key: entry.key, value: entry.value, value_type: entry.value_type })
            };
            rows.extend(read_legacy_array::<HyperparameterIndexEntry>(&dir.join(LEGACY_INDEX_FILE))?.into_iter().map(to_row));
            read_lines::<HyperparameterIndexEntry>(&dir.join(INDEX_FILE), |entry| rows.push(to_row(entry)))?;
        }
        if !run_ids.is_empty() {
            rows.retain(|(run_id, _)| run_ids.contains(run_id));
        }
        Ok(rows)
    }

    fn read_media_index(&self, metric_dir: &Path) -> Result<Vec<MediaIndexEntry>, BoxError> {
        let mut entries = read_legacy_array::<MediaIndexEntry>(&metric_dir.join(LEGACY_INDEX_FILE))?;
        read_lines::<MediaIndexEntry>(&metric_dir.join(INDEX_FILE), |entry| entries.push(entry))?;
        Ok(entries)
    }

    fn query_media(&self, kind: MediaKind, query: &ImageQueryOptions) -> Result<QueryPage<ImageHistoryPoint>, BoxError> {
        let mut rows = Vec::new();
        for run_id in self.candidate_runs(&query.run_ids)? {
            let media_dir = self.run_dir(&run_id).join(kind.dir_name());
            let metric_dirs = match fs::read_dir(&media_dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };

            for metric_entry in metric_dirs {
                let metric_entry = metric_entry?;
                if !metric_entry.file_type()?.is_dir() {
                    continue;
                }
                let metric_dir = metric_entry.path();
                for entry in self.read_media_index(&metric_dir)? {
                    if !query.metrics.is_empty() && !query.metrics.contains(&entry.name) {
                        continue;
                    }
                    let image_base64 = if query.include_data {
                        match fs::read(metric_dir.join(&entry.file_name)) {
                            Ok(bytes) => Some(BASE64_STANDARD.encode(bytes)),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                            Err(error) => return Err(error.into()),
                        }
                    } else {
                        None
                    };
                    rows.push(ImageHistoryPoint {
                        ts_ms: entry.ts_ms,
                        run_id: entry.run_id,
                        name: entry.name,
                        format: entry.format,
                        image_base64,
                    });
                }
            }

            if kind == MediaKind::Image {
                // Oldest format: image batches stored as raw Arrow files.
                for batch in read_arrow_batches(&media_dir)? {
                    rows.extend(parse_image_batch(&batch, query.include_data));
                }
            }
        }
        Ok(apply_image_query(rows, query))
    }

    fn load_media(&self, kind: MediaKind, query: &ImageFrameQuery) -> Result<Option<MediaPayload>, BoxError> {
        let metric_dir = self.media_metric_dir(kind, &query.run_id, &query.name);
        let index = self.read_media_index(&metric_dir)?;
        if let Some(entry) = index
            .into_iter()
            .rev()
            .find(|entry| entry.ts_ms == query.ts_ms && entry.run_id == query.run_id && entry.name == query.name)
        {
            let file_path = metric_dir.join(&entry.file_name);
            return Ok(file_path.exists().then_some(MediaPayload { format: entry.format, source: MediaSource::File(file_path) }));
        }

        if kind == MediaKind::Image {
            // Oldest format: payload embedded in an Arrow batch file.
            for batch in read_arrow_batches(&self.run_dir(&query.run_id).join(kind.dir_name()))? {
                let timestamps = batch.column(0).as_primitive::<arrow::datatypes::TimestampMillisecondType>();
                let run_ids = batch.column(1).as_string::<i32>();
                let names = batch.column(2).as_string::<i32>();
                let payloads = batch.column(3).as_binary::<i32>();
                let formats = batch.column(4).as_string::<i32>();
                for i in 0..batch.num_rows() {
                    if !timestamps.is_null(i)
                        && timestamps.value(i) == query.ts_ms
                        && run_ids.value(i) == query.run_id
                        && names.value(i) == query.name
                    {
                        return Ok(Some(MediaPayload {
                            format: formats.value(i).to_string(),
                            source: MediaSource::Bytes(payloads.value(i).to_vec()),
                        }));
                    }
                }
            }
        }
        Ok(None)
    }
}

#[async_trait]
impl StorageEngine for FilesystemStore {
    async fn append_batch(&self, batch: Arc<TelemetryBatch>) -> Result<(), BoxError> {
        self.blocking(move |inner| inner.append_batch(&batch)).await
    }

    async fn query_history(&self, query: &HistoryQueryOptions) -> Result<QueryPage<HistoryPoint>, BoxError> {
        let query = query.clone();
        self.blocking(move |inner| inner.query_history(&query)).await
    }

    async fn query_hyperparameters(&self, run_ids: &BTreeSet<String>) -> Result<Vec<(String, HyperparameterEntry)>, BoxError> {
        let run_ids = run_ids.clone();
        self.blocking(move |inner| inner.query_hyperparameters(&run_ids)).await
    }

    async fn query_media_history(&self, kind: MediaKind, query: &ImageQueryOptions) -> Result<QueryPage<ImageHistoryPoint>, BoxError> {
        let query = query.clone();
        self.blocking(move |inner| inner.query_media(kind, &query)).await
    }

    async fn load_media(&self, kind: MediaKind, query: &ImageFrameQuery) -> Result<Option<MediaPayload>, BoxError> {
        let query = query.clone();
        self.blocking(move |inner| inner.load_media(kind, &query)).await
    }

    async fn list_runs(&self) -> Result<Vec<String>, BoxError> {
        self.blocking(|inner| inner.list_runs()).await
    }

    async fn clear_history(&self) -> Result<(), BoxError> {
        self.blocking(|inner| inner.clear()).await
    }
}
