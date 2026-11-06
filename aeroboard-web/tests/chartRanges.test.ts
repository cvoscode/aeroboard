import assert from "node:assert/strict";
import test from "node:test";

import { getFocusedRanges, pointsInWindow } from "../src/lib/chartRanges.ts";

const raw = (x: number[], y: number[]) => ({x, raw: y, smooth: []});
const curve = raw([0, 100, 200, 300, 400], [10, 5, 2, 1.5, 1.4]);

const near = (actual: number, expected: number) => assert.ok(Math.abs(actual - expected) < 1e-9, `${actual} !== ${expected}`);

test("without a zoom the y-range covers all the data, padded by 8%", () => {
  const {focusedYRange} = getFocusedRanges([curve], false);
  near(focusedYRange![0], 1.4 - 8.6 * 0.08);
  near(focusedYRange![1], 10 + 8.6 * 0.08);
});

test("zooming fits the y-range to the visible part of the line", () => {
  const {focusedYRange, defaultXRange} = getFocusedRanges([curve], false, null, false, [300, 400]);
  near(focusedYRange![0], 1.4 - 0.1 * 0.08);
  near(focusedYRange![1], 1.5 + 0.1 * 0.08);
  assert.deepEqual(defaultXRange, [0, 400], "The full extent is still known for resetting");
});

test("a line entering the view from outside counts at the edge, so it isn't clipped", () => {
  assert.deepEqual(pointsInWindow([{x: 0, y: 10}, {x: 100, y: 20}, {x: 200, y: 0}], [50, 150]).sort((a, b) => a.x - b.x),
    [{x: 50, y: 15}, {x: 100, y: 20}, {x: 150, y: 10}]);
  // No sample falls inside this window, but the line still crosses it.
  const {focusedYRange} = getFocusedRanges([raw([0, 200], [0, 100])], false, null, false, [50, 150]);
  near(focusedYRange![0], 25 - 50 * 0.08);
  near(focusedYRange![1], 75 + 50 * 0.08);
});

test("log axes interpolate along the straight line they actually draw", () => {
  const [point] = pointsInWindow([{x: 0, y: 1}, {x: 100, y: 100}], [50, 60], true);
  near(point.y, 10);
});

test("a target outside the zoomed view does not stretch the axis, but does when unzoomed", () => {
  assert.ok(getFocusedRanges([curve], false, 0.5).focusedYRange![0] < 0.5);
  const zoomed = getFocusedRanges([curve], false, 0.5, false, [300, 400]).focusedYRange!;
  assert.ok(zoomed[0] > 1.3, `Lower bound ${zoomed[0]} must follow the data`);
});

test("a view covering all the data counts as unzoomed", () => {
  assert.deepEqual(getFocusedRanges([curve], false, 0.5, false, [0, 400]), getFocusedRanges([curve], false, 0.5));
  assert.deepEqual(getFocusedRanges([curve], false, 0.5, false, [-50, 500]), getFocusedRanges([curve], false, 0.5));
});

test("zooming fits raw lines as well as the smoothed line, across every series", () => {
  const smoothed = {x: [0, 1, 2], raw: [1, 9, 2], smooth: [1, 3, 2.5]};
  assert.ok(getFocusedRanges([smoothed], false).focusedYRange![1] < 9, "Unzoomed behavior is unchanged");
  assert.ok(getFocusedRanges([smoothed], false, null, false, [0.5, 1.5]).focusedYRange![1] > 9);
  const second = raw([0, 1, 2], [40, 50, 60]);
  assert.ok(getFocusedRanges([smoothed, second], false, null, false, [0.5, 1.5]).focusedYRange![1] > 50);
});

test("an outlier trim applies inside the zoomed view", () => {
  const x = Array.from({length: 21}, (_, i) => i);
  const y = x.map(i => (i === 15 ? 1000 : i));
  const zoomed = getFocusedRanges([raw(x, y)], true, null, false, [10, 20]).focusedYRange!;
  assert.ok(zoomed[1] < 100, `Upper bound ${zoomed[1]} must ignore the spike`);
});

test("a zoom on missing data falls back to the whole line", () => {
  assert.deepEqual(getFocusedRanges([curve], false, null, false, [900, 950]).focusedYRange, getFocusedRanges([curve], false).focusedYRange);
  const lonely = raw([0, 100], [NaN, 4]);
  assert.ok(getFocusedRanges([lonely], false, null, false, [10, 20]).focusedYRange);
});

test("a flat line in view still gets a usable range", () => {
  const {focusedYRange} = getFocusedRanges([raw([0, 1, 2, 3], [9, 9, 5, 5])], false, null, false, [2, 3]);
  assert.ok(focusedYRange![0] < 5 && focusedYRange![1] > 5);
});
