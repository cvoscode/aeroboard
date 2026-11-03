//! Incrementally maintained per-run summaries (last/min/max per metric plus
//! hyperparameters).
//!
//! A run's summary is computed from storage once, the first time it is requested,
//! and from then on the persistence loop folds every newly persisted batch into it.
//! `/api/runs/summary` therefore costs a map lookup instead of a full history scan.
//!
//! Consistency: the persistence loop calls `record` *after* a batch is stored, and
//! `record` also applies to runs that are still loading. When a load finishes, the
//! deltas recorded meanwhile are folded on top of what storage returned. Folding a
//! batch twice is harmless: min/max/last and keyed hyperparameters are idempotent.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use arrow::array::{Array, AsArray};
use tokio::sync::Mutex;

use crate::history::{HistoryPoint, HyperparameterEntry, MetricSummary, RunSummary};
use crate::storage::{BoxError, HistoryQueryOptions, StorageEngine};
use crate::telemetry::{TelemetryBatch, TelemetryKind};

#[derive(Clone, Copy, Debug)]
pub(crate) struct MetricStats {
    /// Position of `last` as (timestamp, step): fast loops log several steps per
    /// millisecond, and storage may return such ties in any order.
    last_ts_ms: i64,
    last_step: f64,
    last: f64,
    min: f64,
    max: f64,
}

impl MetricStats {
    fn new(ts_ms: i64, step: Option<f64>, value: f64) -> Self {
        Self {
            last_ts_ms: ts_ms,
            last_step: step.unwrap_or(f64::NEG_INFINITY),
            last: value,
            min: if value.is_finite() {
                value
            } else {
                f64::INFINITY
            },
            max: if value.is_finite() {
                value
            } else {
                f64::NEG_INFINITY
            },
        }
    }

    fn update(&mut self, ts_ms: i64, step: Option<f64>, value: f64) {
        let step = step.unwrap_or(f64::NEG_INFINITY);
        if (ts_ms, step) >= (self.last_ts_ms, self.last_step) {
            self.last_ts_ms = ts_ms;
            self.last_step = step;
            self.last = value;
        }
        // Keep non-finite last observations as diagnostics, outside the range
        // used by objective filters and comparisons.
        if value.is_finite() {
            self.min = self.min.min(value);
            self.max = self.max.max(value);
        }
    }

    fn merge(&mut self, other: &MetricStats) {
        self.update(other.last_ts_ms, Some(other.last_step), other.last);
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RunAccumulator {
    metrics: BTreeMap<String, MetricStats>,
    hyperparameters: BTreeMap<String, HyperparameterEntry>,
}

impl RunAccumulator {
    pub(crate) fn add_point(&mut self, name: &str, ts_ms: i64, step: Option<f64>, value: f64) {
        match self.metrics.get_mut(name) {
            Some(stats) => stats.update(ts_ms, step, value),
            None => {
                self.metrics
                    .insert(name.to_string(), MetricStats::new(ts_ms, step, value));
            }
        }
    }

    pub(crate) fn set_hyperparameter(&mut self, entry: HyperparameterEntry) {
        self.hyperparameters.insert(entry.key.clone(), entry);
    }

    /// Fold `newer` on top of `self`; hyperparameters in `newer` win.
    fn merge(&mut self, newer: &RunAccumulator) {
        for (name, stats) in &newer.metrics {
            self.metrics
                .entry(name.clone())
                .and_modify(|existing| existing.merge(stats))
                .or_insert(*stats);
        }
        for (key, entry) in &newer.hyperparameters {
            self.hyperparameters.insert(key.clone(), entry.clone());
        }
    }

    fn is_empty(&self) -> bool {
        self.metrics.is_empty() && self.hyperparameters.is_empty()
    }

    pub(crate) fn to_summary(&self, run_id: &str) -> RunSummary {
        RunSummary {
            run_id: run_id.to_string(),
            hyperparameters: self.hyperparameters.values().cloned().collect(),
            metric_summaries: self
                .metrics
                .iter()
                .map(|(name, stats)| MetricSummary {
                    name: name.clone(),
                    last: stats.last,
                    min: stats.min,
                    max: stats.max,
                })
                .collect(),
        }
    }
}

/// Build accumulators from raw history points and hyperparameters.
pub(crate) fn accumulate(
    points: &[HistoryPoint],
    hyperparameters: Vec<(String, HyperparameterEntry)>,
) -> BTreeMap<String, RunAccumulator> {
    let mut runs: BTreeMap<String, RunAccumulator> = BTreeMap::new();
    for point in points {
        runs.entry(point.run_id.clone()).or_default().add_point(
            &point.name,
            point.ts_ms,
            point.step,
            point.value,
        );
    }
    for (run_id, entry) in hyperparameters {
        runs.entry(run_id).or_default().set_hyperparameter(entry);
    }
    runs
}

struct CachedRun {
    accumulator: RunAccumulator,
    loaded: bool,
}

#[derive(Default)]
struct CacheState {
    runs: HashMap<String, CachedRun>,
    /// Bumped by `clear` so loads that started before a delete are discarded.
    generation: u64,
}

#[derive(Default)]
pub struct SummaryCache {
    state: Mutex<CacheState>,
}

impl SummaryCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Summaries for `run_ids` (all persisted runs when empty). Runs without any
    /// metrics or hyperparameters are omitted.
    pub async fn summaries(
        &self,
        storage: &dyn StorageEngine,
        run_ids: &BTreeSet<String>,
    ) -> Result<Vec<RunSummary>, BoxError> {
        let targets: BTreeSet<String> = if run_ids.is_empty() {
            storage.list_runs().await?.into_iter().collect()
        } else {
            run_ids.clone()
        };

        // Mark missing runs as loading so `record` starts collecting deltas for them.
        let (to_load, generation) = {
            let mut state = self.state.lock().await;
            let mut to_load = BTreeSet::new();
            for run_id in &targets {
                let entry = state
                    .runs
                    .entry(run_id.clone())
                    .or_insert_with(|| CachedRun {
                        accumulator: RunAccumulator::default(),
                        loaded: false,
                    });
                if !entry.loaded {
                    to_load.insert(run_id.clone());
                }
            }
            (to_load, state.generation)
        };

        if !to_load.is_empty() {
            let history = storage
                .query_history(&HistoryQueryOptions {
                    run_ids: to_load.clone(),
                    ..HistoryQueryOptions::default()
                })
                .await?;
            let hyperparameters = storage.query_hyperparameters(&to_load).await?;
            let mut loaded = accumulate(&history.rows, hyperparameters);

            let mut state = self.state.lock().await;
            if state.generation == generation {
                for run_id in &to_load {
                    let mut base = loaded.remove(run_id).unwrap_or_default();
                    if let Some(entry) = state.runs.get_mut(run_id) {
                        base.merge(&entry.accumulator);
                        entry.accumulator = base;
                        entry.loaded = true;
                    }
                }
            }
        }

        let state = self.state.lock().await;
        Ok(targets
            .iter()
            .filter_map(|run_id| {
                let cached = state.runs.get(run_id)?;
                (!cached.accumulator.is_empty()).then(|| cached.accumulator.to_summary(run_id))
            })
            .collect())
    }

    /// Fold a freshly persisted batch into the summaries of runs already in the cache.
    pub async fn record(&self, telemetry: &TelemetryBatch) {
        let batch = &telemetry.batch;
        match telemetry.kind {
            TelemetryKind::Scalars => {
                let ts = batch
                    .column(0)
                    .as_primitive::<arrow::datatypes::TimestampMillisecondType>();
                let run_ids = batch.column(1).as_string::<i32>();
                let names = batch.column(2).as_string::<i32>();
                let values = batch
                    .column(3)
                    .as_primitive::<arrow::datatypes::Float64Type>();
                let steps = batch
                    .column(4)
                    .as_primitive::<arrow::datatypes::Float64Type>();
                let mut state = self.state.lock().await;
                for i in 0..batch.num_rows() {
                    if run_ids.is_null(i) || names.is_null(i) || ts.is_null(i) || values.is_null(i)
                    {
                        continue;
                    }
                    if let Some(cached) = state.runs.get_mut(run_ids.value(i)) {
                        let step = (!steps.is_null(i)).then(|| steps.value(i));
                        cached.accumulator.add_point(
                            names.value(i),
                            ts.value(i),
                            step,
                            values.value(i),
                        );
                    }
                }
            }
            TelemetryKind::Hyperparameters => {
                let run_ids = batch.column(1).as_string::<i32>();
                let keys = batch.column(2).as_string::<i32>();
                let values = batch.column(3).as_string::<i32>();
                let value_types = batch.column(4).as_string::<i32>();
                let mut state = self.state.lock().await;
                for i in 0..batch.num_rows() {
                    if run_ids.is_null(i)
                        || keys.is_null(i)
                        || values.is_null(i)
                        || value_types.is_null(i)
                    {
                        continue;
                    }
                    if let Some(cached) = state.runs.get_mut(run_ids.value(i)) {
                        cached.accumulator.set_hyperparameter(HyperparameterEntry {
                            key: keys.value(i).to_string(),
                            value: values.value(i).to_string(),
                            value_type: value_types.value(i).to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    pub async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.runs.clear();
        state.generation += 1;
    }
}
