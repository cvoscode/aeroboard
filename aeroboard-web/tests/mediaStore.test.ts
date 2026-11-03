import assert from "node:assert/strict";
import test from "node:test";
import {upsertImageFrame, type ImageStore} from "../src/hooks/aeroboard/imageStore.ts";
import {upsertMediaClip, type MediaStore} from "../src/hooks/aeroboard/mediaStore.ts";

test("same-millisecond assets remain distinct and replay merges by identity", () => {
  const images: ImageStore = {}, media: MediaStore = {};
  const frame = {run_id: "run", name: "sample", ts_ms: 10, format: "png", step: 0};
  for (const asset_id of ["first", "second", "first"]) {
    upsertImageFrame(images, {...frame, asset_id});
    upsertMediaClip(media, {...frame, asset_id, kind: "video", format: "mp4"});
  }
  assert.deepEqual(images.run.sample.frames.map(frame => frame.asset_id), ["first", "second"]);
  assert.deepEqual(media.run["video:sample"].clips.map(clip => clip.asset_id), ["first", "second"]);
  // Old history continues to deduplicate using its timestamp-only identity.
  upsertImageFrame(images, frame);upsertImageFrame(images, frame);
  assert.equal(images.run.sample.frames.length, 3);
});
