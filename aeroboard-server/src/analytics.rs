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
    /// Serializes deletion with accepted writes; queries never hold this gate.
    pub gate: Mutex<()>,
}
#[derive(Serialize, Deserialize)]
struct CachedScores {
    signature: String,
    definitions: BTreeMap<String, Value>,
    metrics: BTreeMap<String, ObjectiveStats>,
}
#[derive(Serialize, Deserialize)]
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
    pub async fn remove(&self, run_id: &str) {
        let quoted = serde_json::to_string(run_id).unwrap_or_default();
        self.entries
            .lock()
            .await
            .retain(|key, _| !key.contains(&quoted));
        self.invalidate();
    }
    pub async fn clear(&self) {
        self.entries.lock().await.clear();
        self.invalidate();
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
        ..Default::default()
    };
    let batches = state
        .storage
        .query_history_batches(&options)
        .await
        .map_err(CatalogError::from)?
        .rows;
    let stream = futures::stream::iter(
        batches
            .into_iter()
            .flat_map(|batch| {
                (0..batch.num_rows())
                    .step_by(5000)
                    .map(|offset| batch.slice(offset, 5000.min(batch.num_rows() - offset)))
                    .collect::<Vec<_>>()
            })
            .enumerate()
            .map(|(index, batch)| {
                use arrow::array::{Array, AsArray};
                let mut csv = if index == 0 {
                    "run_id,metric,timestamp_ms,step,value,segment\r\n".to_owned()
                } else {
                    String::new()
                };
                let columns: [arrow::array::ArrayRef; 2] = [
                    arrow::compute::cast(batch.column(1), &arrow::datatypes::DataType::Utf8)
                        .map_err(|e| std::io::Error::other(e.to_string()))?,
                    arrow::compute::cast(batch.column(2), &arrow::datatypes::DataType::Utf8)
                        .map_err(|e| std::io::Error::other(e.to_string()))?,
                ];
                let runs = columns[0].as_string::<i32>();
                let names = columns[1].as_string::<i32>();
                let times = batch
                    .column(0)
                    .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
                let values = batch
                    .column(3)
                    .as_primitive::<arrow::datatypes::Float64Type>();
                let steps = batch
                    .column(4)
                    .as_primitive::<arrow::datatypes::Float64Type>();
                let segments = batch.column(5).as_string::<i32>();
                let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
                for i in 0..batch.num_rows() {
                    csv.push_str(&format!(
                        "{},{},{},{},{},{}\r\n",
                        quote(runs.value(i)),
                        quote(names.value(i)),
                        times.value(i),
                        if steps.is_null(i) {
                            String::new()
                        } else {
                            steps.value(i).to_string()
                        },
                        values.value(i),
                        quote(if segments.is_null(i) {
                            ""
                        } else {
                            segments.value(i)
                        })
                    ));
                }
                Ok::<_, std::io::Error>(csv)
            }),
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
        .map_err(CatalogError::from)?;
    if page.rows.len() > 2_000_000 {
        return Err(CatalogError::http(
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
    let revisions = data_revisions(&state, &selected).await?;
    let definition_revisions = definitions
        .iter()
        .filter(|d| {
            selected
                .iter()
                .any(|r| d.parent == r.id || d.parent == r.parent)
        })
        .map(|d| (&d.id, d.revision))
        .collect::<Vec<_>>();
    let key = format!(
        "scores:{}",
        json!([
            revisions,
            definition_revisions,
            query.metric,
            query.step,
            query.segment
        ])
    );
    if let Some(value) = state.analysis.get(&key).await {
        return Ok(Json(value));
    }
    let ids = selected
        .iter()
        .filter_map(|r| r.data["run_id"].as_str().map(str::to_owned))
        .collect();
    if query.step.is_none() && query.segment.is_none() {
        let mut scores = Vec::new();
        for run in &selected {
            let run_id = run.data["run_id"]
                .as_str()
                .ok_or_else(|| CatalogError::invalid("missing run ID"))?
                .to_owned();
            let signature = definition_signature(run, &definitions);
            let lookup = run_id.clone();
            let (stored, data_revision) = state
                .catalog
                .read(move |db| {
                    use rusqlite::OptionalExtension;
                    Ok((
                        db.query_row(
                            "SELECT body FROM run_scores WHERE run_id=?1",
                            [&lookup],
                            |r| r.get::<_, Vec<u8>>(0),
                        )
                        .optional()?,
                        db.query_row(
                            "SELECT revision FROM data_revisions WHERE run_id=?1",
                            [&lookup],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()?
                        .unwrap_or(0),
                    ))
                })
                .await?;
            let stored: Option<CachedScores> =
                stored.and_then(|bytes| serde_json::from_slice(&bytes).ok());
            let snapshot = if let Some(stored) = stored.filter(|s| s.signature == signature) {
                stored
            } else {
                let points =
                    history(&state, BTreeSet::from([run_id.clone()]), BTreeSet::new()).await?;
                let mut snapshot = empty_scores(run, &definitions);
                for point in points {
                    record_score(&mut snapshot, point);
                }
                let body = serde_json::to_vec(&snapshot)
                    .map_err(|e| CatalogError::invalid(e.to_string()))?;
                let id = run_id.clone();
                state.catalog.quiet(move |db| {
                    use rusqlite::OptionalExtension;
                    let current = db.query_row("SELECT revision FROM data_revisions WHERE run_id=?1",[&id],|r|r.get::<_,i64>(0)).optional()?.unwrap_or(0);
                    // A concurrent write makes this response a snapshot; don't replace its newer counters.
                    if current==data_revision {db.execute("INSERT INTO run_scores VALUES(?1,?2) ON CONFLICT(run_id) DO UPDATE SET body=excluded.body",rusqlite::params![id,body])?;}
                    Ok(())
                }).await?;
                snapshot
            };
            scores.extend(
                snapshot
                    .metrics
                    .into_values()
                    .filter(|s| {
                        query
                            .metric
                            .as_ref()
                            .is_none_or(|name| name == &s.score.name)
                    })
                    .map(|s| s.score),
            );
        }
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
        .map_err(CatalogError::from)?;
    let revision = state.analysis.revision.load(Ordering::Relaxed);
    let revisions = data_revisions(&state, &selected).await?;
    let metadata = selected
        .iter()
        .map(|r| {
            (
                &r.id,
                &r.parent,
                &r.data["metadata_revision"],
                &r.data["status"],
            )
        })
        .collect::<Vec<_>>();
    let definition_revisions = definitions
        .iter()
        .map(|d| (&d.id, d.revision))
        .collect::<Vec<_>>();
    let key = format!(
        "aggregate:{}",
        json!([query, revisions, metadata, definition_revisions])
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

fn definition_signature(run: &Entity, definitions: &[Entity]) -> String {
    serde_json::to_string(
        &definitions
            .iter()
            .filter(|d| d.parent == run.parent || d.parent == run.id)
            .map(|d| (&d.id, d.revision))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default()
}
fn empty_scores(run: &Entity, definitions: &[Entity]) -> CachedScores {
    let mut defs = BTreeMap::new();
    for d in definitions
        .iter()
        .filter(|d| d.parent == run.parent || d.parent == run.id)
    {
        if let Some(name) = d.data["name"].as_str() {
            defs.insert(name.to_owned(), definition_for(run, name, definitions));
        }
    }
    CachedScores {
        signature: definition_signature(run, definitions),
        definitions: defs,
        metrics: BTreeMap::new(),
    }
}
fn record_score(snapshot: &mut CachedScores, point: HistoryPoint) {
    if let Some(stats) = snapshot.metrics.get_mut(&point.name) {
        stats.record(point);
    } else {
        let definition = snapshot
            .definitions
            .get(&point.name)
            .cloned()
            .unwrap_or_else(|| json!({}));
        snapshot
            .metrics
            .insert(point.name.clone(), ObjectiveStats::new(point, definition));
    }
}
pub(crate) fn persist_batch(
    db: &rusqlite::Connection,
    telemetry: &crate::telemetry::TelemetryBatch,
) -> Result<()> {
    use rusqlite::OptionalExtension;
    if telemetry.kind != crate::telemetry::TelemetryKind::Scalars {
        return Ok(());
    }
    let definitions = catalog::list(db, "metric", None)?;
    let mut runs: HashMap<String, CachedScores> = HashMap::new();
    let mut rebuild = BTreeSet::new();
    for point in crate::storage::codec::parse_scalar_batch(&telemetry.batch) {
        if rebuild.contains(&point.run_id) {
            continue;
        }
        if !runs.contains_key(&point.run_id) {
            let run = catalog::run_by_id(db, &point.run_id)?;
            let stored: Option<Vec<u8>> = db
                .query_row(
                    "SELECT body FROM run_scores WHERE run_id=?1",
                    [&point.run_id],
                    |r| r.get(0),
                )
                .optional()?;
            let stored: Option<CachedScores> =
                stored.and_then(|bytes| serde_json::from_slice(&bytes).ok());
            let signature = definition_signature(&run, &definitions);
            if let Some(stored) = stored.filter(|s| s.signature == signature) {
                runs.insert(point.run_id.clone(), stored);
            } else {
                let previous:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM run_summaries s,json_each(CAST(s.body AS TEXT),'$.metrics') WHERE s.run_id=?1)",[&point.run_id],|r|r.get(0))?;
                if previous {
                    rebuild.insert(point.run_id.clone());
                    continue;
                }
                runs.insert(point.run_id.clone(), empty_scores(&run, &definitions));
            }
        }
        if let Some(snapshot) = runs.get_mut(&point.run_id) {
            record_score(snapshot, point);
        }
    }
    for (id, snapshot) in runs {
        let bytes =
            serde_json::to_vec(&snapshot).map_err(|e| CatalogError::invalid(e.to_string()))?;
        db.execute("INSERT INTO run_scores VALUES(?1,?2) ON CONFLICT(run_id) DO UPDATE SET body=excluded.body",rusqlite::params![id,bytes])?;
    }
    Ok(())
}

async fn data_revisions(state: &AppState, runs: &[&Entity]) -> Result<Vec<(String, i64)>> {
    let names = runs
        .iter()
        .filter_map(|run| run.data["run_id"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    state
        .catalog
        .read(move |db| {
            use rusqlite::OptionalExtension;
            names
                .into_iter()
                .map(|name| {
                    let revision = db
                        .query_row(
                            "SELECT revision FROM data_revisions WHERE run_id=?1",
                            [&name],
                            |r| r.get::<_, i64>(0),
                        )
                        .optional()?
                        .unwrap_or(0);
                    Ok((name, revision))
                })
                .collect()
        })
        .await
}
