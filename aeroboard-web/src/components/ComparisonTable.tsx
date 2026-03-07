import { useMemo, useState } from 'react';
import type { RunSummary } from '../hooks/useAeroboard';

interface ComparisonTableProps {
  runs: RunSummary[];
  selectedHyperparameters: string[];
  selectedMetrics: string[];
}

function metricValue(summary: RunSummary, metricName: string): string {
  const metric = summary.metric_summaries.find((entry) => entry.name === metricName);
  return metric ? metric.last.toFixed(4) : '—';
}

function hyperValue(summary: RunSummary, hyperName: string): string {
  const value = summary.hyperparameters.find((entry) => entry.key === hyperName);
  return value ? value.value : '—';
}

export function ComparisonTable({ runs, selectedHyperparameters, selectedMetrics }: ComparisonTableProps) {
  const [filterText, setFilterText] = useState('');
  const [sortColumn, setSortColumn] = useState<string>('run');
  const [sortDirection, setSortDirection] = useState<'asc' | 'desc'>('asc');

  const filteredRuns = useMemo(() => {
    const query = filterText.trim().toLowerCase();
    const visible = runs.filter((run) => {
      if (!query) return true;
      const haystack = [
        run.run_id,
        ...run.hyperparameters.map((entry) => `${entry.key}:${entry.value}`),
        ...run.metric_summaries.map((entry) => `${entry.name}:${entry.last}`),
      ].join(' ').toLowerCase();
      return haystack.includes(query);
    });

    const sorted = [...visible].sort((left, right) => {
      const direction = sortDirection === 'asc' ? 1 : -1;
      if (sortColumn === 'run') {
        return left.run_id.localeCompare(right.run_id) * direction;
      }

      const [kind, key] = sortColumn.split(':', 2);
      if (kind === 'hp') {
        const leftValue = left.hyperparameters.find((entry) => entry.key === key)?.value ?? '';
        const rightValue = right.hyperparameters.find((entry) => entry.key === key)?.value ?? '';
        const leftNum = Number(leftValue);
        const rightNum = Number(rightValue);
        if (Number.isFinite(leftNum) && Number.isFinite(rightNum)) {
          return (leftNum - rightNum) * direction;
        }
        return leftValue.localeCompare(rightValue) * direction;
      }

      const leftMetric = left.metric_summaries.find((entry) => entry.name === key)?.last ?? Number.NEGATIVE_INFINITY;
      const rightMetric = right.metric_summaries.find((entry) => entry.name === key)?.last ?? Number.NEGATIVE_INFINITY;
      return (leftMetric - rightMetric) * direction;
    });

    return sorted;
  }, [filterText, runs, sortColumn, sortDirection]);

  if (runs.length === 0 || (selectedHyperparameters.length === 0 && selectedMetrics.length === 0)) {
    return (
      <div className="comparison-card empty-state-card">
        <h3>Comparison Table</h3>
        <p>Select runs plus at least one hyperparameter or metric to compare.</p>
      </div>
    );
  }

  return (
    <div className="comparison-card">
      <div className="comparison-card-header">
        <div>
          <h3>Comparison Table</h3>
          <p>{filteredRuns.length} / {runs.length} run{runs.length === 1 ? '' : 's'} visible</p>
        </div>
        <div className="comparison-table-controls">
          <input
            className="comparison-filter-input"
            type="search"
            placeholder="Filter runs, hparams, metrics"
            value={filterText}
            onChange={(event) => setFilterText(event.target.value)}
          />
          <select
            className="comparison-sort-select"
            value={sortColumn}
            onChange={(event) => setSortColumn(event.target.value)}
          >
            <option value="run">Run</option>
            {selectedHyperparameters.map((key) => (
              <option key={`sort-hp-${key}`} value={`hp:${key}`}>{`Hyperparameter: ${key}`}</option>
            ))}
            {selectedMetrics.map((key) => (
              <option key={`sort-metric-${key}`} value={`metric:${key}`}>{`Metric: ${key}`}</option>
            ))}
          </select>
          <button className="chip-btn" type="button" onClick={() => setSortDirection((value) => value === 'asc' ? 'desc' : 'asc')}>
            {sortDirection === 'asc' ? 'Asc ↑' : 'Desc ↓'}
          </button>
        </div>
      </div>
      <div className="comparison-table-scroll">
        <table className="comparison-table">
          <thead>
            <tr>
              <th>Run</th>
              {selectedHyperparameters.map((key) => (
                <th key={`hp-${key}`}>{key}</th>
              ))}
              {selectedMetrics.map((key) => (
                <th key={`metric-${key}`}>{key} (last)</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {filteredRuns.map((run) => (
              <tr key={run.run_id}>
                <td>{run.run_id}</td>
                {selectedHyperparameters.map((key) => (
                  <td key={`hp-${run.run_id}-${key}`}>{hyperValue(run, key)}</td>
                ))}
                {selectedMetrics.map((key) => (
                  <td key={`metric-${run.run_id}-${key}`}>{metricValue(run, key)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
