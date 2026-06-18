use axum::{extract::{Query, State}, http::StatusCode, Json};
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeSet;

use crate::app_state::AppState;
use crate::storage::{HistoryQueryOptions, ImageFramePayload, ImageFrameQuery, ImageQueryOptions};

/// Standard history query parameters.
/// 
/// All fields are optional for flexible querying:
/// - `run_id`: Filter by specific run ID or comma-separated list
/// - `metric`: Filter by specific metric name or comma-separated list
/// - `start_ms`: Start timestamp in milliseconds (inclusive)
/// - `end_ms`: End timestamp in milliseconds (inclusive)
/// - `limit`: Maximum number of results to return
/// - `cursor`: Pagination cursor (offset-style)
///
/// API contract:
/// - Backend-agnostic query interface that works with all storage backends
/// - Always returns explicit empty results rather than relying on frontend assumptions
/// - Error messages are descriptive and include backend-specific context
///
/// Cursor-based pagination semantics:
/// - `cursor = None`: Return first `limit` results from start of dataset
/// - `cursor = N`: Return next `limit` results after position `N`
/// - `limit = None`: Return all remaining results (unlimited)
/// - Cursor is position-based, not item-based for consistent pagination
#[derive(Debug, Deserialize, Default)]
pub struct HistoryQuery {
    pub run_id: Option<String>,
    pub metric: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
    pub include_summaries: Option<bool>,
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

#[derive(Debug, Deserialize)]
pub struct ImageFrameContentQuery {
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

/// Response from /api/history/images endpoint.
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
#[derive(Serialize, Clone)]
pub struct HistoryPoint {
    pub ts_ms: i64,
    pub run_id: String,
    pub name: String,
    pub value: f64,
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
#[derive(Serialize, Clone)]
pub struct HyperparameterEntry {
    pub key: String,
    pub value: String,
    pub value_type: String,
}

/// Summary of metrics for a run.
#[derive(Serialize, Clone)]
pub struct MetricSummary {
    pub name: String,
    pub last: f64,
    pub min: f64,
    pub max: f64,
}

/// Summary of a complete run including hyperparameters and metrics.
#[derive(Serialize, Clone)]
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

#[derive(Serialize)]
pub struct ImageFrameContentResponse {
    pub run_id: String,
    pub name: String,
    pub ts_ms: i64,
    pub format: String,
    pub image_base64: String,
    pub error: Option<String>,
}

/// Request payload for /api/runs/resolve endpoint.
#[derive(Debug, Deserialize)]
pub struct ResolveRunIdRequest {
    pub base_run_id: String,
}

/// Resolve the canonical run ID for a given base run ID.
///
/// This function queries the storage backend to find all runs matching the
/// base run ID (including versioned variants like `run_name-v1`, `run_name-v2`, etc.).
/// It returns the canonical run ID to use, which may be the base run ID itself or
/// a versioned variant if the base already exists.
pub fn resolve_run_id(base: &str, existing: &BTreeSet<String>) -> String {
    let normalized_base = normalize_run_id(base);
    let existing_versions = extract_version_from_existing(existing, &normalized_base);

    if existing_versions.is_empty() || !existing_versions.contains(&1) {
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

/// Split a run ID into base and framework suffix (e.g., "resnet-pl3.10" → ("resnet", "-pl3.10")).
fn split_framework_suffix(candidate: &str) -> (String, String) {
    match candidate.rsplit_once("-pl") {
        Some((prefix, suffix)) if !suffix.is_empty() => (prefix.to_string(), format!("-pl{}", suffix)),
        _ => (candidate.to_string(), String::new()),
    }
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
///
/// This function aggregates data points into per-run summaries, tracking
/// the last value, min, and max for each metric.
pub fn build_run_summaries(points: &[HistoryPoint], hyperparameters: Vec<(String, HyperparameterEntry)>) -> Vec<RunSummary> {
    let mut per_run_metrics: std::collections::BTreeMap<String, std::collections::BTreeMap<String, MetricStatAccumulator>> = std::collections::BTreeMap::new();
    for point in points {
        let metric_map = per_run_metrics.entry(point.run_id.clone()).or_default();
        let stats = metric_map.entry(point.name.clone()).or_insert_with(|| MetricStatAccumulator {
            last_ts_ms: point.ts_ms,
            last: point.value,
            min: point.value,
            max: point.value,
        });
        stats.update(point.ts_ms, point.value);
    }

    let mut per_run_hparams: std::collections::BTreeMap<String, std::collections::BTreeMap<String, HyperparameterEntry>> = std::collections::BTreeMap::new();
    for (run_id, entry) in hyperparameters {
        per_run_hparams.entry(run_id).or_default().insert(entry.key.clone(), entry);
    }

    let run_ids: std::collections::BTreeSet<String> = per_run_metrics.keys().chain(per_run_hparams.keys()).cloned().collect();
    run_ids.into_iter().map(|run_id| {
        let hyperparameters = per_run_hparams.remove(&run_id)
            .map(|entries| entries.into_values().collect())
            .unwrap_or_default();
        let metric_summaries = per_run_metrics.remove(&run_id)
            .map(|metrics| metrics.into_iter().map(|(name, acc)| MetricSummary {
                name,
                last: acc.last,
                min: acc.min,
                max: acc.max,
            }).collect())
            .unwrap_or_default();
        RunSummary { run_id, hyperparameters, metric_summaries }
    }).collect()
}

/// Aggregator for metric statistics.
struct MetricStatAccumulator {
    last_ts_ms: i64,
    last: f64,
    min: f64,
    max: f64,
}

impl MetricStatAccumulator {
    fn update(&mut self, ts_ms: i64, value: f64) {
        if ts_ms >= self.last_ts_ms {
            self.last_ts_ms = ts_ms;
            self.last = value;
        }
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }
}

/// Apply standard history filters to query results.
///
/// Filter semantics:
/// - Multiple run_id/metric values are joined with OR logic (CSV parsing)
/// - Timestamp range uses inclusive bounds on both ends
/// - Empty filters (missing fields) apply to all records
/// - Time window filtering only applies if bounds are specified
///
/// This function ensures consistent filtering behavior across all storage backends.
pub fn apply_history_filters(rows: Vec<HistoryPoint>, query: &HistoryQuery) -> Vec<HistoryPoint> {
    let run_ids = csv_set(query.run_id.as_deref());
    let metrics = csv_set(query.metric.as_deref());

    rows.into_iter()
        .filter(|row| {
            (run_ids.is_empty() || run_ids.contains(&row.run_id))
                && (metrics.is_empty() || metrics.contains(&row.name))
                && query.start_ms.is_none_or(|start| row.ts_ms >= start)
                && query.end_ms.is_none_or(|end| row.ts_ms <= end)
        })
        .collect()
}

/// Apply image history filters (same semantics as scalar filters).
pub fn apply_image_filters(rows: Vec<ImageHistoryPoint>, query: &ImageHistoryQuery) -> Vec<ImageHistoryPoint> {
    let run_ids = csv_set(query.run_id.as_deref());
    let metrics = csv_set(query.metric.as_deref());

    rows.into_iter()
        .filter(|row| {
            (run_ids.is_empty() || run_ids.contains(&row.run_id))
                && (metrics.is_empty() || metrics.contains(&row.name))
                && query.start_ms.is_none_or(|start| row.ts_ms >= start)
                && query.end_ms.is_none_or(|end| row.ts_ms <= end)
        })
        .collect()
}

/// Filter hyperparameters by run ID.
pub fn filter_hyperparameters(
    rows: Vec<(String, HyperparameterEntry)>,
    query: &HistoryQuery,
) -> Vec<(String, HyperparameterEntry)> {
    let run_ids = csv_set(query.run_id.as_deref());
    rows.into_iter()
        .filter(|(run_id, _)| run_ids.is_empty() || run_ids.contains(run_id))
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

/// Page a vector of items based on cursor and limit.
///
/// Cursor-based pagination returns items at positions [cursor, cursor + limit)
/// from the sorted dataset. Returns None for next_cursor when all items are returned.
pub fn page_vec<T>(rows: Vec<T>, cursor: Option<usize>, limit: Option<usize>) -> (Vec<T>, Option<usize>) {
    let start = cursor.unwrap_or(0);
    let limit = limit.unwrap_or(rows.len().max(1));
    let total = rows.len();

    let paged: Vec<T> = rows.into_iter().skip(start).take(limit).collect();
    let next_cursor = if start + paged.len() < total {
        Some(start + paged.len())
    } else {
        None
    };

    (paged, next_cursor)
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

/// GET /api/history handler
///
/// Queries persisted history using the StorageEngine trait for backend-agnostic behavior.
pub async fn history_handler(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Json<HistoryResponse> {
    let storage_query = build_history_query_options(&query);

    match state.storage.query_history(&storage_query).await {
        Ok(page) => {
            let run_summaries = if query.include_summaries.unwrap_or(true) {
                match state.storage.query_hyperparameters(&storage_query.run_ids).await {
                    Ok(hyperparameters) => build_run_summaries(&page.rows, hyperparameters),
                    Err(error) => {
                        let msg = error.to_string();
                        tracing::error!("Failed to query hyperparameters: {}", msg);
                        return Json(HistoryResponse { rows: vec![], run_summaries: vec![], next_cursor: None, error: Some(msg) });
                    }
                }
            } else {
                vec![]
            };

            Json(HistoryResponse { rows: page.rows, run_summaries, next_cursor: page.next_cursor, error: None })
        }
        Err(error) => {
            let msg = error.to_string();
            tracing::error!("Failed to query history: {}", msg);
            Json(HistoryResponse { rows: vec![], run_summaries: vec![], next_cursor: None, error: Some(msg) })
        }
    }
}

/// GET /api/runs/summary handler
pub async fn run_summaries_handler(
    State(state): State<AppState>,
    Query(query): Query<RunSummaryQuery>,
) -> Json<RunSummariesResponse> {
    let run_ids = csv_set(query.run_id.as_deref());
    let storage_query = HistoryQueryOptions {
        run_ids: run_ids.clone(),
        ..HistoryQueryOptions::default()
    };

    match state.storage.query_history(&storage_query).await {
        Ok(page) => match state.storage.query_hyperparameters(&run_ids).await {
            Ok(hyperparameters) => {
                let run_summaries = build_run_summaries(&page.rows, hyperparameters);
                Json(RunSummariesResponse { run_summaries, error: None })
            }
            Err(error) => {
                let msg = error.to_string();
                tracing::error!("Failed to query hyperparameters: {}", msg);
                Json(RunSummariesResponse { run_summaries: vec![], error: Some(msg) })
            }
        },
        Err(error) => {
            let msg = error.to_string();
            tracing::error!("Failed to query history: {}", msg);
            Json(RunSummariesResponse { run_summaries: vec![], error: Some(msg) })
        },
    }
}

/// GET /api/history/images handler
///
/// Queries image history using StorageEngine for backend-agnostic behavior.
pub async fn image_history_handler(
    State(state): State<AppState>,
    Query(query): Query<ImageHistoryQuery>,
) -> Json<ImageHistoryResponse> {
    let storage_query = build_image_query_options(&query);

    match state.storage.query_image_history(&storage_query).await {
        Ok(page) => {
            Json(ImageHistoryResponse { rows: page.rows, next_cursor: page.next_cursor, error: None })
        }
        Err(error) => {
            let msg = error.to_string();
            tracing::error!("Failed to query image history: {}", msg);
            Json(ImageHistoryResponse { rows: vec![], next_cursor: None, error: Some(msg) })
        }
    }
}

pub async fn image_frame_content_handler(
    State(state): State<AppState>,
    Query(query): Query<ImageFrameContentQuery>,
) -> Json<ImageFrameContentResponse> {
    match state.storage.load_image_frame(&ImageFrameQuery {
        run_id: query.run_id.clone(),
        name: query.name.clone(),
        ts_ms: query.ts_ms,
    }).await {
        Ok(Some(ImageFramePayload { format, image_base64 })) => Json(ImageFrameContentResponse {
            run_id: query.run_id,
            name: query.name,
            ts_ms: query.ts_ms,
            format,
            image_base64,
            error: None,
        }),
        Ok(None) => Json(ImageFrameContentResponse {
            run_id: query.run_id,
            name: query.name,
            ts_ms: query.ts_ms,
            format: String::new(),
            image_base64: String::new(),
            error: Some("image frame not found".to_string()),
        }),
        Err(error) => {
            let msg = error.to_string();
            tracing::error!("Failed to load image frame: {}", msg);
            Json(ImageFrameContentResponse {
                run_id: query.run_id,
                name: query.name,
                ts_ms: query.ts_ms,
                format: String::new(),
                image_base64: String::new(),
                error: Some(msg),
            })
        }
    }
}

/// GET /api/runs handler
///
/// Lists runs from the live t_junction and persisted storage.
pub async fn runs_handler(
    State(state): State<AppState>,
) -> Json<RunsResponse> {
    let mut runs: BTreeSet<String> = state.t_junction.current_runs().await.into_iter().collect();
    match state.storage.list_runs().await {
        Ok(stored_runs) => runs.extend(stored_runs),
        Err(error) => tracing::warn!("Failed to list persisted runs: {}", error),
    }

    Json(RunsResponse {
        runs: runs.into_iter().collect(),
    })
}

/// POST /api/runs/resolve handler
///
/// Resolves the canonical run ID for a given base run ID.
pub async fn resolve_run_id_handler(
    State(state): State<AppState>,
    Json(payload): Json<ResolveRunIdRequest>,
) -> Json<ResolveRunIdResponse> {
    let requested = normalize_run_id(&payload.base_run_id);
    let mut existing: BTreeSet<String> = state.t_junction.current_runs().await.into_iter().collect();
    match state.storage.list_runs().await {
        Ok(stored_runs) => existing.extend(stored_runs),
        Err(error) => tracing::warn!("Failed to list persisted runs while resolving run id: {}", error),
    }

    Json(ResolveRunIdResponse {
        run_id: resolve_run_id(&requested, &existing),
    })
}

/// DELETE /api/history handler
///
/// Clears persisted history and live run cache using StorageEngine and TJunction.
pub async fn delete_history_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<DeleteRunsResponse>) {
    // Clear persisted storage
    if let Err(error) = state.storage.clear_history().await {
        let msg = error.to_string();
        tracing::error!("Failed to delete persisted history: {}", msg);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(DeleteRunsResponse { deleted: false, error: Some(msg) })
        );
    }

    // Clear live run IDs from T-Junction so sidebar re-fetches fresh run list
    state.t_junction.clear_live_runs().await;

    (
        StatusCode::OK,
        Json(DeleteRunsResponse { deleted: true, error: None })
    )
}
