import assert from "node:assert/strict";
import test from "node:test";

import {
  ingestScalarRows,
  mergeHistoryRows,
  type RunSummaryStore,
  type ScalarStore,
} from "../src/hooks/aeroboard/scalarStore.ts";
import type { ScalarRow } from "../src/hooks/aeroboard/types.ts";

function emptyStores() {
  return { store: {} as ScalarStore, summaries: {} as RunSummaryStore, starts: {} as Record<string, number> };
}

function points(store: ScalarStore, run: string, name: string) {
  const series = store[run][name];
  return Array.from({ length: series.count }, (_, i) => ({
    ts: Math.round(series.wallTimes[i] * 1000),
    value: series.values[i],
    step: series.steps[i],
    x: series.plotX[i],
  }));
}

const row = (ts_ms: number, name: string, value: number, step?: number | null): ScalarRow => ({ ts_ms, run_id: "r", name, value, step });

test("live batches use each row's own step", () => {
  const { store, summaries, starts } = emptyStores();
  // One batch spanning three log() calls; previously all got the first step.
  ingestScalarRows(store, summaries, starts, [row(1000, "loss", 0.9, 1), row(2000, "loss", 0.8, 2), row(3000, "loss", 0.7, 3)]);
  assert.deepEqual(points(store, "r", "loss").map((p) => p.x), [1, 2, 3]);
});

test("legacy 'step' metrics are matched by timestamp", () => {
  const { store, summaries, starts } = emptyStores();
  ingestScalarRows(store, summaries, starts, [
    row(1000, "loss", 0.9), row(1000, "step", 10),
    row(2000, "loss", 0.8), row(2000, "step", 20),
  ]);
  assert.deepEqual(Object.keys(store.r), ["loss"]);
  assert.deepEqual(points(store, "r", "loss").map((p) => p.step), [10, 20]);
});

test("out-of-order live points are inserted in time order", () => {
  const { store, summaries, starts } = emptyStores();
  ingestScalarRows(store, summaries, starts, [row(1000, "loss", 1, 1), row(3000, "loss", 3, 3)]);
  ingestScalarRows(store, summaries, starts, [row(2000, "loss", 2, 2)]);
  assert.deepEqual(points(store, "r", "loss").map((p) => p.value), [1, 2, 3]);
});

test("history merges with live points without duplicates and rebases relative time", () => {
  const { store, summaries, starts } = emptyStores();
  // Live data arrives first (from t=5s), then history (t=1..6s) that overlaps it.
  ingestScalarRows(store, summaries, starts, [row(5000, "loss", 0.5, 5), row(6000, "loss", 0.4, 6)]);
  const history = [1, 2, 3, 4, 5, 6].map((s) => row(s * 1000, "loss", 1 - s / 10, s));
  mergeHistoryRows(store, summaries, starts, ["r"], history);

  const merged = points(store, "r", "loss");
  assert.deepEqual(merged.map((p) => p.ts), [1000, 2000, 3000, 4000, 5000, 6000]);
  assert.equal(starts.r, 1);
  assert.equal(store.r.loss.relTimes[0], 0);

  const summary = summaries.r.metric_summaries.find((entry) => entry.name === "loss")!;
  assert.equal(summary.last, 0.4);
  assert.equal(summary.max, 0.9);
});

test("merge keeps distinct steps logged within the same millisecond", () => {
  const { store, summaries, starts } = emptyStores();
  ingestScalarRows(store, summaries, starts, [row(1000, "loss", 0.3, 3)]);
  mergeHistoryRows(store, summaries, starts, ["r"], [row(1000, "loss", 0.1, 1), row(1000, "loss", 0.2, 2), row(1000, "loss", 0.3, 3)]);
  assert.deepEqual(points(store, "r", "loss").map((p) => p.step), [1, 2, 3]);
});


test('column merging keeps live overlap, sparse steps, bounds, and small allocations',async()=>{
  const {mergeScalarColumns,getGlobalXBounds,DEFAULT_BUFFER_SIZE}=await import('../src/hooks/aeroboard/scalarStore.ts');
  const {seriesVersion}=await import('../src/hooks/aeroboard/seriesSubscriptions.ts');
  const store:ScalarStore={},summaries:RunSummaryStore={},starts:Record<string,number>={};
  const columns=(times:number[],steps:(number|null)[],values:number[])=>({numRows:times.length,getChild:(name:string)=>({get:(i:number)=>({timestamp:times,run_id:times.map(()=> 'r'),name:times.map(()=> 'loss'),value:values,step:steps}[name]?.[i])})});
  mergeScalarColumns(store,summaries,starts,[columns([2000,3000],[2,3],[2,3])]);
  const series=store.r.loss,version=seriesVersion(series);
  mergeScalarColumns(store,summaries,starts,[columns([1000,2000,2000],[1,2,2.5],[1,2,2.5])]);
  assert.equal(store.r.loss,series);
  assert.deepEqual(Array.from(series.values.subarray(0,series.count)),[1,2,2.5,3]);
  assert.deepEqual(getGlobalXBounds(store,['r'],'rel'),[0,2]);
  assert.deepEqual(getGlobalXBounds(store,['r'],'step'),[1,3]);
  assert.ok(seriesVersion(series)>version);
  assert.ok(DEFAULT_BUFFER_SIZE<=256);
});

test('hyperparameter updates replace the indexed entry without appending duplicates',async()=>{
  const {setSummaryHyperparameter,ensureRunSummary}=await import('../src/hooks/aeroboard/scalarStore.ts');
  const summary=ensureRunSummary({},'r');
  setSummaryHyperparameter(summary,'lr','1','int');
  setSummaryHyperparameter(summary,'lr','0.1','float');
  assert.deepEqual(summary.hyperparameters,[{key:'lr',value:'0.1',value_type:'float'}]);
});
