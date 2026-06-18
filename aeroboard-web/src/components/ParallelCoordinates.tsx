import type { RunSummary } from '../hooks/useAeroboard';

interface ParallelCoordinatesProps {
  runs: RunSummary[];
  selectedHyperparameters: string[];
  selectedMetrics: string[];
  runColors: Record<string, string>;
}

function parseNumeric(value: string): number | null {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

interface AxisDescriptor {
  key: string;
  label: string;
  getValue: (run: RunSummary) => number | null;
}

export function ParallelCoordinates({ runs, selectedHyperparameters, selectedMetrics, runColors }: ParallelCoordinatesProps) {
  const axes: AxisDescriptor[] = [
    ...selectedHyperparameters.map((key) => ({
      key: `hp:${key}`,
      label: key,
      getValue: (run: RunSummary) => {
        const hp = run.hyperparameters.find((entry) => entry.key === key);
        return hp ? parseNumeric(hp.value) : null;
      },
    })),
    ...selectedMetrics.map((key) => ({
      key: `metric:${key}`,
      label: key,
      getValue: (run: RunSummary) => run.metric_summaries.find((entry) => entry.name === key)?.last ?? null,
    })),
  ];

  const usableAxes = axes.filter((axis) => runs.some((run) => axis.getValue(run) !== null));

  if (runs.length === 0 || usableAxes.length < 2) {
    return (
      <div className="comparison-card empty-state-card">
        <h3>Parallel Coordinates</h3>
        <p>Choose at least two numeric hyperparameters or metrics to draw the comparison.</p>
      </div>
    );
  }

  const width = 900;
  const height = 360;
  const padding = { top: 28, right: 30, bottom: 42, left: 30 };
  const axisSpacing = usableAxes.length > 1 ? (width - padding.left - padding.right) / (usableAxes.length - 1) : 0;

  const ranges = usableAxes.map((axis) => {
    const values = runs.map((run) => axis.getValue(run)).filter((value): value is number => value !== null);
    if (values.length === 0) {
      return { min: 0, max: 1 };
    }
    const min = Math.min(...values);
    const max = Math.max(...values);
    return { min, max: max === min ? max + 1 : max };
  });

  const lines = runs.map((run) => {
    const points = usableAxes.map((axis, index) => {
      const value = axis.getValue(run);
      if (value === null) {
        return null;
      }
      const range = ranges[index];
      const ratio = (value - range.min) / (range.max - range.min);
      const x = padding.left + index * axisSpacing;
      const y = padding.top + (1 - ratio) * (height - padding.top - padding.bottom);
      return { x, y, value };
    });

    if (points.some((point) => point === null)) {
      return null;
    }

    return {
      run,
      path: points.map((point, index) => `${index === 0 ? 'M' : 'L'} ${(point as { x: number; y: number }).x} ${(point as { x: number; y: number }).y}`).join(' '),
      points: points as { x: number; y: number; value: number }[],
    };
  }).filter((line): line is { run: RunSummary; path: string; points: { x: number; y: number; value: number }[] } => line !== null);

  return (
    <div className="comparison-card">
      <div className="comparison-card-header">
        <div>
          <h3>Parallel Coordinates</h3>
          <p>Selected numeric hyperparameters + metrics</p>
        </div>
      </div>
      <div className="parallel-svg-wrap">
        <svg viewBox={`0 0 ${width} ${height}`} className="parallel-svg" role="img" aria-label="Parallel coordinates chart">
          {usableAxes.map((axis, index) => {
            const x = padding.left + index * axisSpacing;
            const range = ranges[index];
            return (
              <g key={axis.key}>
                <line x1={x} y1={padding.top} x2={x} y2={height - padding.bottom} className="parallel-axis" />
                <text x={x} y={height - 12} textAnchor="middle" className="parallel-axis-label">{axis.label}</text>
                <text x={x} y={padding.top - 8} textAnchor="middle" className="parallel-axis-range">{range.max.toFixed(3)}</text>
                <text x={x} y={height - padding.bottom + 14} textAnchor="middle" className="parallel-axis-range">{range.min.toFixed(3)}</text>
              </g>
            );
          })}

          {lines.map(({ run, path, points }) => (
            <g key={run.run_id}>
              <path d={path} fill="none" stroke={runColors[run.run_id] ?? '#00d2ff'} strokeWidth="2.2" strokeOpacity="0.85" />
              {points.map((point, index) => (
                <circle key={`${run.run_id}-${index}`} cx={point.x} cy={point.y} r="3.5" fill={runColors[run.run_id] ?? '#00d2ff'}>
                  <title>{`${run.run_id} • ${usableAxes[index].label}: ${point.value.toFixed(4)}`}</title>
                </circle>
              ))}
            </g>
          ))}
        </svg>
      </div>
    </div>
  );
}
