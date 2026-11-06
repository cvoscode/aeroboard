import type { Score } from "../hooks/useWorkspace";
import type { RunSummary } from "../hooks/aeroboard/types";
export type Reducer = "last" | "best" | "at_step";
export function scoreValue(score: Score | undefined, reducer: Reducer): number | null {
  const value = score?.[reducer];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}
/** All analysis views use the same reducer, including its missing-value semantics. */
export function metricValue(run: RunSummary, key: string, scores: Score[], reducer: Reducer): number | null {
  const score = scores.find(score => score.run_id === run.run_id && score.name === key);
  if (score) return scoreValue(score, reducer);
  const value = reducer === "last" ? run.metric_summaries.find(metric => metric.name === key)?.last : null;
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}
export function improvement(value: number | null, baseline: number | null, goal: Score["goal"]): {absolute: number; percent: number | null} | null {
  if (value === null || baseline === null || !goal) return null;
  const absolute = goal === "minimize" ? baseline - value : value - baseline;
  if (!Number.isFinite(absolute)) return null;
  const percent = baseline === 0 ? null : absolute / Math.abs(baseline) * 100;
  return {absolute, percent: percent !== null && Number.isFinite(percent) ? percent : null};
}
