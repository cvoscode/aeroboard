//! Objective-aware scores and exact-position aggregation over persisted data.
use crate::{
    app_state::AppState,
    catalog::{self, CatalogError, Entity, Result},
    history::HistoryPoint,
    storage::HistoryQueryOptions,
};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct AnalysisCache {
    pub revision: AtomicU64,
    entries: Mutex<HashMap<String, Value>>,
    /// Serializes cold objective rebuilds with persistence, so counters never
    /// double-count the batch at a snapshot boundary.
    pub gate: Mutex<()>,
    scores: Mutex<HashMap<String, CachedScores>>,
    seen: Mutex<(BTreeSet<String>, std::collections::VecDeque<String>)>,
}
struct CachedScores {
    signature: String,
    definitions: BTreeMap<String, Value>,
    metrics: BTreeMap<String, ObjectiveStats>,
}
struct ObjectiveStats {
    score: Score,
    first_ms: i64,
}
impl ObjectiveStats {
    fn new(point: HistoryPoint, definition: Value) -> Self {
        Self {
            first_ms: point.ts_ms,
            score: calculate(
                &point.run_id.clone(),
                &point.name.clone(),
                vec![point],
                &definition,
                None,
            ),
        }
    }
    fn record(&mut self, point: HistoryPoint) {
        self.first_ms = self.first_ms.min(point.ts_ms);
        let score = &mut self.score;
        let position = (point.ts_ms, point.step.unwrap_or(f64::NEG_INFINITY));
        if position
            >= (
                score.last_at_ms.unwrap_or(i64::MIN),
                score.last_step.unwrap_or(f64::NEG_INFINITY),
            )
        {
            score.last_at_ms = Some(point.ts_ms);
            score.last_step = point.step;
            score.last_is_finite = point.value.is_finite();
            score.last = point.value.is_finite().then_some(point.value);
        }
        if point.value.is_finite() {
            score.finite_count += 1;
            score.min = Some(score.min.map_or(point.value, |v| v.min(point.value)));
            score.max = Some(score.max.map_or(point.value, |v| v.max(point.value)));
            let better = score.best.is_none_or(|v| match score.goal.as_deref() {
                Some("minimize") => point.value < v,
                Some("maximize") => point.value > v,
                _ => false,
            });
            let earlier = score.best == Some(point.value)
                && position
                    < (
                        score.best_at_ms.unwrap_or(i64::MAX),
                        score.best_step.unwrap_or(f64::INFINITY),
                    );
            if score.goal.is_some() && (better || earlier) {
                score.best = Some(point.value);
                score.best_step = point.step;
                score.best_at_ms = Some(point.ts_ms);
            }
            let crossing = score
                .target
                .is_some_and(|target| match score.goal.as_deref() {
                    Some("minimize") => point.value <= target,
                    Some("maximize") => point.value >= target,
                    _ => false,
                });
            if crossing && score.target_at_ms.is_none_or(|t| point.ts_ms < t) {
                score.target_at_ms = Some(point.ts_ms);
            }
        }
        score.time_to_target_ms = score.target_at_ms.map(|t| t - self.first_ms);
    }
}
impl AnalysisCache {
    pub fn invalidate(&self) {
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
    pub async fn clear(&self) {
        self.scores.lock().await.clear();
        self.entries.lock().await.clear();
        self.invalidate();
    }
    pub async fn record(&self, telemetry: &crate::telemetry::TelemetryBatch) {
        if telemetry.kind != crate::telemetry::TelemetryKind::Scalars {
            return;
        }
        if let Some(envelope) = &telemetry.envelope {
            let mut seen = self.seen.lock().await;
            if !seen.0.insert(envelope.batch_id.clone()) {
                return;
            }
            seen.1.push_back(envelope.batch_id.clone());
            if seen.1.len() > 8192
                && let Some(id) = seen.1.pop_front()
            {
                seen.0.remove(&id);
            }
        }
        let mut cache = self.scores.lock().await;
        for point in crate::storage::codec::parse_scalar_batch(&telemetry.batch) {
            if let Some(run) = cache.get_mut(&point.run_id) {
                if let Some(stats) = run.metrics.get_mut(&point.name) {
                    stats.record(point);
                } else {
                    let definition = run
                        .definitions
                        .get(&point.name)
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    run.metrics
                        .insert(point.name.clone(), ObjectiveStats::new(point, definition));
                }
            }
        }
    }
    async fn get(&self, key: &str) -> Option<Value> {
        self.entries.lock().await.get(key).cloned()
    }
    async fn put(&self, key: String, value: Value) {
        let mut entries = self.entries.lock().await;
        if entries.len() >= 64 {
            entries.clear();
        }
        entries.insert(key, value);
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/scorecards", get(scorecards))
        .route("/comparison/aggregate", post(aggregate))
        .route("/export/scalars.csv", get(export_csv))
}

#[derive(Deserialize)]
struct ExportQuery {
    run_id: String,
    metric: Option<String>,
}
async fn export_csv(
    State(state): State<AppState>,
    Query(query): Query<ExportQuery>,
) -> Result<axum::response::Response> {
    use axum::response::IntoResponse;
    let run_ids: BTreeSet<String> = query
        .run_id
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if run_ids.is_empty() || run_ids.len() > 50 {
        return Err(CatalogError::invalid("select 1–50 runs"));
    }
    let metrics = query
        .metric
        .map(|s| BTreeSet::from([s]))
        .unwrap_or_default();
    let options = HistoryQueryOptions {
        run_ids,
        metrics,
        limit: Some(5000),
        ..Default::default()
    };
    let first = state
        .storage
        .query_history(&options)
        .await
        .map_err(|e| CatalogError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let stream = futures::stream::try_unfold(
        (state.storage, options, Some(first), true),
        |(storage, mut options, page, header)| async move {
            let page = match page {
                Some(page) => page,
                None => return Ok::<_, std::io::Error>(None),
            };
            let mut csv = if header {
                "run_id,metric,timestamp_ms,step,value,segment\r\n".to_owned()
            } else {
                String::new()
            };
            for point in page.rows {
                let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
                csv.push_str(&format!(
                    "{},{},{},{},{},{}\r\n",
                    quote(&point.run_id),
                    quote(&point.name),
                    point.ts_ms,
                    point.step.map(|n| n.to_string()).unwrap_or_default(),
                    point.value,
                    quote(point.segment.as_deref().unwrap_or(""))
                ));
            }
            let next = if let Some(cursor) = page.next_cursor {
                options.cursor = Some(cursor);
                Some(
                    storage
                        .query_history(&options)
                        .await
                        .map_err(|e| std::io::Error::other(e.to_string()))?,
                )
            } else {
                None
            };
            Ok(Some((csv, (storage, options, next, false))))
        },
    );
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=metrics-raw.csv",
            ),
        ],
        axum::body::Body::from_stream(stream),
    )
        .into_response())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Score {
    pub run_id: String,
    pub name: String,
    pub goal: Option<String>,
    pub unit: Option<String>,
    pub target: Option<f64>,
    pub last: Option<f64>,
    pub last_step: Option<f64>,
    pub last_at_ms: Option<i64>,
    pub last_is_finite: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub best: Option<f64>,
    pub best_step: Option<f64>,
    pub best_at_ms: Option<i64>,
    pub finite_count: usize,
    pub target_at_ms: Option<i64>,
    pub time_to_target_ms: Option<i64>,
    pub at_step: Option<f64>,
}

pub fn calculate(
    run_id: &str,
    name: &str,
    mut points: Vec<HistoryPoint>,
    definition: &Value,
    step: Option<f64>,
) -> Score {
    points.sort_by(|a, b| {
        a.ts_ms.cmp(&b.ts_ms).then_with(|| {
            a.step
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&b.step.unwrap_or(f64::NEG_INFINITY))
        })
    });
    let goal = definition["goal"].as_str().map(str::to_owned);
    let target = definition["target"].as_f64().filter(|v| v.is_finite());
    let valid: Vec<_> = points.iter().filter(|p| p.value.is_finite()).collect();
    let last = points.last();
    let best = if let Some(direction) = goal.as_deref() {
        valid.iter().copied().min_by(|a, b| {
            let order = if a.value == b.value {
                std::cmp::Ordering::Equal
            } else if direction == "minimize" {
                a.value.total_cmp(&b.value)
            } else {
                b.value.total_cmp(&a.value)
            };
            order.then(a.ts_ms.cmp(&b.ts_ms)).then_with(|| {
                a.step
                    .unwrap_or(f64::INFINITY)
                    .total_cmp(&b.step.unwrap_or(f64::INFINITY))
            })
        })
    } else {
        None
    };
    let crossing = target.and_then(|target| {
        valid.iter().copied().find(|p| match goal.as_deref() {
            Some("minimize") => p.value <= target,
            Some("maximize") => p.value >= target,
            _ => false,
        })
    });
    let at_step = step.and_then(|requested| {
        let min = points
            .iter()
            .filter_map(|p| p.step)
            .min_by(f64::total_cmp)?;
        let max = points
            .iter()
            .filter_map(|p| p.step)
            .max_by(f64::total_cmp)?;
        if requested < min || requested > max {
            return None;
        }
        valid
            .iter()
            .copied()
            .filter(|p| p.step.is_some_and(|s| s <= requested))
            .max_by(|a, b| {
                a.step
                    .unwrap_or(0.0)
                    .total_cmp(&b.step.unwrap_or(0.0))
                    .then(a.ts_ms.cmp(&b.ts_ms))
            })
            .map(|p| p.value)
    });
    Score {
        run_id: run_id.into(),
        name: name.into(),
        goal,
        unit: definition["unit"].as_str().map(str::to_owned),
        target,
        last: last.and_then(|p| p.value.is_finite().then_some(p.value)),
        last_step: last.and_then(|p| p.step),
        last_at_ms: last.map(|p| p.ts_ms),
        last_is_finite: last.is_some_and(|p| p.value.is_finite()),
        min: valid.iter().map(|p| p.value).min_by(f64::total_cmp),
        max: valid.iter().map(|p| p.value).max_by(f64::total_cmp),
        best: best.map(|p| p.value),
        best_step: best.and_then(|p| p.step),
        best_at_ms: best.map(|p| p.ts_ms),
        finite_count: valid.len(),
        target_at_ms: crossing.map(|p| p.ts_ms),
        time_to_target_ms: crossing.and_then(|p| points.first().map(|first| p.ts_ms - first.ts_ms)),
        at_step,
    }
}

pub fn definition_for(run: &Entity, name: &str, definitions: &[Entity]) -> Value {
    let mut result = json!({});
    for parent in [&run.parent, &run.id] {
        if let Some(definition) = definitions
            .iter()
            .find(|d| &d.parent == parent && d.data["name"] == name)
        {
            result = definition.data.clone();
        }
    }
    result
}

async fn metadata(state: &AppState) -> Result<(Vec<Entity>, Vec<Entity>)> {
    state
        .catalog
        .import_runs(crate::history::known_runs(state).await)
        .await?;
    state
        .catalog
        .read(|db| {
            Ok((
                catalog::list(db, "run", None)?,
                catalog::list(db, "metric", None)?,
            ))
        })
        .await
}

async fn history(
    state: &AppState,
    run_ids: BTreeSet<String>,
    metrics: BTreeSet<String>,
) -> Result<Vec<HistoryPoint>> {
    let page = state
        .storage
        .query_history(&HistoryQueryOptions {
            run_ids,
            metrics,
            limit: Some(2_000_001),
            ..Default::default()
        })
        .await
        .map_err(|e| CatalogError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if page.rows.len() > 2_000_000 {
        return Err(CatalogError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "comparison exceeds 2 million observations; select fewer runs or metrics".into(),
        ));
    }
    Ok(page.rows)
}

#[derive(Deserialize)]
struct ScoreQuery {
    run_id: String,
    metric: Option<String>,
    step: Option<f64>,
    segment: Option<String>,
}
async fn scorecards(
    State(state): State<AppState>,
    Query(query): Query<ScoreQuery>,
) -> Result<Json<Value>> {
    if query.step.is_some_and(|v| !v.is_finite()) {
        return Err(CatalogError::invalid("step must be finite"));
    }
    let requested: BTreeSet<String> = query
        .run_id
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if requested.is_empty() || requested.len() > 50 {
        return Err(CatalogError::invalid("select 1–50 runs"));
    }
    let (runs, definitions) = metadata(&state).await?;
    let selected: Vec<_> = runs
        .iter()
        .filter(|r| {
            (requested.contains(&r.id)
                || requested.contains(r.data["run_id"].as_str().unwrap_or("")))
                && r.data["deleted"] != true
        })
        .collect();
    let revision = state.analysis.revision.load(Ordering::Relaxed);
    let key = format!(
        "scores:{requested:?}:{:?}:{:?}:{revision}:{:?}:{:?}",
        query.metric,
        query.step,
        definitions
            .iter()
            .map(|d| (&d.id, d.revision))
            .collect::<Vec<_>>(),
        query.segment
    );
    if let Some(value) = state.analysis.get(&key).await {
        return Ok(Json(value));
    }
    let ids = selected
        .iter()
        .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned))
        .collect();
    if query.step.is_none() && query.segment.is_none() {
        let _gate = state.analysis.gate.lock().await;
        let mut missing = BTreeSet::new();
        let signatures: BTreeMap<String, String> = selected
            .iter()
            .map(|run| {
                (
                    run.data["run_id"].as_str().unwrap_or("").to_owned(),
                    format!(
                        "{:?}",
                        definitions
                            .iter()
                            .filter(|d| d.parent == run.parent || d.parent == run.id)
                            .map(|d| (&d.id, d.revision))
                            .collect::<Vec<_>>()
                    ),
                )
            })
            .collect();
        {
            let cache = state.analysis.scores.lock().await;
            for (id, signature) in &signatures {
                if cache.get(id).is_none_or(|c| &c.signature != signature) {
                    missing.insert(id.clone());
                }
            }
        }
        if !missing.is_empty() {
            let points = history(&state, missing.clone(), BTreeSet::new()).await?;
            let mut cache = state.analysis.scores.lock().await;
            if cache.len() + missing.len() > 128 {
                cache.retain(|id, _| signatures.contains_key(id));
            }
            for id in &missing {
                let run = selected
                    .iter()
                    .find(|r| r.data["run_id"] == *id)
                    .ok_or_else(CatalogError::missing)?;
                let mut defs = BTreeMap::new();
                for d in definitions
                    .iter()
                    .filter(|d| d.parent == run.parent || d.parent == run.id)
                {
                    let name = d.data["name"].as_str().unwrap_or("");
                    defs.insert(name.to_owned(), definition_for(run, name, &definitions));
                }
                cache.insert(
                    id.clone(),
                    CachedScores {
                        signature: signatures[id].clone(),
                        definitions: defs,
                        metrics: BTreeMap::new(),
                    },
                );
            }
            for point in points {
                let run = cache
                    .get_mut(&point.run_id)
                    .ok_or_else(CatalogError::missing)?;
                if let Some(stats) = run.metrics.get_mut(&point.name) {
                    stats.record(point);
                } else {
                    let definition = run
                        .definitions
                        .get(&point.name)
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    run.metrics
                        .insert(point.name.clone(), ObjectiveStats::new(point, definition));
                }
            }
        }
        let cache = state.analysis.scores.lock().await;
        let scores: Vec<_> = selected
            .iter()
            .filter_map(|r| cache.get(r.data["run_id"].as_str().unwrap_or("")))
            .flat_map(|r| r.metrics.values())
            .filter(|s| {
                query
                    .metric
                    .as_ref()
                    .is_none_or(|name| name == &s.score.name)
            })
            .map(|s| s.score.clone())
            .collect();
        return Ok(Json(json!({"rows":scores,"data_revision":revision})));
    }
    let metrics = query
        .metric
        .map(|s| BTreeSet::from([s]))
        .unwrap_or_default();
    let points = history(&state, ids, metrics).await?;
    let mut grouped: BTreeMap<(String, String), Vec<HistoryPoint>> = BTreeMap::new();
    for point in points.into_iter().filter(|p| {
        query.segment.as_ref().is_none_or(|s| {
            if s == "__global__" {
                p.segment.is_none()
            } else {
                p.segment.as_ref() == Some(s)
            }
        })
    }) {
        grouped
            .entry((point.run_id.clone(), point.name.clone()))
            .or_default()
            .push(point);
    }
    let mut scores = Vec::new();
    for ((id, name), points) in grouped {
        if let Some(run) = selected.iter().find(|r| r.data["run_id"] == id) {
            let ambiguous = query.step.is_some()
                && query.segment.is_none()
                && points
                    .iter()
                    .map(|p| p.segment.as_deref())
                    .collect::<BTreeSet<_>>()
                    .len()
                    > 1;
            let mut score = calculate(
                &id,
                &name,
                points,
                &definition_for(run, &name, &definitions),
                query.step,
            );
            if ambiguous {
                score.at_step = None;
            }
            scores.push(score);
        }
    }
    let value = json!({"rows":scores,"data_revision":revision});
    state.analysis.put(key, value.clone()).await;
    Ok(Json(value))
}

#[derive(Deserialize, Serialize)]
struct AggregateQuery {
    run_ids: Vec<String>,
    metric: String,
    #[serde(default)]
    group_by: Vec<String>,
    #[serde(default = "step_alignment")]
    alignment: String,
    #[serde(default)]
    include_unfinished: bool,
    #[serde(default = "max_points")]
    max_points: usize,
    segment: Option<String>,
}
fn step_alignment() -> String {
    "step".into()
}
fn max_points() -> usize {
    2000
}
async fn aggregate(
    State(state): State<AppState>,
    Json(query): Json<AggregateQuery>,
) -> Result<Json<Value>> {
    if query.run_ids.is_empty()
        || query.run_ids.len() > 50
        || query.metric.is_empty()
        || query.group_by.len() > 8
        || !(["step", "wall", "rel"].contains(&query.alignment.as_str()))
    {
        return Err(CatalogError::invalid(
            "select 1–50 runs, a metric, up to 8 grouping fields, and step/wall/rel alignment",
        ));
    }
    if query.group_by.iter().any(|g| {
        !(g.starts_with("params.")
            || g.starts_with("provenance.")
            || g == "project"
            || g == "experiment")
    }) {
        return Err(CatalogError::invalid(
            "grouping fields must be project, experiment, params.KEY or provenance.KEY",
        ));
    }
    let (runs, definitions) = metadata(&state).await?;
    let selected: Vec<_> = runs
        .iter()
        .filter(|r| {
            query
                .run_ids
                .iter()
                .any(|id| id == &r.id || r.data["run_id"] == *id)
                && r.data["deleted"] != true
        })
        .filter(|r| query.include_unfinished || r.data["status"] == "completed")
        .collect();
    if selected.is_empty() {
        return Ok(Json(
            json!({"groups":[],"excluded_runs":query.run_ids.len(),"alignment":query.alignment}),
        ));
    }
    let ids: BTreeSet<_> = selected
        .iter()
        .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned))
        .collect();
    let summaries = state
        .summaries
        .summaries(state.storage.as_ref(), &ids)
        .await
        .map_err(|e| CatalogError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let revision = state.analysis.revision.load(Ordering::Relaxed);
    let key = format!(
        "aggregate:{}:{revision}:{definitions:?}:{selected:?}",
        serde_json::to_string(&query).unwrap_or_default()
    );
    if let Some(value) = state.analysis.get(&key).await {
        return Ok(Json(value));
    }
    let points: Vec<_> = history(&state, ids, BTreeSet::from([query.metric.clone()]))
        .await?
        .into_iter()
        .filter(|p| {
            query
                .segment
                .as_ref()
                .is_none_or(|s| p.segment.as_ref() == Some(s))
        })
        .collect();
    let mut grouped: BTreeMap<String, Vec<(&Entity, Option<String>)>> = BTreeMap::new();
    for run in &selected {
        let summary = summaries.iter().find(|s| run.data["run_id"] == s.run_id);
        let fields = if query.group_by.is_empty() {
            let mut fields = vec!["project".to_string(), "experiment".to_string()];
            if let Some(summary) = summary {
                fields.extend(
                    summary
                        .hyperparameters
                        .iter()
                        .filter(|h| {
                            !matches!(
                                h.key.as_str(),
                                "seed" | "random_seed" | "run_id" | "__aeroboard_dashboard__"
                            )
                        })
                        .map(|h| format!("params.{}", h.key)),
                );
            }
            fields
        } else {
            query.group_by.clone()
        };
        let mut parts = vec![];
        for field in fields {
            let value = if let Some(name) = field.strip_prefix("params.") {
                summary
                    .and_then(|s| s.hyperparameters.iter().find(|h| h.key == name))
                    .map(|h| h.value.clone())
                    .unwrap_or_else(|| "missing".into())
            } else if let Some(name) = field.strip_prefix("provenance.") {
                run.data["provenance"][name].to_string()
            } else if field == "project" {
                run.parent.clone()
            } else {
                run.data["provenance"]["experiment"]
                    .as_str()
                    .unwrap_or("ungrouped")
                    .to_owned()
            };
            parts.push(format!("{field}={value}"));
        }
        // Dataset and objective compatibility are always part of the grouping key.
        let definition = definition_for(run, &query.metric, &definitions);
        parts.push(format!("dataset={}", run.data["provenance"]["dataset"]));
        parts.push(format!("definition={definition}"));
        let segments: BTreeSet<_> = points
            .iter()
            .filter(|p| run.data["run_id"] == p.run_id)
            .map(|p| p.segment.clone())
            .collect();
        for segment in segments {
            let mut label = parts.join(" · ");
            label.push_str(&format!(
                " · segment={}",
                segment.as_deref().unwrap_or("global")
            ));
            grouped.entry(label).or_default().push((run, segment));
        }
    }
    let mut groups = vec![];
    for (label, members) in grouped {
        let mut positions: BTreeMap<u64, Vec<f64>> = BTreeMap::new();
        for (run, segment) in &members {
            let mut rows: Vec<_> = points
                .iter()
                .filter(|p| {
                    run.data["run_id"] == p.run_id && p.value.is_finite() && p.segment == *segment
                })
                .collect();
            rows.sort_by_key(|p| p.ts_ms);
            let first = rows.first().map_or(0, |p| p.ts_ms);
            let mut per_run: BTreeMap<u64, f64> = BTreeMap::new();
            for point in rows {
                let x = match query.alignment.as_str() {
                    "step" => point.step,
                    "wall" => Some(point.ts_ms as f64 / 1000.0),
                    _ => Some((point.ts_ms - first) as f64 / 1000.0),
                };
                if let Some(x) = x.filter(|x| x.is_finite()) {
                    per_run.insert(
                        if x == 0.0 {
                            0.0_f64.to_bits()
                        } else {
                            x.to_bits()
                        },
                        point.value,
                    );
                }
            }
            for (x, value) in per_run {
                positions.entry(x).or_default().push(value);
            }
        }
        let mut rows: Vec<Value> = positions
            .into_iter()
            .map(|(x, values)| {
                let n = values.len();
                let scale=values.iter().map(|v|v.abs()).fold(0.0_f64,f64::max);
                let normalized=if scale==0.0{0.0}else{(values.iter().map(|v|v/scale).sum::<f64>()/n as f64).clamp(-1.0,1.0)};
                let mean=normalized*scale;
                let deviation=(n>1).then(||if scale==0.0{0.0}else{(values.iter().map(|v|(v/scale-normalized).powi(2)).sum::<f64>()/(n-1)as f64).sqrt()*scale});
                let stddev=deviation.filter(|v|v.is_finite());
                json!({"x":f64::from_bits(x),"mean":mean,"stddev":stddev,"n":n,"variance_unavailable":deviation.is_some()&&stddev.is_none()})
            })
            .collect();
        rows.sort_by(|a, b| {
            a["x"]
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&b["x"].as_f64().unwrap_or(0.0))
        });
        let max = query.max_points.clamp(2, 5000);
        if rows.len() > max {
            let last = rows.len() - 1;
            rows = (0..max)
                .map(|i| rows[i * last / (max - 1)].clone())
                .collect();
        }
        groups.push(json!({"label":label,"runs":members.iter().map(|(r,_)|&r.id).collect::<Vec<_>>(),"points":rows}));
    }
    let value = json!({"groups":groups,"excluded_runs":query.run_ids.len()-selected.len(),"alignment":query.alignment,"band":"sample standard deviation","data_revision":revision});
    state.analysis.put(key, value.clone()).await;
    Ok(Json(value))
}
