import assert from "node:assert/strict";
import test from "node:test";
import type { RunSummary } from "../src/hooks/aeroboard/types.ts";
import type { Score } from "../src/hooks/useWorkspace.ts";
import { configurationChanges, formatConfiguration, parseConfiguration } from "../src/lib/configuration.ts";
import { metricValue } from "../src/lib/comparison.ts";
import { axisLabelLines, formatAxisValue, numericHyperparameter, parallelAxes, parallelPoints } from "../src/lib/parallelCoordinates.ts";

function run(run_id: string, hyperparameters: Record<string, string>, last = 1): RunSummary {
  return { run_id, hyperparameters: Object.entries(hyperparameters).map(([key, value]) => ({ key, value, value_type: "string" })),
    metric_summaries: [{ name: "val/loss", last, min: 0.1, max: 2 }] };
}
const runs = [run("baseline", { a: "0", c: "0.000001", constant: "64" }), run("candidate", { a: "2", b: "1", c: "0.000002", constant: "64" })];

test("parallel coordinates keep sparse runs and break paths at missing values", () => {
  const axes = parallelAxes(runs, ["a", "b", "c"], [], [], "last");
  const line = parallelPoints(axes, 0, 160, 36, 280, 72);
  assert.equal(line.points.length, 2);
  assert.equal(line.path.match(/M/g)?.length, 2, "Do not connect across an absent value");
  assert.equal(line.path.includes("L"), false);
  assert.equal(parallelPoints(axes, 1, 160, 36, 280, 72).points.length, 3);
});

test("empty and nonfinite hyperparameters remain missing, while zero is numeric", () => {
  for (const value of [undefined, "", " ", "False", "None", "NaN", "Infinity", "{'dim': 384}"]) assert.equal(numericHyperparameter(value), null);
  assert.equal(numericHyperparameter("0"), 0);
  assert.equal(numericHyperparameter("1e-8"), 1e-8);
});

test("constant axes show their real range and position observations at the center", () => {
  const axes = parallelAxes(runs, ["constant"], [], [], "last");
  assert.equal(axes[0].min, 64);
  assert.equal(axes[0].max, 64);
  assert.equal(parallelPoints(axes, 0, 160, 36, 280, 72).points[0].y, 176);
});

test("coordinate normalization handles finite extremes and subnormal ranges", () => {
  for (const [min, max] of [[-1e308, 1e308], [0, Number.MIN_VALUE]]) {
    const axes = [{ key: "hp:x", label: "x", values: [min, max], min, max }];
    assert.equal(parallelPoints(axes, 0, 160, 36, 280, 72).points[0].y, 316);
    assert.equal(parallelPoints(axes, 1, 160, 36, 280, 72).points[0].y, 36);
  }
});

test("table and parallel axes share the selected reducer and preserve missing scores", () => {
  const scores: Score[] = [{ run_id: "baseline", name: "val/loss", goal: "minimize", target: null, unit: null,
    best: 0.1, best_step: 10, best_at_ms: null, last: 1, last_step: 20, last_is_finite: true, finite_count: 3,
    at_step: 0, time_to_target_ms: null, target_at_ms: null }];
  assert.equal(metricValue(runs[0], "val/loss", scores, "best"), 0.1);
  assert.deepEqual(parallelAxes(runs, [], ["val/loss"], scores, "best")[0].values, [0.1, null]);
  assert.deepEqual(parallelAxes(runs, [], ["val/loss"], scores, "at_step")[0].values, [0, null]);
  assert.equal(metricValue(runs[1], "val/loss", [], "best"), null);
  assert.equal(metricValue(runs[1], "val/loss", [], "last"), 1);
});

test("axis labels stay bounded and tiny values retain meaningful digits", () => {
  const lines = axisLabelLines("encoder.really_long_configuration_name_with_many_nested_fields");
  assert.ok(lines.length <= 3 && lines.every(line => line.length <= 20));
  assert.equal(formatAxisValue(0.000001), "1.00e-6");
  assert.equal(formatAxisValue(64), "64");
});

test("configuration parsing supports JSON and Python literals without evaluating expressions", () => {
  assert.deepEqual(parseConfiguration("{'dim': 384, 'dropout': 0.1, 'checkpointing': True, 'optional': None, 'layers': [1, 2,], 'name': 'it\\'s fine'}"),
    { dim: 384, dropout: 0.1, checkpointing: true, optional: null, layers: [1, 2], name: "it's fine" });
  assert.deepEqual(parseConfiguration('{"dim":384,"enabled":false}'), { dim: 384, enabled: false });
  for (const raw of ["__import__('os').system('echo unsafe')", "{'a': object()}", "{'unclosed': [1, 2}", "{'a': 1} trailing"]) assert.equal(parseConfiguration(raw), raw);
});

test("configuration differences report nested fields, additions, and missing versus null", () => {
  assert.deepEqual(configurationChanges(
    { encoder: parseConfiguration("{'dim': 384, 'dropout': 0.1, 'optional': None}"), warmup: 0.02 },
    { encoder: parseConfiguration('{"dropout":0.3,"dim":384,"new":true}') },
  ), [
    { key: "encoder.dropout", before: 0.1, after: 0.3 },
    { key: "encoder.new", before: undefined, after: true },
    { key: "encoder.optional", before: null, after: undefined },
    { key: "warmup", before: 0.02, after: undefined },
  ]);
  assert.deepEqual(configurationChanges({ options: [{ a: 1, b: 2 }] }, { options: [{ b: 2, a: 1 }] }), []);
  assert.equal(formatConfiguration(undefined), "Missing");
  assert.equal(formatConfiguration(null), "null");
  assert.deepEqual(configurationChanges({}, { constructor: 1 }), [{ key: "constructor", before: undefined, after: 1 }]);
});
