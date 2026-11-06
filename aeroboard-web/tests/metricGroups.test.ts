import assert from "node:assert/strict";
import test from "node:test";

import { moveMetricGroup, parseMetricGroups, saveMetricGroup, type MetricGroup } from "../src/lib/metricGroups.ts";

const groups: MetricGroup[] = [
  { id: "losses", name: "Losses", metrics: ["loss", "aux_loss"], hidden: true },
  { id: "metrics", name: "Metrics", metrics: ["accuracy"], hidden: false },
];

test("group preferences tolerate malformed values and share membership across stages", () => {
  assert.deepEqual(parseMetricGroups(null), []);
  assert.deepEqual(parseMetricGroups({ groups }), []);
  assert.deepEqual(parseMetricGroups([
    null, 7, { id: "", name: "Invalid" }, { id: "blank", name: " " },
    { id: "losses", name: " Losses ", metrics: ["train/loss", "val/loss", 7, ""], hidden: true },
    { id: "losses", name: "Duplicate", metrics: ["lr"] },
    { id: "metrics", name: "Metrics", metrics: ["val/loss", "val/accuracy"], hidden: "true" },
  ]), [
    { id: "losses", name: "Losses", metrics: ["loss"], hidden: true },
    { id: "metrics", name: "Metrics", metrics: ["accuracy"], hidden: false },
  ]);
});

test("saving a group transfers its charts while preserving the other groups' settings", () => {
  const result = saveMetricGroup(groups, { ...groups[1], name: "Quality", metrics: ["val/loss", "accuracy"] });
  assert.deepEqual(result, [
    { ...groups[0], metrics: ["aux_loss"] },
    { ...groups[1], name: "Quality", metrics: ["loss", "accuracy"] },
  ]);
  assert.deepEqual(groups[0].metrics, ["loss", "aux_loss"], "Existing state is not mutated");
});

test("creating a group appends it and keeps unavailable members in existing groups", () => {
  assert.deepEqual(saveMetricGroup(groups, { id: "speed", name: "Speed", metrics: ["lr"], hidden: false }), [
    ...groups, { id: "speed", name: "Speed", metrics: ["lr"], hidden: false },
  ]);
});

test("moving a hidden group carries all its members and visibility with it", () => {
  assert.deepEqual(moveMetricGroup(groups, "losses", "metrics"), [groups[1], groups[0]]);
  assert.deepEqual(moveMetricGroup(groups, "metrics", "losses"), [groups[1], groups[0]]);
  assert.equal(moveMetricGroup(groups, "losses", "losses"), groups);
  assert.equal(moveMetricGroup(groups, "missing", "metrics"), groups);
});
