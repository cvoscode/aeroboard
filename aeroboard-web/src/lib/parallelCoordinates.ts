import type { RunSummary } from "../hooks/aeroboard/types";
import type { Score } from "../hooks/useWorkspace";
import { metricValue, type Reducer } from "./comparison.ts";

export interface ParallelAxis {
  key: string;
  label: string;
  values: (number | null)[];
  min: number;
  max: number;
}

export function numericHyperparameter(value: string | undefined): number | null {
  if (!value?.trim()) return null;
  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

export function parallelAxes(runs: RunSummary[], hyperparameters: string[], metrics: string[], scores: Score[], reducer: Reducer): ParallelAxis[] {
  return [
    ...[...new Set(hyperparameters)].map(label => ({ key: `hp:${label}`, label,
      values: runs.map(run => numericHyperparameter(run.hyperparameters.find(entry => entry.key === label)?.value)) })),
    ...[...new Set(metrics)].map(label => ({ key: `metric:${label}`, label,
      values: runs.map(run => metricValue(run, label, scores, reducer)) })),
  ].flatMap(axis => {
    const values = axis.values.filter((value): value is number => value !== null);
    return values.length ? [{ ...axis, min: Math.min(...values), max: Math.max(...values) }] : [];
  });
}

/** Missing observations break the path; the remaining observations still render. */
export function parallelPoints(axes: ParallelAxis[], runIndex: number, spacing: number, top: number, plotHeight: number, left: number) {
  let connected = false;
  let path = "";
  const points: { x: number; y: number; value: number; axisIndex: number }[] = [];
  axes.forEach((axis, axisIndex) => {
    const value = axis.values[runIndex];
    if (value == null) {connected = false;return;}
    const span = axis.max - axis.min;
    const ratio = axis.min === axis.max ? 0.5 : Number.isFinite(span)
      ? (value - axis.min) / span : (value / 2 - axis.min / 2) / (axis.max / 2 - axis.min / 2);
    const x = left + axisIndex * spacing;
    const y = top + (1 - ratio) * plotHeight;
    points.push({ x, y, value, axisIndex });
    path += `${connected ? "L" : "M"} ${x} ${y} `;
    connected = true;
  });
  return { path: path.trim(), points };
}

export function formatAxisValue(value: number): string {
  const magnitude = Math.abs(value);
  return magnitude !== 0 && (magnitude < 0.001 || magnitude >= 100_000)
    ? value.toExponential(2) : Number(value.toPrecision(4)).toString();
}

export function axisLabelLines(label: string): string[] {
  const lines: string[] = [];
  let line = "";
  for (const part of label.split(/(?<=[/._-])/)) {
    if (line.length + part.length > 20 && line) {lines.push(line);line = "";}
    line += part;
    while (line.length > 20) {lines.push(line.slice(0, 20));line = line.slice(20);}
  }
  if (line) lines.push(line);
  return lines.length > 3 ? [...lines.slice(0, 2), `${lines[2].slice(0, 19)}…`] : lines;
}
