import type { MediaClip, MediaKind, MediaSeries } from "./types";

/** run_id -> `${kind}:${name}` -> series */
export type MediaStore = Record<string, Record<string, MediaSeries>>;

export function upsertMediaClip(store: MediaStore, clip: MediaClip) {
  const runSeries = (store[clip.run_id] ??= {});
  const key = `${clip.kind}:${clip.name}`;
  const series = (runSeries[key] ??= { kind: clip.kind, run_id: clip.run_id, name: clip.name, clips: [] });
  const clips = series.clips;

  // Fast path: live clips arrive in timestamp order.
  const last = clips[clips.length - 1];
  if (!last || last.ts_ms < clip.ts_ms) {
    clips.push(clip);
    return;
  }

  const existingIndex = clips.findIndex((candidate) => candidate.ts_ms === clip.ts_ms);
  if (existingIndex >= 0) {
    const existing = clips[existingIndex];
    if (existing.objectUrl && clip.objectUrl && existing.objectUrl !== clip.objectUrl) {
      URL.revokeObjectURL(existing.objectUrl);
    }
    clips[existingIndex] = { ...existing, ...clip, objectUrl: clip.objectUrl ?? existing.objectUrl };
    return;
  }

  clips.push(clip);
  clips.sort((a, b) => a.ts_ms - b.ts_ms);
}

export function getMediaSeriesForRun(store: MediaStore, runId: string): MediaSeries[] {
  return Object.values(store[runId] ?? {}).sort(
    (a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name),
  );
}

export function revokeMediaStore(store: MediaStore) {
  for (const runSeries of Object.values(store)) {
    for (const series of Object.values(runSeries)) {
      for (const clip of series.clips) {
        if (clip.objectUrl) URL.revokeObjectURL(clip.objectUrl);
      }
    }
  }
}

export const MEDIA_KINDS: MediaKind[] = ["audio", "video"];
