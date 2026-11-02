use std::collections::BTreeSet;
use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, StringDictionaryBuilder, TimestampMillisecondArray};
use arrow::datatypes::{DataType, Field, Int32Type, Schema, TimeUnit};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use axum::{
    body::{Body, Bytes},
    extract::{Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde::Serialize;

use crate::app_state::AppState;
use crate::storage::{
    BoxError, HistoryQueryOptions, ImageFrameQuery, ImageQueryOptions, MediaKind, MediaPayload,
    MediaSource,
};

/// Content type of `GET /api/history?format=arrow` responses.
pub const ARROW_STREAM_CONTENT_TYPE: &str = "application/vnd.apache.arrow.stream";
/// Pagination cursor header for Arrow responses (JSON responses carry `next_cursor`).
pub const NEXT_CURSOR_HEADER: &str = "x-aeroboard-next-cursor";

/// Standard history query parameters.
///
/// All fields are optional:
/// - `run_id`: a run ID or comma-separated list
/// - `metric`: a metric name or comma-separated list
/// - `start_ms` / `end_ms`: inclusive timestamp bounds in milliseconds
/// - `limit` / `cursor`: offset-style pagination; no `limit` returns everything
/// - `format`: `json` (default) or `arrow` for an Arrow IPC stream with dictionary-
///   encoded `run_id`/`name` columns, which is far smaller for long runs
///
/// Errors are reported with a non-2xx status and an `error` field in the JSON body.
#[derive(Debug, Deserialize, Default)]
pub struct HistoryQuery {
    pub run_id: Option<String>,
    pub metric: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
    pub include_summaries: Option<bool>,
    pub format: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ImageHistoryQuery {
    pub run_id: Option<String>,
    pub metric: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
    pub include_data: Option<bool>,
}

/// Query for `/api/history/media`. Returns metadata only; payload bytes are
/// fetched lazily from `/api/history/media/content`.
#[derive(Debug, Deserialize)]
pub struct MediaHistoryQuery {
    pub kind: MediaKind,
    pub run_id: Option<String>,
    pub metric: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct MediaContentQuery {
    pub kind: MediaKind,
    pub run_id: String,
    pub name: String,
    pub ts_ms: i64,
}

#[derive(Debug, Deserialize, Default)]
pub struct RunSummaryQuery {
    pub run_id: Option<String>,
}

/// Response from /api/history endpoint.
#[derive(Serialize)]
pub struct HistoryResponse {
    pub rows: Vec<HistoryPoint>,
    pub run_summaries: Vec<RunSummary>,
    pub next_cursor: Option<usize>,
    pub error: Option<String>,
}

/// Response from /api/history/images and /api/history/media.
#[derive(Serialize)]
pub struct ImageHistoryResponse {
    pub rows: Vec<ImageHistoryPoint>,
    pub next_cursor: Option<usize>,
    pub error: Option<String>,
}

/// Response from /api/runs endpoint.
#[derive(Serialize)]
pub struct RunsResponse {
    pub runs: Vec<String>,
}

/// Response from /api/runs/resolve endpoint.
#[derive(Serialize)]
pub struct ResolveRunIdResponse {
    pub run_id: String,
}

/// Response from DELETE /api/history endpoint.
#[derive(Serialize)]
pub struct DeleteRunsResponse {
    pub deleted: bool,
    pub error: Option<String>,
}

/// A single scalar data point returned from persisted history.
#[derive(Serialize, Clone, Debug)]
pub struct HistoryPoint {
    pub ts_ms: i64,
    pub run_id: String,
    pub name: String,
    pub value: f64,
    /// Training step logged alongside the value, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

/// A single image data point returned from image history.
#[derive(Serialize, Clone)]
pub struct ImageHistoryPoint {
    pub ts_ms: i64,
    pub run_id: String,
    pub name: String,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
}

/// Summary of hyperparameters for a run.
#[derive(Serialize, Clone, Debug)]
pub struct HyperparameterEntry {
    pub key: String,
    pub value: String,
    pub value_type: String,
}

/// Summary of metrics for a run.
#[derive(Serialize, Clone, Debug)]
pub struct MetricSummary {
    pub name: String,
    pub last: f64,
    pub min: f64,
    pub max: f64,
}

/// Summary of a complete run including hyperparameters and metrics.
#[derive(Serialize, Clone, Debug)]
pub struct RunSummary {
    pub run_id: String,
    pub hyperparameters: Vec<HyperparameterEntry>,
    pub metric_summaries: Vec<MetricSummary>,
}

#[derive(Serialize)]
pub struct RunSummariesResponse {
    pub run_summaries: Vec<RunSummary>,
    pub error: Option<String>,
}

/// Request payload for /api/runs/resolve endpoint.
#[derive(Debug, Deserialize)]
pub struct ResolveRunIdRequest {
    pub base_run_id: String,
}

/// Resolve the run ID to use for a requested base name.
///
/// Returns the base name if it is unused, otherwise the next free version:
/// `name` → `name-v2` → `name-v3` … A Lightning framework suffix (`-pl2.5.1`) is
/// kept at the end: `exp-pl2.5.1` → `exp-v2-pl2.5.1`.
pub fn resolve_run_id(base: &str, existing: &BTreeSet<String>) -> String {
    let normalized_base = normalize_run_id(base);
    let existing_versions = extract_version_from_existing(existing, &normalized_base);

    if !existing_versions.contains(&1) {
        return normalized_base;
    }

    let next_version = existing_versions.iter().copied().max().unwrap_or(1) + 1;
    let (base_without_suffix, framework_suffix) = split_framework_suffix(&normalized_base);
    format!("{base_without_suffix}-v{next_version}{framework_suffix}")
}

/// Normalize a run ID by trimming whitespace.
fn normalize_run_id(run_id: &str) -> String {
    run_id.trim().to_string()
}

/// Split off a trailing Lightning version suffix: `-pl` followed by a dotted
/// version number (`resnet-pl2.5.1` → `("resnet", "-pl2.5.1")`). Names that merely
/// contain `-pl` (`sample-placement`) have no suffix.
fn split_framework_suffix(candidate: &str) -> (String, String) {
    if let Some((prefix, version)) = candidate.rsplit_once("-pl") {
        let is_version = !version.is_empty()
            && version.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
        if is_version && !prefix.is_empty() {
            return (prefix.to_string(), format!("-pl{version}"));
        }
    }
    (candidate.to_string(), String::new())
}

/// Extract the version number from a candidate run ID relative to a base run ID.
fn extract_version(candidate: &str, base: &str) -> Option<usize> {
    let (candidate_base, candidate_suffix) = split_framework_suffix(candidate);
    let (base_without_suffix, base_suffix) = split_framework_suffix(base);

    if candidate_suffix != base_suffix {
        return None;
    }

    if candidate_base == base_without_suffix {
        return Some(1);
    }

    let version_prefix = format!("{base_without_suffix}-v");
    candidate_base
        .strip_prefix(&version_prefix)
        .and_then(|suffix| suffix.parse::<usize>().ok())
}

/// Extract all version numbers from existing runs that match a base run ID.
fn extract_version_from_existing(existing: &BTreeSet<String>, base: &str) -> BTreeSet<usize> {
    existing
        .iter()
        .filter_map(|candidate| extract_version(candidate, base))
        .collect()
}

/// Build run summaries from history points and hyperparameters.
#[cfg(test)]
pub fn build_run_summaries(points: &[HistoryPoint], hyperparameters: Vec<(String, HyperparameterEntry)>) -> Vec<RunSummary> {
    crate::summaries::accumulate(points, hyperparameters)
        .into_iter()
        .map(|(run_id, accumulator)| accumulator.to_summary(&run_id))
        .collect()
}

pub fn csv_set(value: Option<&str>) -> BTreeSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn build_history_query_options(query: &HistoryQuery) -> HistoryQueryOptions {
    HistoryQueryOptions {
        run_ids: csv_set(query.run_id.as_deref()),
        metrics: csv_set(query.metric.as_deref()),
        start_ms: query.start_ms,
        end_ms: query.end_ms,
        limit: query.limit,
        cursor: query.cursor,
    }
}

pub fn build_image_query_options(query: &ImageHistoryQuery) -> ImageQueryOptions {
    ImageQueryOptions {
        run_ids: csv_set(query.run_id.as_deref()),
        metrics: csv_set(query.metric.as_deref()),
        start_ms: query.start_ms,
        end_ms: query.end_ms,
        limit: query.limit,
        cursor: query.cursor,
        include_data: query.include_data.unwrap_or(false),
    }
}

/// A JSON error body with a 500 status (or the given status).
fn json_error<T: Serialize>(status: StatusCode, context: &str, error: &BoxError, body: impl FnOnce(String) -> T) -> Response {
    let msg = error.to_string();
    tracing::error!("{context}: {msg}");
    (status, Json(body(msg))).into_response()
}

/// Encode history rows as an Arrow IPC stream. `run_id` and `name` are
/// dictionary-encoded, so each distinct string is sent once.
pub fn history_rows_to_arrow(rows: &[HistoryPoint]) -> Result<Vec<u8>, arrow::error::ArrowError> {
    let dictionary = DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
    let schema = Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("run_id", dictionary.clone(), false),
        Field::new("name", dictionary, false),
        Field::new("value", DataType::Float64, false),
        Field::new("step", DataType::Float64, true),
    ]));

    let mut run_ids = StringDictionaryBuilder::<Int32Type>::new();
    let mut names = StringDictionaryBuilder::<Int32Type>::new();
    for row in rows {
        run_ids.append_value(&row.run_id);
        names.append_value(&row.name);
    }
    let columns: Vec<ArrayRef> = vec![
        Arc::new(TimestampMillisecondArray::from_iter_values(rows.iter().map(|row| row.ts_ms))),
        Arc::new(run_ids.finish()),
        Arc::new(names.finish()),
        Arc::new(Float64Array::from_iter_values(rows.iter().map(|row| row.value))),
        Arc::new(Float64Array::from_iter(rows.iter().map(|row| row.step))),
    ];
    let batch = RecordBatch::try_new(schema.clone(), columns)?;

    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, &schema)?;
    writer.write(&batch)?;
    writer.finish()?;
    drop(writer);
    Ok(buf)
}

/// GET /api/history handler
pub async fn history_handler(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Response {
    let storage_query = build_history_query_options(&query);
    let as_arrow = query.format.as_deref().is_some_and(|format| format.eq_ignore_ascii_case("arrow"));

    let page = match state.storage.query_history(&storage_query).await {
        Ok(page) => page,
        Err(error) => {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to query history", &error, |msg| {
                HistoryResponse { rows: vec![], run_summaries: vec![], next_cursor: None, error: Some(msg) }
            });
        }
    };

    if as_arrow {
        return match history_rows_to_arrow(&page.rows) {
            Ok(bytes) => {
                let mut builder = Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, ARROW_STREAM_CONTENT_TYPE)
                    .header(header::CACHE_CONTROL, "no-store");
                if let Some(cursor) = page.next_cursor {
                    builder = builder.header(NEXT_CURSOR_HEADER, cursor.to_string());
                }
                builder.body(Body::from(bytes)).unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
            }
            Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode history", &error.into(), |msg| {
                HistoryResponse { rows: vec![], run_summaries: vec![], next_cursor: None, error: Some(msg) }
            }),
        };
    }

    // Summaries cover whole runs, not just the requested page.
    let run_summaries = if query.include_summaries.unwrap_or(true) {
        match state.summaries.summaries(state.storage.as_ref(), &storage_query.run_ids).await {
            Ok(summaries) => summaries,
            Err(error) => {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to build run summaries", &error, |msg| {
                    HistoryResponse { rows: vec![], run_summaries: vec![], next_cursor: None, error: Some(msg) }
                });
            }
        }
    } else {
        vec![]
    };

    Json(HistoryResponse { rows: page.rows, run_summaries, next_cursor: page.next_cursor, error: None }).into_response()
}

/// GET /api/runs/summary handler
pub async fn run_summaries_handler(
    State(state): State<AppState>,
    Query(query): Query<RunSummaryQuery>,
) -> Response {
    let run_ids = csv_set(query.run_id.as_deref());
    match state.summaries.summaries(state.storage.as_ref(), &run_ids).await {
        Ok(run_summaries) => Json(RunSummariesResponse { run_summaries, error: None }).into_response(),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to build run summaries", &error, |msg| {
            RunSummariesResponse { run_summaries: vec![], error: Some(msg) }
        }),
    }
}

/// GET /api/history/images handler
pub async fn image_history_handler(
    State(state): State<AppState>,
    Query(query): Query<ImageHistoryQuery>,
) -> Response {
    let storage_query = build_image_query_options(&query);

    match state.storage.query_image_history(&storage_query).await {
        Ok(page) => Json(ImageHistoryResponse { rows: page.rows, next_cursor: page.next_cursor, error: None }).into_response(),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to query image history", &error, |msg| {
            ImageHistoryResponse { rows: vec![], next_cursor: None, error: Some(msg) }
        }),
    }
}

/// Live run IDs plus every persisted run.
async fn known_runs(state: &AppState) -> BTreeSet<String> {
    let mut runs: BTreeSet<String> = state.t_junction.current_runs().into_iter().collect();
    match state.storage.list_runs().await {
        Ok(stored_runs) => runs.extend(stored_runs),
        Err(error) => tracing::warn!("Failed to list persisted runs: {}", error),
    }
    runs
}

/// GET /api/runs handler
pub async fn runs_handler(State(state): State<AppState>) -> Json<RunsResponse> {
    Json(RunsResponse { runs: known_runs(&state).await.into_iter().collect() })
}

/// POST /api/runs/resolve handler
///
/// Resolves and reserves the run ID for a base name, so two training processes
/// starting at the same moment with the same name get different versions.
pub async fn resolve_run_id_handler(
    State(state): State<AppState>,
    Json(payload): Json<ResolveRunIdRequest>,
) -> Json<ResolveRunIdResponse> {
    let requested = normalize_run_id(&payload.base_run_id);
    let existing = known_runs(&state).await;
    // Resolve-and-reserve happens under the run registry lock via `reserve_run`.
    let run_id = state.t_junction.reserve_run(|live| {
        let mut all = existing;
        all.extend(live.iter().cloned());
        resolve_run_id(&requested, &all)
    });
    Json(ResolveRunIdResponse { run_id })
}

/// DELETE /api/history handler
///
/// Clears persisted history, cached summaries and the live run registry.
pub async fn delete_history_handler(State(state): State<AppState>) -> (StatusCode, Json<DeleteRunsResponse>) {
    if let Err(error) = state.storage.clear_history().await {
        let msg = error.to_string();
        tracing::error!("Failed to delete persisted history: {}", msg);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(DeleteRunsResponse { deleted: false, error: Some(msg) }),
        );
    }

    state.summaries.clear().await;
    state.t_junction.clear_live_runs();

    (StatusCode::OK, Json(DeleteRunsResponse { deleted: true, error: None }))
}

/// GET /api/history/media?kind=audio|video|image
///
/// Lists media items without payloads so the dashboard can render a timeline
/// cheaply and only download the items the user actually views.
pub async fn media_history_handler(
    State(state): State<AppState>,
    Query(query): Query<MediaHistoryQuery>,
) -> Response {
    let storage_query = ImageQueryOptions {
        run_ids: csv_set(query.run_id.as_deref()),
        metrics: csv_set(query.metric.as_deref()),
        start_ms: query.start_ms,
        end_ms: query.end_ms,
        limit: query.limit,
        cursor: query.cursor,
        include_data: false,
    };

    match state.storage.query_media_history(query.kind, &storage_query).await {
        Ok(page) => Json(ImageHistoryResponse { rows: page.rows, next_cursor: page.next_cursor, error: None }).into_response(),
        Err(error) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("Failed to query {:?} history", query.kind), &error, |msg| {
            ImageHistoryResponse { rows: vec![], next_cursor: None, error: Some(msg) }
        }),
    }
}

pub async fn media_content_handler(
    State(state): State<AppState>,
    Query(query): Query<MediaContentQuery>,
    request: Request,
) -> Response {
    let frame_query = ImageFrameQuery {
        run_id: query.run_id,
        name: query.name,
        ts_ms: query.ts_ms,
    };

    let payload = match state.storage.load_media(query.kind, &frame_query).await {
        Ok(Some(payload)) => payload,
        Ok(None) => return media_error(StatusCode::NOT_FOUND, "media item not found"),
        Err(error) => {
            tracing::error!("Failed to load {:?} payload: {}", query.kind, error);
            return media_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
    };

    let MediaPayload { format, source } = payload;
    let mime = mime_guess::from_ext(&format).first_or_octet_stream();

    let mut response = match source {
        MediaSource::File(path) => {
            match tower_http::services::ServeFile::new_with_mime(path, &mime).try_call(request).await {
                Ok(response) => response.map(Body::new),
                Err(error) => {
                    tracing::error!("Failed to stream media file: {}", error);
                    return media_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read media file");
                }
            }
        }
        MediaSource::Bytes(bytes) => bytes_response(bytes, mime.as_ref(), request.headers()),
    };

    let cacheable = response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED;
    let headers = response.headers_mut();
    if cacheable {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"));
    }
    // Payloads are user-supplied; never let the browser sniff or execute them
    // (e.g. an "svg"/"html" format opened directly in a tab).
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; media-src 'self'; img-src 'self'; sandbox"),
    );
    response
}

fn media_error(status: StatusCode, message: &str) -> Response {
    let mut response = (status, Json(serde_json::json!({ "error": message }))).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Build a response for an in-memory payload, honouring a single `Range: bytes=` header.
fn bytes_response(bytes: Vec<u8>, content_type: &str, request_headers: &HeaderMap) -> Response {
    // `Bytes` lets range responses share the buffer instead of copying the slice.
    let bytes = Bytes::from(bytes);
    let total = bytes.len() as u64;
    let content_type = HeaderValue::from_str(content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));

    let range = request_headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(|value| parse_byte_range(value, total));

    let builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes");

    let response = match range {
        Some(Some((start, end))) => builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total}"))
            .header(header::CONTENT_LENGTH, end - start + 1)
            .body(Body::from(bytes.slice(start as usize..=end as usize))),
        Some(None) => builder
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .body(Body::empty()),
        None => builder
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, total)
            .body(Body::from(bytes)),
    };

    response.unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Parse a single-range `bytes=` header into an inclusive `(start, end)`.
/// Returns `None` when the range cannot be satisfied. For multi-range requests
/// only the first range is served.
pub(crate) fn parse_byte_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    if total == 0 {
        return None;
    }

    let (start, end) = match (start.trim(), end.trim()) {
        ("", suffix) => {
            let suffix: u64 = suffix.parse().ok()?;
            if suffix == 0 {
                return None;
            }
            (total.saturating_sub(suffix), total - 1)
        }
        (start, "") => (start.parse().ok()?, total - 1),
        (start, end) => (start.parse().ok()?, end.parse::<u64>().ok()?.min(total - 1)),
    };

    (start <= end && start < total).then_some((start, end))
}
