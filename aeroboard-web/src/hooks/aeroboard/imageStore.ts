import type { ImageFrame, ImageSeries } from "./types";

export type ImageStore = Record<string, Record<string, ImageSeries>>;

export function upsertImageFrame(imageStore: ImageStore, frame: ImageFrame) {
  const runSeries = (imageStore[frame.run_id] ??= {});
  const series = (runSeries[frame.name] ??= { run_id: frame.run_id, name: frame.name, frames: [] });
  const frames = series.frames;

  // Fast path: live frames arrive in timestamp order.
  const last = frames[frames.length - 1];
  if (!last || last.ts_ms < frame.ts_ms) {
    frames.push(frame);
    return;
  }

  const existingIndex = frames.findIndex((candidate) => frame.asset_id ? candidate.asset_id === frame.asset_id : !candidate.asset_id && candidate.ts_ms === frame.ts_ms);
  if (existingIndex >= 0) {
    const existing = frames[existingIndex];
    if (existing.objectUrl && frame.objectUrl && existing.objectUrl !== frame.objectUrl) {
      URL.revokeObjectURL(existing.objectUrl);
    }
    frames[existingIndex] = { ...existing, ...frame, objectUrl: frame.objectUrl ?? existing.objectUrl };
    return;
  }

  frames.push(frame);
  frames.sort((a, b) => a.ts_ms - b.ts_ms);
}

export function getImageSeriesForRun(imageStore: ImageStore, runId: string): ImageSeries[] {
  return Object.values(imageStore[runId] ?? {}).sort((a, b) => a.name.localeCompare(b.name));
}

export function getImageSeries(imageStore: ImageStore, runId: string, name: string): ImageSeries | null {
  return imageStore[runId]?.[name] ?? null;
}

/** Release the Blob URLs held by live frames. */
export function revokeImageStore(imageStore: ImageStore) {
  for (const runSeries of Object.values(imageStore)) {
    for (const series of Object.values(runSeries)) {
      for (const frame of series.frames) {
        if (frame.objectUrl) URL.revokeObjectURL(frame.objectUrl);
      }
    }
  }
}
