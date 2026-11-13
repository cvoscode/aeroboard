import { changedSeries } from "./seriesSubscriptions.ts";
import type { MetricSummary, RunSummary, ScalarRow, TimeSeriesData } from "./types";

export type ScalarStore = Record<string, Record<string, TimeSeriesData>>;
export type RunSummaryStore = Record<string, RunSummary>;

type MetricSummaryInternal = MetricSummary & { __ts?: number };
const summaryIndexes = new WeakMap<RunSummary, Map<string,MetricSummaryInternal>>();

export const DEFAULT_BUFFER_SIZE = 128;
export const BUFFER_GROWTH_FACTOR = 2;
export const BUFFER_MIN_CAPACITY = 128;

export function makeEmptySeries(): TimeSeriesData {
  return {
    wallTimes: new Float64Array(DEFAULT_BUFFER_SIZE),
    get relTimes() { return this.wallTimes.subarray(0,this.count).map(time => time - (this.startTime ?? 0)); },
    steps: new Float64Array(DEFAULT_BUFFER_SIZE).fill(-1),
    get plotX() { return this.wallTimes.subarray(0,this.count).map((time,i) => this.steps[i] >= 0 ? this.steps[i] : time - (this.startTime ?? 0)); },
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
  series.steps = growFloat64Array(series.steps, nextSize, -1);
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
  let index = summaryIndexes.get(summary);
  if (!index) { index = new Map(metricEntries.map(entry => [entry.name,entry])); summaryIndexes.set(summary,index); }
  const existing = index.get(name);
  if (existing) {
    existing.min = Math.min(existing.min, value);
    existing.max = Math.max(existing.max, value);
    if ((existing.__ts ?? Number.NEGATIVE_INFINITY) <= tsMs) {
      existing.last = value;
      existing.__ts = tsMs;
    }
    return;
  }

  const entry = { name, last: value, min: value, max: value, __ts: tsMs };
  metricEntries.push(entry); index.set(name,entry);
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
  if (wallTime < startTimes[row.run_id]) {
    startTimes[row.run_id] = wallTime;
    for (const series of Object.values(store[row.run_id] ?? {})) {series.startTime=wallTime;changedSeries(series);}
  }

  if (!store[row.run_id]) store[row.run_id] = {};
  const runStore = store[row.run_id];

  if (!runStore[row.name]) runStore[row.name] = makeEmptySeries();
  const series = runStore[row.name];

  ensureSeriesCapacity(series);

  // Points normally arrive in time order; keep the arrays sorted if one doesn't.
  let i = series.count;
  if (i > 0 && wallTime < series.wallTimes[i - 1]) {
    i = upperBound(series, wallTime);
    for (const array of [series.wallTimes, series.steps, series.values]) {
      array.copyWithin(i + 1, i, series.count);
    }
  }
  series.wallTimes[i] = wallTime;
  series.startTime = startTimes[row.run_id];
  series.values[i] = row.value;
  series.steps[i] = step ?? -1;
  series.count += 1;
  if (step !== null && step >= 0) {series.minStep=Math.min(series.minStep ?? Infinity,step);series.maxStep=Math.max(series.maxStep ?? -Infinity,step);}
  changedSeries(series);

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
      if (!series.count) continue;
      const low = xAxis === "step" ? series.minStep ?? Infinity : series.wallTimes[0] - (xAxis === "rel" ? series.startTime ?? 0 : 0);
      const high = xAxis === "step" ? series.maxStep ?? -Infinity : series.wallTimes[series.count-1] - (xAxis === "rel" ? series.startTime ?? 0 : 0);
      min=Math.min(min,low);max=Math.max(max,high);
    }
  }

  return min === Infinity ? [0, 100] : [min, max];
}

/** Arrow vectors stay columnar; allocate indices once per series, never point objects. */
interface VectorLike {get(index:number):unknown}
export interface ScalarColumns {numRows:number;getChild(name:string):VectorLike|null}
export function mergeScalarColumns(store:ScalarStore,summaries:RunSummaryStore,starts:Record<string,number>,batches:ScalarColumns[],allowed?:Set<string>):boolean {
  let structureChanged=false;
  for (const batch of batches) {
    const times=batch.getChild('timestamp'),runs=batch.getChild('run_id'),names=batch.getChild('name'),values=batch.getChild('value'),steps=batch.getChild('step');
    if(!times||!runs||!names||!values)continue;
    const groups=new Map<string,Map<string,number[]>>();
    const legacy=new Map<string,Map<number,number>>();
    for(let i=0;i<batch.numRows;i++) {
      const run=runs.get(i) as string,name=names.get(i) as string;
      if(allowed&&!allowed.has(run))continue;
      if(isStepMetric(name)){let map=legacy.get(run);if(!map){map=new Map();legacy.set(run,map);}map.set(Number(times.get(i)),Number(values.get(i)));continue;}
      let metrics=groups.get(run);if(!metrics){metrics=new Map();groups.set(run,metrics);}
      let rows=metrics.get(name);if(!rows){rows=[];metrics.set(name,rows);}rows.push(i);
    }
    for(const [run,metrics] of groups) for(const [name,indices] of metrics) {
      const stepAt=(i:number)=>{const value=steps?.get(i);return value==null?legacy.get(run)?.get(Number(times.get(i)))??-1:Number(value);};
      indices.sort((a,b)=>Number(times.get(a))-Number(times.get(b))||stepAt(a)-stepAt(b));
      const runStore=store[run]??=Object.create(null) as Record<string,TimeSeriesData>;
      let series=runStore[name];
      if(!series){series=makeEmptySeries();runStore[name]=series;structureChanged=true;}
      const first=Number(times.get(indices[0]))/1000;
      if(starts[run]===undefined||first<starts[run]) {
        starts[run]=first;
        for(const other of Object.values(runStore)){other.startTime=first;changedSeries(other);}
      }
      const firstStep=stepAt(indices[0]);
      if(!series.count||first>series.wallTimes[series.count-1]||first===series.wallTimes[series.count-1]&&firstStep>series.steps[series.count-1]) {
        const summary=ensureRunSummary(summaries,run);let min=Infinity,max=-Infinity,last=-Infinity,lastValue=NaN;
        for(const row of indices){
          const time=Number(times.get(row))/1000,step=stepAt(row),value=Number(values.get(row));
          const previous=series.count-1;
          if(previous>=0&&series.wallTimes[previous]===time&&series.steps[previous]===step)continue;
          ensureSeriesCapacity(series);const i=series.count++;
          series.wallTimes[i]=time;series.steps[i]=step;series.values[i]=value;
          if(step>=0){series.minStep=Math.min(series.minStep??Infinity,step);series.maxStep=Math.max(series.maxStep??-Infinity,step);}
          if(Number.isFinite(value)){min=Math.min(min,value);max=Math.max(max,value);}
          last=time*1000;lastValue=value;
        }
        series.startTime=starts[run];updateMetricSummary(summary,name,last,lastValue);
        const metric=summaryIndexes.get(summary)!.get(name)!;metric.min=Math.min(metric.min,min);metric.max=Math.max(metric.max,max);
        changedSeries(series);continue;
      }
      const capacity=Math.max(DEFAULT_BUFFER_SIZE,series.count+indices.length);
      const wallTimes=new Float64Array(capacity),newValues=new Float64Array(capacity),newSteps=new Float64Array(capacity).fill(-1);
      let a=0,b=0,count=0,lastTime=-Infinity,lastStep=-Infinity;
      const put=(time:number,step:number,value:number)=>{
        if(time===lastTime&&step===lastStep)return;
        wallTimes[count]=time;newSteps[count]=step;newValues[count]=value;count++;lastTime=time;lastStep=step;
      };
      while(a<series.count||b<indices.length){
        const row=indices[b],time=b<indices.length?Number(times.get(row))/1000:Infinity,step=b<indices.length?stepAt(row):Infinity;
        if(a<series.count&&(b>=indices.length||series.wallTimes[a]<time||series.wallTimes[a]===time&&series.steps[a]<=step)) {put(series.wallTimes[a],series.steps[a],series.values[a]);a++;}
        else {put(time,step,Number(values.get(row)));b++;}
      }
      series.wallTimes=wallTimes;series.values=newValues;series.steps=newSteps;series.count=count;series.startTime=starts[run];
      let min=Infinity,max=-Infinity;series.minStep=Infinity;series.maxStep=-Infinity;
      for(let i=0;i<count;i++){if(Number.isFinite(newValues[i])){min=Math.min(min,newValues[i]);max=Math.max(max,newValues[i]);}if(newSteps[i]>=0){series.minStep=Math.min(series.minStep,newSteps[i]);series.maxStep=Math.max(series.maxStep,newSteps[i]);}}
      const summary=ensureRunSummary(summaries,run);updateMetricSummary(summary,name,wallTimes[count-1]*1000,newValues[count-1]);
      const metric=summaryIndexes.get(summary)!.get(name)!;metric.min=Math.min(metric.min,min);metric.max=Math.max(metric.max,max);
      changedSeries(series);
    }
  }
  return structureChanged;
}

const hyperparameterIndexes = new WeakMap<RunSummary, Map<string, RunSummary["hyperparameters"][number]>>();
/** Live hyperparameter updates use one index per summary object. */
export function setSummaryHyperparameter(summary: RunSummary, key: string, value: string, valueType: string) {
  let entries = hyperparameterIndexes.get(summary);
  if (!entries) {
    entries = new Map(summary.hyperparameters.map(entry => [entry.key, entry]));
    hyperparameterIndexes.set(summary, entries);
  }
  const existing = entries.get(key);
  if (existing) {
    existing.value = value;
    existing.value_type = valueType;
  } else {
    const entry = { key, value, value_type: valueType };
    entries.set(key, entry);
    summary.hyperparameters.push(entry);
  }
}
