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

export function ingestPoint(
  store: ScalarStore,
  summaryStore: RunSummaryStore,
  startTimes: Record<string, number>,
  row: ScalarRow,
  step: number | null,
): boolean {
  const wallTime = row.ts_ms / 1000;

  if (!startTimes[row.run_id]) startTimes[row.run_id] = wallTime;
  const relTime = wallTime - startTimes[row.run_id];

  if (!store[row.run_id]) store[row.run_id] = {};
  const runStore = store[row.run_id];

  if (!runStore[row.name]) runStore[row.name] = makeEmptySeries();
  const series = runStore[row.name];

  ensureSeriesCapacity(series);

  const i = series.count;
  series.wallTimes[i] = wallTime;
  series.relTimes[i] = relTime;
  series.plotX[i] = step ?? relTime;
  series.values[i] = row.value;
  if (step !== null) series.steps[i] = step;
  series.count = i + 1;

  updateMetricSummary(ensureRunSummary(summaryStore, row.run_id), row.name, row.ts_ms, row.value);
  return true;
}

export function ingestScalarRows(
  store: ScalarStore,
  summaryStore: RunSummaryStore,
  startTimes: Record<string, number>,
  rows: ScalarRow[],
) {
  const stepMap: Record<string, Map<number, number>> = {};
  for (const row of rows) {
    if (row.name === "step" || row.name.endsWith("/step")) {
      if (!stepMap[row.run_id]) stepMap[row.run_id] = new Map();
      stepMap[row.run_id].set(row.ts_ms, row.value);
    }
  }

  for (const row of rows) {
    if (row.name === "step" || row.name.endsWith("/step")) continue;
    const step = stepMap[row.run_id]?.get(row.ts_ms) ?? null;
    ingestPoint(store, summaryStore, startTimes, row, step);
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
