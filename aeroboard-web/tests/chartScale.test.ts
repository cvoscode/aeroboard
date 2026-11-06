import assert from "node:assert/strict";
import test from "node:test";
import {createExponentialTransform, logAxisTicks, parseChartScales} from "../src/lib/chartScale.ts";

test("exponential scaling expands small changes near the high end", () => {
  const transform = createExponentialTransform(0, 1);
  assert.equal(transform.fwd(0), 0);
  assert.equal(transform.fwd(1), 1);
  const upperChange = transform.fwd(1) - transform.fwd(0.99);
  const lowerChange = transform.fwd(0.01) - transform.fwd(0);
  assert.ok(upperChange > 3 * 0.01);
  assert.ok(lowerChange < 0.01);
});

test("the exponential axis round-trips original values, including zero, negatives, and outliers", () => {
  const transform = createExponentialTransform(-2, 3);
  const values = [-100, -2, -1, 0, 1, 2, 3, 100];
  for (const value of values) {
    const mapped = transform.fwd(value);
    assert.ok(Number.isFinite(mapped));
    assert.ok(Math.abs(transform.bwd(mapped) - value) < 1e-10);
  }
  const mapped = values.map(value => transform.fwd(value));
  assert.deepEqual(mapped.slice().sort((a, b) => a - b), mapped);
});

test("exponential scaling stays finite for large, tiny, and closely spaced values", () => {
  for (const [min, max] of [[1e200, 2e200], [1e-200, 2e-200], [1_000_000, 1_000_000.001], [-1e308, 1e308]]) {
    const transform = createExponentialTransform(min, max);
    const value = min / 2 + max / 2;
    assert.ok(Number.isFinite(transform.fwd(value)));
    assert.ok(Math.abs(transform.fwd(value) - createExponentialTransform(0, 1).fwd(0.5)) < 1e-5);
    const restored = transform.bwd(transform.fwd(value));
    assert.ok(Math.abs(restored - value) <= Math.max(Math.abs(value) * 1e-14, Number.MIN_VALUE));
  }
});

test("invalid or constant ranges safely use an identity transform", () => {
  for (const [min, max] of [[1, 1], [2, 1], [NaN, 1], [0, Infinity]]) {
    const transform = createExponentialTransform(min, max);
    assert.equal(transform.fwd(42), 42);
    assert.equal(transform.bwd(42), 42);
  }
});

test("saved chart scales accept supported modes and ignore malformed settings", () => {
  assert.deepEqual(parseChartScales({loss: "exponential", accuracy: "log", lr: "linear", bad: "other", old: true}),
    {loss: "exponential", accuracy: "log", lr: "linear"});
  for (const raw of [null, undefined, [], "log", 1]) assert.deepEqual(parseChartScales(raw), {});
});

test("log ticks stay readable when a zoom leaves no decade in view", () => {
  const close = (actual: number[], expected: number[]) => {
    assert.equal(actual.length, expected.length, String(actual));
    actual.forEach((tick, index) => assert.ok(Math.abs(tick - expected[index]) < 1e-9, String(actual)));
  };
  close(logAxisTicks(1.36, 1.56), [1.4, 1.45, 1.5, 1.55]);
  close(logAxisTicks(1, 8), [2, 4, 6, 8]);
  close(logAxisTicks(0.5, 20), [0.5, 1, 2, 5, 10, 20]);
  close(logAxisTicks(0.001, 100), [0.001, 0.01, 0.1, 1, 10, 100]);
});

test("log ticks stay bounded and handle unusable ranges", () => {
  assert.ok(logAxisTicks(1e-100, 1e100).length <= 12);
  assert.deepEqual(logAxisTicks(0, 1), []);
  assert.deepEqual(logAxisTicks(5, 5), []);
  assert.deepEqual(logAxisTicks(1, Infinity), []);
});
