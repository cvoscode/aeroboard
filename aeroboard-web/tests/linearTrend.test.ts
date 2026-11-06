import assert from "node:assert/strict";
import test from "node:test";
import {fitLinearTrend} from "../src/lib/linearTrend.ts";

test("least-squares regression recovers the trend through noisy samples", () => {
  const fit = fitLinearTrend([-2, -1, 0, 1, 2], [-5, 1, 2, 3, 9]);
  assert.ok(fit);
  assert.equal(fit.slope, 3);
  assert.equal(fit.predict(0), 2);
  assert.equal(fit.predict(2), 8);
  assert.deepEqual([fit.minX, fit.maxX], [-2, 2]);
});

test("centering preserves small differences between large wall-clock timestamps", () => {
  const origin = 1_700_000_000;
  const x = Float64Array.from([0, 0.001, 0.002, 0.003], offset => origin + offset);
  const y = Float64Array.from(x, value => 2 + 3 * (value - origin));
  const fit = fitLinearTrend(x, y);
  assert.ok(fit);
  assert.ok(Math.abs(fit.slope - 3) < 1e-10);
  assert.ok(Math.abs(fit.predict(x[2]) - y[2]) < 1e-12);
});

test("invalid samples are ignored without turning gaps into zeros", () => {
  const fit = fitLinearTrend([0, NaN, 1, Infinity, 2, 3], [1, 10, 3, 0, NaN, 7]);
  assert.ok(fit);
  assert.equal(fit.slope, 2);
  assert.equal(fit.predict(2), 5);
});

test("window and outlier bounds restrict the fitted samples", () => {
  for (const bounds of [{xRange: [1, 2] as [number, number]}, {yRange: [0, 10] as [number, number]}]) {
    const fit = fitLinearTrend([0, 1, 2, 3], [100, 3, 5, -100], bounds);
    assert.ok(fit);
    assert.equal(fit.slope, 2);
    assert.equal(fit.predict(1.5), 4);
    assert.deepEqual([fit.minX, fit.maxX], [1, 2]);
  }
});

test("unsorted samples and repeated steps produce the same fit", () => {
  const fit = fitLinearTrend([2, 0, 1, 1], [5, 1, 3, 3]);
  assert.ok(fit);
  assert.equal(fit.slope, 2);
  assert.equal(fit.predict(0), 1);
});

test("flat zero data has a trend; insufficient or degenerate data does not", () => {
  const flat = fitLinearTrend([0, 1, 2], [0, 0, 0]);
  assert.ok(flat);
  assert.equal(flat.slope, 0);
  assert.equal(flat.predict(1), 0);
  assert.equal(fitLinearTrend([], []), null);
  assert.equal(fitLinearTrend([1], [2]), null);
  assert.equal(fitLinearTrend([1, 1], [2, 3]), null);
  assert.equal(fitLinearTrend([0, 1], [1, 2], {xRange: [5, 10]}), null);
  assert.equal(fitLinearTrend([0, 1], [NaN, Infinity]), null);
});
