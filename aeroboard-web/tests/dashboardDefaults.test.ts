import assert from "node:assert/strict";
import test from "node:test";

import {
  defaultOrderRank,
  isStarredByDefault,
  resolveDashboardDefaults,
} from "../src/lib/dashboardDefaults.ts";

test("a run's full metric target overrides a server chart-name target", () => {
  const defaults = resolveDashboardDefaults(
    { targets: { loss: 0.05 } },
    [{ targets: { "val/loss": 0.1 } }],
    ["loss", "val/loss", "train/loss"],
  );

  assert.deepEqual(defaults.targets, { loss: 0.1, "val/loss": 0.1, "train/loss": 0.05 });
});

test("a run's chart-name target overrides a server full metric target", () => {
  const defaults = resolveDashboardDefaults(
    { targets: { "val/loss": 0.05 } },
    [{ targets: { loss: 0.1 } }],
    ["loss", "val/loss", "train/loss"],
  );

  assert.deepEqual(defaults.targets, { loss: 0.1, "val/loss": 0.1, "train/loss": 0.1 });
});

test("zero is a target override and unrelated targets retain server defaults", () => {
  const defaults = resolveDashboardDefaults(
    { targets: { loss: 0.05, accuracy: 0.95 } },
    [{ targets: { "val/loss": 0 } }],
    ["loss", "accuracy", "lr"],
  );

  assert.deepEqual(defaults.targets, { loss: 0, accuracy: 0.95 });
});

test("an exact chart key wins over aliases within the same layer", () => {
  const defaults = resolveDashboardDefaults(
    null,
    [{ targets: { loss: 0.5, "val/loss": 0.25 } }],
    ["loss", "val/loss", "train/loss"],
  );

  assert.deepEqual(defaults.targets, { loss: 0.5, "val/loss": 0.25, "train/loss": 0.5 });
});

test("the first selected run's target wins even when a later run uses an exact key", () => {
  const defaults = resolveDashboardDefaults(
    { targets: { loss: 0.05 } },
    [{ targets: { "val/loss": 0.1 } }, { targets: { loss: 0.2 } }],
    ["loss", "val/loss", "train/loss"],
  );

  assert.deepEqual(defaults.targets, { loss: 0.1, "val/loss": 0.1, "train/loss": 0.2 });
});

test("later runs supply targets missing from earlier runs", () => {
  const defaults = resolveDashboardDefaults(
    { targets: { accuracy: 0.8 } },
    [null, { targets: {} }, { targets: { "val/accuracy": 0.95 } }],
    ["accuracy", "val/accuracy"],
  );

  assert.deepEqual(defaults.targets, { accuracy: 0.95, "val/accuracy": 0.95 });
});

test("matching one grouped chart does not turn a full metric name into a broad alias", () => {
  const defaults = resolveDashboardDefaults(
    { targets: { "val/loss": 0.9 } },
    [{ targets: { "train/loss": 0.2 } }],
    ["loss", "train/loss", "val/loss"],
  );

  assert.deepEqual(defaults.targets, { loss: 0.2, "train/loss": 0.2, "val/loss": 0.9 });
});

test("unset run fields inherit server stars, order, and targets", () => {
  const defaults = resolveDashboardDefaults(
    { starred: ["val/accuracy"], order: ["accuracy", "loss"], targets: { loss: 0.05 } },
    [null, { targets: {} }],
    ["accuracy", "loss"],
  );

  assert.equal(isStarredByDefault(defaults, "accuracy"), true);
  assert.equal(defaultOrderRank(defaults, "loss"), 1);
  assert.deepEqual(defaults.targets, { loss: 0.05 });
});

test("explicit empty run lists replace server stars and order", () => {
  const defaults = resolveDashboardDefaults(
    { starred: ["loss"], order: ["loss"] },
    [{ starred: [], order: [] }],
    ["loss"],
  );

  assert.deepEqual(defaults, { starred: [], order: [], targets: {} });
});

test("comparison stars and order retain the selected runs' union", () => {
  const defaults = resolveDashboardDefaults(
    { starred: ["lr"], order: ["lr"] },
    [
      { starred: ["loss"], order: ["loss", "accuracy"] },
      { starred: ["accuracy", "loss"], order: ["accuracy", "lr"] },
    ],
    ["loss", "accuracy", "lr"],
  );

  assert.deepEqual(defaults.starred, ["loss", "accuracy"]);
  assert.deepEqual(defaults.order, ["loss", "accuracy", "lr"]);
});

test("no configuration produces an empty initial view", () => {
  assert.deepEqual(resolveDashboardDefaults(null, [null], ["loss"]), {
    starred: [], order: [], targets: {},
  });
});
