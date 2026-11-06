import assert from "node:assert/strict";
import test from "node:test";

import { formatAge } from "../src/lib/format.ts";
import { buildExperimentGroups, filterExperimentGroups, runLabelInGroup, sortRunsNewestFirst } from "../src/lib/runGroups.ts";

const runs = ["alpha/seed0", "beta/seed0", "beta/seed0-v2", "gamma/seed0"];

test("the group holding the newest run comes first, whatever its name", () => {
  const groups = buildExperimentGroups(runs, {"alpha/seed0": 100, "beta/seed0": 200, "beta/seed0-v2": 300, "gamma/seed0": 250});
  assert.deepEqual(groups.map(group => group.experimentName), ["beta/seed0", "gamma/seed0", "alpha/seed0"]);
  assert.deepEqual(groups[0].runs.map(run => run.fullRunId), ["beta/seed0-v2", "beta/seed0"]);
  assert.equal(groups[0].latestRunId, "beta/seed0-v2");
  assert.equal(groups[0].latestStartedAtMs, 300);
});

test("start time outranks version number inside a group", () => {
  const [group] = buildExperimentGroups(["a-v2", "a-v3"], {"a-v2": 500, "a-v3": 100});
  assert.deepEqual(group.runs.map(run => run.fullRunId), ["a-v2", "a-v3"]);
});

test("runs without start times fall back to name and version order", () => {
  const groups = buildExperimentGroups(["b", "a", "a-v2"]);
  assert.deepEqual(groups.map(group => group.experimentName), ["a", "b"]);
  assert.deepEqual(groups[0].runs.map(run => run.fullRunId), ["a-v2", "a"]);
});

test("a timed run sorts ahead of untimed runs", () => {
  assert.deepEqual(sortRunsNewestFirst(["a", "b", "c"], {c: 5, b: 9}), ["b", "c", "a"]);
  assert.deepEqual(buildExperimentGroups(["a", "z"], {z: 1}).map(group => group.experimentName), ["z", "a"]);
});

test("a filter keeps the size of the whole group, so a partial match still shows its heading", () => {
  const [group] = filterExperimentGroups(buildExperimentGroups(["a", "a-v2", "a-v3"]), "v3");
  assert.deepEqual(group.runs.map(run => run.fullRunId), ["a-v3"]);
  assert.equal(group.totalRuns, 3);
});

test("rows under a group heading drop the repeated name but keep what tells runs apart", () => {
  const [group] = buildExperimentGroups(["exp-v2", "exp-plabc", "exp-plabc-v3"]);
  assert.deepEqual(group.runs.map(runLabelInGroup), ["plabc v3", "v2", "plabc v1"]);
});

test("ages stay short at every scale", () => {
  const now = Date.UTC(2026, 10, 6, 12);
  assert.equal(formatAge(now - 20_000, now), "now");
  assert.equal(formatAge(now - 12 * 60_000, now), "12m");
  assert.equal(formatAge(now - 3 * 3_600_000, now), "3h");
  assert.equal(formatAge(now - 5 * 86_400_000, now), "5d");
  assert.match(formatAge(now - 60 * 86_400_000, now), /Sep/);
});
