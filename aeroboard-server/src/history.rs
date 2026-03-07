use axum::{extract::{Query, State}, http::StatusCode, Json};
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeSet;

use crate::app_state::AppState;

/// A single scalar data point returned from persisted history
#[derive(Serialize)]
pub struct HistoryPoint {
    pub ts_ms: i64,
    pub run_id: String,
    pub name: String,
    pub value: f64,
}

/// Response from /api/history
#[derive(Serialize)]
pub struct HistoryResponse {
    pub rows: Vec<HistoryPoint>,
    pub run_summaries: Vec<RunSummary>,
    pub next_cursor: Option<usize>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct RunSummary {
    pub run_id: String,
    pub hyperparameters: Vec<HyperparameterEntry>,
    pub metric_summaries: Vec<MetricSummary>,
}

#[derive(Serialize, Clone)]
pub struct HyperparameterEntry {
    pub key: String,
    pub value: String,
    pub value_type: String,
}

#[derive(Serialize, Clone)]
pub struct MetricSummary {
    pub name: String,
    pub last: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Serialize)]
pub struct ImageHistoryPoint {
    pub ts_ms: i64,
    pub run_id: String,
    pub name: String,
    pub format: String,
    pub image_base64: String,
}

#[derive(Serialize)]
pub struct ImageHistoryResponse {
    pub rows: Vec<ImageHistoryPoint>,
    pub next_cursor: Option<usize>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct DeleteRunsResponse {
    pub deleted: bool,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct RunsResponse {
    pub runs: Vec<String>,
}

#[derive(Serialize)]
pub struct ResolveRunIdResponse {
    pub run_id: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct HistoryQuery {
    pub run_id: Option<String>,
    pub metric: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveRunIdRequest {
    pub base_run_id: String,
}

/// GET /api/history
pub async fn history_handler(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Json<HistoryResponse> {
    match state.storage.query_history().await {
        Ok(rows) => match state.storage.query_hyperparameters().await {
            Ok(hyperparameters) => {
                let filtered_rows = apply_history_filters(rows, &query);
                let filtered_hyperparameters = filter_hyperparameters(hyperparameters, &query);
                let run_summaries = build_run_summaries(&filtered_rows, filtered_hyperparameters.clone());
                let (page_rows, next_cursor) = page_vec(filtered_rows, query.cursor, query.limit);
                Json(HistoryResponse { rows: page_rows, run_summaries, next_cursor, error: None })
            }
            Err(error) => {
                let msg = error.to_string();
                tracing::error!("Failed to query hyperparameters: {}", msg);
                Json(HistoryResponse { rows: vec![], run_summaries: vec![], next_cursor: None, error: Some(msg) })
            }
        },
        Err(error) => {
            let msg = error.to_string();
            tracing::error!("Failed to query history: {}", msg);
            Json(HistoryResponse { rows: vec![], run_summaries: vec![], next_cursor: None, error: Some(msg) })
        }
    }
}

/// GET /api/history/images
pub async fn image_history_handler(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Json<ImageHistoryResponse> {
    match state.storage.query_image_history().await {
        Ok(rows) => {
            let filtered_rows = apply_image_filters(rows, &query);
            let (page_rows, next_cursor) = page_vec(filtered_rows, query.cursor, query.limit);
            Json(ImageHistoryResponse { rows: page_rows, next_cursor, error: None })
        }
        Err(error) => {
            let msg = error.to_string();
            tracing::error!("Failed to query image history: {}", msg);
            Json(ImageHistoryResponse { rows: vec![], next_cursor: None, error: Some(msg) })
        }
    }
}

/// GET /api/runs
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

/// POST /api/runs/resolve
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

/// DELETE /api/history
pub async fn delete_history_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<DeleteRunsResponse>) {
    match state.storage.clear_history().await {
        Ok(()) => (
            StatusCode::OK,
            Json(DeleteRunsResponse { deleted: true, error: None })
        ),
        Err(error) => {
            let msg = error.to_string();
            tracing::error!("Failed to delete history: {}", msg);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DeleteRunsResponse { deleted: false, error: Some(msg) })
            )
        }
    }
}

pub(crate) fn build_run_summaries(points: &[HistoryPoint], hyperparameters: Vec<(String, HyperparameterEntry)>) -> Vec<RunSummary> {
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

fn normalize_run_id(run_id: &str) -> String {
    let cleaned = run_id.trim();
    if cleaned.is_empty() {
        format!("run-{}", chrono_like_unix_seconds())
    } else {
        cleaned.to_string()
    }
}

pub(crate) fn resolve_run_id(base: &str, existing: &BTreeSet<String>) -> String {
    let (base_without_suffix, framework_suffix) = split_framework_suffix(base);
    let existing_versions: BTreeSet<usize> = existing
        .iter()
        .filter_map(|candidate| extract_version(candidate, base))
        .collect();

    if !existing_versions.contains(&1) {
        return base.to_string();
    }

    let next_version = existing_versions.iter().copied().max().unwrap_or(1) + 1;
    format!("{base_without_suffix}-v{next_version}{framework_suffix}")
}

fn split_framework_suffix(candidate: &str) -> (String, String) {
    match candidate.rsplit_once("-pl") {
        Some((prefix, suffix)) if !suffix.is_empty() => (prefix.to_string(), format!("-pl{}", suffix)),
        _ => (candidate.to_string(), String::new()),
    }
}

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

fn apply_history_filters(rows: Vec<HistoryPoint>, query: &HistoryQuery) -> Vec<HistoryPoint> {
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

fn apply_image_filters(rows: Vec<ImageHistoryPoint>, query: &HistoryQuery) -> Vec<ImageHistoryPoint> {
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

fn filter_hyperparameters(
    rows: Vec<(String, HyperparameterEntry)>,
    query: &HistoryQuery,
) -> Vec<(String, HyperparameterEntry)> {
    let run_ids = csv_set(query.run_id.as_deref());
    rows.into_iter()
        .filter(|(run_id, _)| run_ids.is_empty() || run_ids.contains(run_id))
        .collect()
}

fn page_vec<T>(rows: Vec<T>, cursor: Option<usize>, limit: Option<usize>) -> (Vec<T>, Option<usize>) {
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

fn csv_set(value: Option<&str>) -> BTreeSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn chrono_like_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
