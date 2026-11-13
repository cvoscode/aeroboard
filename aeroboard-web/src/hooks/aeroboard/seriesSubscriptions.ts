import type { TimeSeriesData } from './types';
const versions = new WeakMap<TimeSeriesData, number>();
const listeners = new WeakMap<TimeSeriesData, Set<() => void>>();
const pending = new Set<TimeSeriesData>();
let scheduled = false;
export function seriesVersion(series: TimeSeriesData): number { return versions.get(series) ?? 0; }
export function subscribeSeries(series: TimeSeriesData, listener: () => void): () => void {
  let entries = listeners.get(series);
  if (!entries) { entries = new Set(); listeners.set(series, entries); }
  entries.add(listener);
  return () => entries.delete(listener);
}
export function changedSeries(series: TimeSeriesData) {
  versions.set(series, seriesVersion(series) + 1);
  pending.add(series);
  if (scheduled) return;
  scheduled = true;
  const schedule = typeof requestAnimationFrame === 'function' ? requestAnimationFrame : (cb: () => void) => queueMicrotask(cb);
  schedule(() => {
    scheduled = false;
    const changed = Array.from(pending); pending.clear();
    const callbacks = new Set(changed.flatMap(series => Array.from(listeners.get(series) ?? [])));
    for (const callback of callbacks) callback();
  });
}
