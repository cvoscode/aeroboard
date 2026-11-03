import type { MetricSummary, RunSummary, ScalarRow, TimeSeriesData } from "./types";

export type ScalarStore = Record<string, Record<string, TimeSeriesData>>;
export type RunSummaryStore = Record<string, RunSummary>;

type MetricSummaryInternal = MetricSummary & { __ts?: number };

export const DEFAULT_BUFFER_SIZE = 4096;
export const BUFFER_GROWTH_FACTOR = 2;
export const BUFFER_MIN_CAPACITY = 1024;

export function makeEmptySeries(): TimeSeriesData {
  return {
    wallTimes: new Float64Array(DEFAULT_BUFFER_SIZE),
    relTimes: new Float64Array(DEFAULT_BUFFER_SIZE),
    steps: new Float64Array(DEFAULT_BUFFER_SIZE).fill(-1),
    plotX: new Float64Array(DEFAULT_BUFFER_SIZE),
    values: new Float64Array(DEFAULT_BUFFER_SIZE),
    count: 0,
  };
}

function growFloat64Array(source: Float64Array, nextSize: number, fillValue?: number): Float64Array {
  const next = new Float64Array(nextSize);
  next.set(source);
  if (fillValue !== undefined && nextSize > source.length) {
    next.fill(fillValue, source.length);
  }
  return next;
}

function ensureSeriesCapacity(series: TimeSeriesData) {
  if (series.count < series.values.length) return;
  const nextSize = Math.max(BUFFER_MIN_CAPACITY, series.values.length * BUFFER_GROWTH_FACTOR);
  series.wallTimes = growFloat64Array(series.wallTimes, nextSize);
  series.relTimes = growFloat64Array(series.relTimes, nextSize);
  series.steps = growFloat64Array(series.steps, nextSize, -1);
  series.plotX = growFloat64Array(series.plotX, nextSize);
  series.values = growFloat64Array(series.values, nextSize);
}

export function ensureRunSummary(summaryStore: RunSummaryStore, runId: string): RunSummary {
  if (!summaryStore[runId]) {
    summaryStore[runId] = { run_id: runId, hyperparameters: [], metric_summaries: [] };
  }
  return summaryStore[runId];
}

export function updateMetricSummary(summary: RunSummary, name: string, tsMs: number, value: number) {
  const metricEntries = summary.metric_summaries as MetricSummaryInternal[];
  const existing = metricEntries.find((entry) => entry.name === name);
  if (existing) {
    existing.min = Math.min(existing.min, value);
    existing.max = Math.max(existing.max, value);
    if ((existing.__ts ?? Number.NEGATIVE_INFINITY) <= tsMs) {
      existing.last = value;
      existing.__ts = tsMs;
    }
    return;
  }

  metricEntries.push({ name, last: value, min: value, max: value, __ts: tsMs });
}

function isStepMetric(name: string): boolean {
  return name === "step" || name.endsWith("/step");
}

/** Index of the first point with wallTime > `wallTime` (points are kept sorted). */
function upperBound(series: TimeSeriesData, wallTime: number): number {
  let lo = 0;
  let hi = series.count;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (series.wallTimes[mid] <= wallTime) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

export function ingestPoint(
  store: ScalarStore,
  summaryStore: RunSummaryStore,
  startTimes: Record<string, number>,
  row: ScalarRow,
  step: number | null,
): boolean {
  const wallTime = row.ts_ms / 1000;

  if (startTimes[row.run_id] === undefined) startTimes[row.run_id] = wallTime;
  const relTime = wallTime - startTimes[row.run_id];

  if (!store[row.run_id]) store[row.run_id] = {};
  const runStore = store[row.run_id];

  if (!runStore[row.name]) runStore[row.name] = makeEmptySeries();
  const series = runStore[row.name];

  ensureSeriesCapacity(series);

  // Points normally arrive in time order; keep the arrays sorted if one doesn't.
  let i = series.count;
  if (i > 0 && wallTime < series.wallTimes[i - 1]) {
    i = upperBound(series, wallTime);
    for (const array of [series.wallTimes, series.relTimes, series.steps, series.plotX, series.values]) {
      array.copyWithin(i + 1, i, series.count);
    }
  }
  series.wallTimes[i] = wallTime;
  series.relTimes[i] = relTime;
  series.plotX[i] = step ?? relTime;
  series.values[i] = row.value;
  series.steps[i] = step ?? -1;
  series.count += 1;

  updateMetricSummary(ensureRunSummary(summaryStore, row.run_id), row.name, row.ts_ms, row.value);
  return true;
}

/**
 * Ingest rows (one live batch, or a page of history). The step comes from the
 * row's `step` column; data from older clients logged it as a separate "step"
 * metric with the same timestamp, which is used as a fallback.
 */
export function ingestScalarRows(
  store: ScalarStore,
  summaryStore: RunSummaryStore,
  startTimes: Record<string, number>,
  rows: ScalarRow[],
): boolean {
  const legacySteps: Record<string, Map<number, number>> = {};
  for (const row of rows) {
    if (isStepMetric(row.name)) {
      (legacySteps[row.run_id] ??= new Map()).set(row.ts_ms, row.value);
    }
  }

  let ingested = false;
  for (const row of rows) {
    if (isStepMetric(row.name)) continue;
    const step = row.step ?? legacySteps[row.run_id]?.get(row.ts_ms) ?? null;
    ingestPoint(store, summaryStore, startTimes, row, step);
    ingested = true;
  }
  return ingested;
}

/** The points currently held for a run, as rows (steps already resolved). */
function storedRows(store: ScalarStore, runId: string): ScalarRow[] {
  const rows: ScalarRow[] = [];
  for (const [name, series] of Object.entries(store[runId] ?? {})) {
    for (let i = 0; i < series.count; i++) {
      rows.push({
        ts_ms: Math.round(series.wallTimes[i] * 1000),
        run_id: runId,
        name,
        value: series.values[i],
        step: series.steps[i] >= 0 ? series.steps[i] : null,
      });
    }
  }
  return rows;
}

/**
 * Merge fetched history for `runIds` with whatever is already in the store
 * (typically live points that arrived before or during the fetch). Each run's
 * series are rebuilt sorted by time with duplicate (metric, timestamp, step)
 * points removed, and relative time is re-based on the run's true first point.
 */
export function mergeHistoryRows(
  store: ScalarStore,
  summaryStore: RunSummaryStore,
  startTimes: Record<string, number>,
  runIds: string[],
  historyRows: ScalarRow[],
) {
  const legacySteps: Record<string, Map<number, number>> = {};
  const byRun: Record<string, ScalarRow[]> = {};
  for (const runId of runIds) byRun[runId] = storedRows(store, runId);

  for (const row of historyRows) {
    if (isStepMetric(row.name)) {
      (legacySteps[row.run_id] ??= new Map()).set(row.ts_ms, row.value);
    }
  }
  for (const row of historyRows) {
    if (isStepMetric(row.name) || !byRun[row.run_id]) continue;
    const step = row.step ?? legacySteps[row.run_id]?.get(row.ts_ms) ?? null;
    byRun[row.run_id].push({ ...row, step });
  }

  for (const runId of runIds) {
    const rows = byRun[runId];
    if (rows.length === 0) continue;
    rows.sort((a, b) => a.ts_ms - b.ts_ms || (a.step ?? -1) - (b.step ?? -1));

    delete store[runId];
    startTimes[runId] = rows[0].ts_ms / 1000;
    const seen = new Set<string>();
    for (const row of rows) {
      // Same metric, millisecond and step means the same point seen twice (live and
      // history). Fast loops log many steps within one millisecond, so the step counts.
      const key = `${row.name}\u0000${row.ts_ms}\u0000${row.step ?? ""}`;
      if (seen.has(key)) continue;
      seen.add(key);
      ingestPoint(store, summaryStore, startTimes, row, row.step ?? null);
    }
  }
}

export function getChartData(store: ScalarStore, runId: string, name: string): TimeSeriesData | null {
  if (!runId) return null;
  const series = store[runId]?.[name];
  return series && series.count > 0 ? series : null;
}

export function getMetricsForRun(store: ScalarStore, runId: string): string[] {
  if (!runId) return [];
  return Object.keys(store[runId] ?? {});
}

export function getGlobalXBounds(
  store: ScalarStore,
  targetRuns: string[],
  xAxis: "wall" | "rel" | "step",
): [number, number] {
  let min = Infinity;
  let max = -Infinity;

  for (const runId of targetRuns) {
    const runStore = store[runId];
    if (!runStore) continue;
    for (const series of Object.values(runStore)) {
      const xArr = xAxis === "wall" ? series.wallTimes : xAxis === "rel" ? series.relTimes : series.steps;
      for (let i = 0; i < series.count; i++) {
        if (xAxis === "step" && xArr[i] < 0) continue;
        if (xArr[i] < min) min = xArr[i];
        if (xArr[i] > max) max = xArr[i];
      }
    }
  }

  return min === Infinity ? [0, 100] : [min, max];
}
