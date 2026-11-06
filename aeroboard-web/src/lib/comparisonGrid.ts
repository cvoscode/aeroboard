import type { RunSummary } from "../hooks/aeroboard/types";
import type { RunRecord, Score } from "../hooks/useWorkspace";
import { improvement, scoreValue, type Reducer } from "./comparison.ts";
import { numericHyperparameter } from "./parallelCoordinates.ts";

export interface ComparisonGridRow {
  runId: string;
  status: string;
  baseline: boolean;
  hyperparameters: Map<string, string>;
  numericHyperparameters: Map<string, number | null>;
  metrics: Map<string, number | null>;
  improvements: Map<string, { absolute: number; percent: number | null }>;
  searchText: string;
}

/** Index once per data update so rendering, filtering and sorting never scan a run's entries. */
export function comparisonRows(runs: RunSummary[], selectedMetrics: string[], scores: Score[], reducer: Reducer, baseline: string, records: RunRecord[]): ComparisonGridRow[] {
  const scoreIndex = new Map<string, Map<string, Score>>();
  for (const score of scores) {
    if (!scoreIndex.has(score.run_id)) scoreIndex.set(score.run_id, new Map());
    scoreIndex.get(score.run_id)!.set(score.name, score);
  }
  const statuses = new Map(records.map(record => [record.data.run_id, record.data.display_status]));
  const prepared = runs.map(run => {
    const hyperparameters = new Map(run.hyperparameters.map(entry => [entry.key, entry.value]));
    const summaries = new Map(run.metric_summaries.map(entry => [entry.name, entry.last]));
    const metrics = new Map(selectedMetrics.map(key => {
      const score = scoreIndex.get(run.run_id)?.get(key);
      const value = score ? scoreValue(score, reducer) : reducer === "last" ? summaries.get(key) : null;
      return [key, typeof value === "number" && Number.isFinite(value) ? value : null] as const;
    }));
    return { runId: run.run_id, status: statuses.get(run.run_id) ?? "unknown", baseline: run.run_id === baseline,
      hyperparameters, numericHyperparameters: new Map([...hyperparameters].map(([key, raw]) => [key, numericHyperparameter(raw)])),
      metrics, improvements: new Map<string, { absolute: number; percent: number | null }>(),
      searchText: [run.run_id, statuses.get(run.run_id) ?? "unknown",
        ...[...hyperparameters].map(([key, value]) => `${key}:${value}`),
        ...[...summaries].map(([key, value]) => `${key}:${value}`),
        ...[...metrics].map(([key, value]) => `${key}:${value ?? ""}`)].join(" "),
    };
  });
  const reference = prepared.find(row => row.baseline);
  if (reference) for (const row of prepared) {
    if (row.baseline) continue;
    for (const key of selectedMetrics) {
      const score = scoreIndex.get(row.runId)?.get(key);
      const referenceScore = scoreIndex.get(baseline)?.get(key);
      if (!score || !referenceScore || score.unit !== referenceScore.unit || score.goal !== referenceScore.goal) continue;
      const delta = improvement(row.metrics.get(key) ?? null, reference.metrics.get(key) ?? null, score.goal);
      if (delta) row.improvements.set(key, delta);
    }
  }
  return prepared;
}

export function isNumericColumn(rows: ComparisonGridRow[], key: string): boolean {
  let numeric = false;
  for (const row of rows) {
    const value = row.hyperparameters.get(key);
    if (value == null || !value.trim()) continue;
    if (row.numericHyperparameters.get(key) == null) return false;
    numeric = true;
  }
  return numeric;
}

/** AG Grid reverses the comparator for descending order; missing values stay last either way. */
export function compareGridValues(left: string | number | null | undefined, right: string | number | null | undefined, descending: boolean): number {
  if (left == null) return right == null ? 0 : descending ? -1 : 1;
  if (right == null) return descending ? 1 : -1;
  return typeof left === "number" && typeof right === "number" ? left - right : String(left).localeCompare(String(right));
}
