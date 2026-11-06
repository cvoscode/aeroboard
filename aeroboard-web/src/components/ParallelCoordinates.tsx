import { useEffect, useRef, useState } from "react";
import { RUN_COLORS } from "../lib/runColors";
import type { RunSummary } from "../hooks/useAeroboard";
import type { Score } from "../hooks/useWorkspace";
import type { Reducer } from "../lib/comparison";
import { axisLabelLines, formatAxisValue, parallelAxes, parallelPoints } from "../lib/parallelCoordinates";

interface ParallelCoordinatesProps {
  runs: RunSummary[];
  selectedHyperparameters: string[];
  selectedMetrics: string[];
  runColors: Record<string, string>;
  scores: Score[];
  reducer: Reducer;
}

export function ParallelCoordinates({ runs, selectedHyperparameters, selectedMetrics, runColors, scores, reducer }: ParallelCoordinatesProps) {
  const container = useRef<HTMLDivElement>(null);
  const [availableWidth, setAvailableWidth] = useState(640);
  const axes = parallelAxes(runs, selectedHyperparameters, selectedMetrics, scores, reducer);
  const hasChart = runs.length > 0 && axes.length >= 2;
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const observer = new ResizeObserver(entries => setAvailableWidth(entries[0].contentRect.width));
    observer.observe(element);
    return () => observer.disconnect();
  }, [hasChart]);

  if (!hasChart) {
    return <div className="comparison-card empty-state-card">
      <h3>Parallel coordinates</h3>
      <p>Choose at least two numeric hyperparameters or metrics with observations for the selected reducer.</p>
    </div>;
  }

  const height = 400;
  const padding = { top: 36, right: 72, bottom: 84, left: 72 };
  const width = Math.max(availableWidth, 640, (axes.length - 1) * 160 + padding.left + padding.right);
  const spacing = (width - padding.left - padding.right) / (axes.length - 1);
  const plotHeight = height - padding.top - padding.bottom;
  const lines = runs.map((run, index) => ({ run, ...parallelPoints(axes, index, spacing, padding.top, plotHeight, padding.left) }));

  return (
    <div className="comparison-card parallel-card">
      <div className="comparison-card-header">
        <div>
          <h3>Parallel coordinates</h3>
          <p>{axes.length} numeric axes · {reducer === "at_step" ? "At training step" : reducer === "best" ? "Best (defined goal)" : "Last observation"}. Gaps indicate missing values.</p>
        </div>
      </div>
      <p className="comparison-scroll-hint">Scroll horizontally to explore all axes. Hover over a point for its run and value.</p>
      <div ref={container} className="parallel-svg-wrap" tabIndex={0} role="region" aria-label="Scrollable parallel coordinates">
        <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} className="parallel-svg" role="img" aria-label="Parallel coordinates chart">
          {axes.map((axis, index) => {
            const x = padding.left + index * spacing;
            return <g key={axis.key} className="parallel-axis-group" data-axis={axis.key}>
              <title>{axis.label}</title>
              <line x1={x} y1={padding.top} x2={x} y2={height - padding.bottom} className="parallel-axis" />
              <text x={x} y={height - padding.bottom + 40} textAnchor="middle" className="parallel-axis-label">
                {axisLabelLines(axis.label).map((line, row) => <tspan key={row} x={x} dy={row ? 14 : 0}>{line}</tspan>)}
              </text>
              {axis.min === axis.max ? (
                <text x={x} y={padding.top - 12} textAnchor="middle" className="parallel-axis-range">{formatAxisValue(axis.min)} (constant)</text>
              ) : <>
                <text x={x} y={padding.top - 12} textAnchor="middle" className="parallel-axis-range">{formatAxisValue(axis.max)}</text>
                <text x={x} y={height - padding.bottom + 18} textAnchor="middle" className="parallel-axis-range">{formatAxisValue(axis.min)}</text>
              </>}
            </g>;
          })}
          {lines.map(({ run, path, points }) => <g key={run.run_id} className="parallel-run" data-run={run.run_id}>
            <path d={path} fill="none" stroke={runColors[run.run_id] ?? RUN_COLORS[0]} strokeWidth="2.2" strokeOpacity="0.8" />
            {points.map(point => <circle key={point.axisIndex} cx={point.x} cy={point.y} r="4" fill={runColors[run.run_id] ?? RUN_COLORS[0]}>
              <title>{`${run.run_id} • ${axes[point.axisIndex].label}: ${point.value}`}</title>
            </circle>)}
          </g>)}
        </svg>
      </div>
      <ul className="parallel-legend" aria-label="Parallel coordinates runs">
        {lines.map(({ run, points }) => <li key={run.run_id}>
          <span className="legend-dot" style={{ backgroundColor: runColors[run.run_id] ?? RUN_COLORS[0] }} />
          <span>{run.run_id}</span><small>{points.length}/{axes.length} values</small>
        </li>)}
      </ul>
    </div>
  );
}
