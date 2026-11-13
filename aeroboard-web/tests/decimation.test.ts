import test from 'node:test';
import assert from 'node:assert/strict';
import {minMaxIndices} from '../src/lib/decimation.ts';
test('decimation preserves spikes, troughs, gaps, and endpoints',()=>{
 const values=Array(10000).fill(1);values[1234]=100;values[7654]=-100;values[5555]=NaN;
 const selected=minMaxIndices(values.length,i=>values[i],200);
 for(const index of [0,1234,7654,5555,9999])assert.ok(selected.includes(index));
 assert.ok(selected.length<=300);
 assert.deepEqual(selected,[...selected].sort((a,b)=>a-b));
});
