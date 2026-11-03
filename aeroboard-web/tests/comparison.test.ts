import assert from "node:assert/strict";
import test from "node:test";
import {improvement, scoreValue} from "../src/lib/comparison.ts";
test("baseline deltas follow the metric objective and never divide by zero", () => {
  assert.deepEqual(improvement(0.1,0.2,"minimize"),{absolute:0.1,percent:50});
  assert.deepEqual(improvement(2,0,"maximize"),{absolute:2,percent:null});
  assert.equal(improvement(1,2,null),null);
  assert.equal(improvement(null,2,"maximize"),null);
});
test("missing scores remain missing", () => {
  assert.equal(scoreValue(undefined,"best"),null);
});
