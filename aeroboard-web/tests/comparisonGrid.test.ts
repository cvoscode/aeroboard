import assert from "node:assert/strict";
import test from "node:test";
import type { RunSummary } from "../src/hooks/aeroboard/types.ts";
import type { Score } from "../src/hooks/useWorkspace.ts";
import { compareGridValues, comparisonRows, isNumericColumn } from "../src/lib/comparisonGrid.ts";

const runs: RunSummary[] = [
  { run_id: "baseline", hyperparameters: [{ key: "encoder.dim", value: "00384", value_type: "int" }, { key: "encoder", value: "{'dropout': 0.1}", value_type: "string" }],
    metric_summaries: [{ name: "val/loss", last: 0.8, min: 0.1, max: 1 }] },
  { run_id: "candidate", hyperparameters: [{ key: "encoder.dim", value: "512", value_type: "int" }],
    metric_summaries: [{ name: "val/loss", last: 0.5, min: 0, max: 1 }] },
  { run_id: "missing", hyperparameters: [], metric_summaries: [] },
];
const score = (run_id: string, best: number, unit: string | null = null): Score => ({
  run_id, name: "val/loss", best, last: best + 0.3, at_step: 0, goal: "minimize", unit, target: null,
  best_step: 0, best_at_ms: null, last_step: 1, last_is_finite: true, finite_count: 2, time_to_target_ms: null, target_at_ms: null,
});

test("grid rows index metric reducers and retain original hyperparameter values", () => {
  const scores = [score("baseline", 0.1), score("candidate", 0)];
  const rows = comparisonRows(runs, ["val/loss"], scores, "best", "baseline", []);
  assert.equal(rows[0].hyperparameters.get("encoder.dim"), "00384");
  assert.equal(rows[0].numericHyperparameters.get("encoder.dim"), 384);
  assert.equal(rows[0].baseline, true);
  assert.equal(rows[1].metrics.get("val/loss"), 0);
  assert.equal(rows[2].metrics.get("val/loss"), null);
  assert.deepEqual(rows[1].improvements.get("val/loss"), { absolute: 0.1, percent: 100 });
  assert.equal(comparisonRows(runs, ["val/loss"], scores, "at_step", "", [])[0].metrics.get("val/loss"), 0);
  assert.equal(comparisonRows(runs, ["val/loss"], [], "last", "", [])[0].metrics.get("val/loss"), 0.8);
});

test("baseline deltas require matching goals and units", () => {
  const rows = comparisonRows(runs, ["val/loss"], [score("baseline", 0.1), score("candidate", 0, "percent")], "best", "baseline", []);
  assert.equal(rows[1].improvements.size, 0);
});

test("numeric column inference excludes missing values and keeps configurations as text", () => {
  const rows = comparisonRows(runs, [], [], "last", "", []);
  assert.equal(isNumericColumn(rows, "encoder.dim"), true);
  assert.equal(isNumericColumn(rows, "encoder"), false);
  assert.equal(isNumericColumn(rows, "missing"), false);
  assert.match(rows[0].searchText, /encoder:\{'dropout': 0\.1\}/);
});

test("numeric sorting and both directions keep missing cells last", () => {
  const values = [100, null, 2, 0, undefined];
  assert.deepEqual([...values].sort((a, b) => compareGridValues(a, b, false)), [0, 2, 100, null, undefined]);
  assert.deepEqual([...values].sort((a, b) => -compareGridValues(a, b, true)), [100, 2, 0, null, undefined]);
});
