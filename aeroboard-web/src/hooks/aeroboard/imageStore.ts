import type { ImageFrame, ImageSeries } from "./types";

export type ImageStore = Record<string, Record<string, ImageSeries>>;

export function upsertImageFrame(imageStore: ImageStore, frame: ImageFrame) {
  if (!imageStore[frame.run_id]) imageStore[frame.run_id] = {};
  if (!imageStore[frame.run_id][frame.name]) {
    imageStore[frame.run_id][frame.name] = { run_id: frame.run_id, name: frame.name, frames: [] };
  }

  const series = imageStore[frame.run_id][frame.name];
  const existingIndex = series.frames.findIndex((candidate) => candidate.ts_ms === frame.ts_ms);
  if (existingIndex >= 0) {
    const existing = series.frames[existingIndex];
    series.frames[existingIndex] = {
      ...existing,
      ...frame,
      image_base64: frame.image_base64 ?? existing.image_base64,
    };
  } else {
    series.frames.push(frame);
  }
  series.frames.sort((a, b) => a.ts_ms - b.ts_ms);
}

export function getImageSeriesForRun(imageStore: ImageStore, runId: string): ImageSeries[] {
  return Object.values(imageStore[runId] ?? {}).sort((a, b) => a.name.localeCompare(b.name));
}

export function getImageSeries(imageStore: ImageStore, runId: string, name: string): ImageSeries | null {
  return imageStore[runId]?.[name] ?? null;
}
