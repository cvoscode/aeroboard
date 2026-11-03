import { useMemo, useState } from 'react';
import type { RunSummary } from '../hooks/useAeroboard';
import type { Score, RunRecord } from '../hooks/useWorkspace';
import { improvement, scoreValue, type Reducer } from '../lib/comparison';
import { downloadText } from '../lib/export';

interface ComparisonTableProps {
  runs: RunSummary[];
  selectedHyperparameters: string[];
  selectedMetrics: string[];
  scores: Score[];
  reducer: Reducer;
  onReducerChange: (reducer: Reducer) => void;
  baseline: string;
  onBaselineChange: (run: string) => void;
  comparisonStep: number | null;
  onStepChange: (step: number | null) => void;
  records: RunRecord[];
  comparisonSegment: string;
  onSegmentChange: (value: string) => void;
}

function hyperValue(summary: RunSummary, hyperName: string): string {
  const value = summary.hyperparameters.find((entry) => entry.key === hyperName);
  return value ? value.value : '—';
}

export function ComparisonTable({ runs, selectedHyperparameters, selectedMetrics, scores, reducer, onReducerChange, baseline, onBaselineChange, comparisonStep, onStepChange, records, comparisonSegment, onSegmentChange }: ComparisonTableProps) {
  const [filterText, setFilterText] = useState('');
  const [sortColumn, setSortColumn] = useState<string>('run');
  const [sortDirection, setSortDirection] = useState<'asc' | 'desc'>('asc');
  const value = (run: RunSummary, key: string) => {
    const score = scores.find(score => score.run_id === run.run_id && score.name === key);
    return score ? scoreValue(score, reducer) : reducer === "last" ? run.metric_summaries.find(metric => metric.name === key)?.last ?? null : null;
  };

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

      const split = sortColumn.indexOf(':');
      const kind = sortColumn.slice(0,split), key = sortColumn.slice(split+1);
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

      const find = (run: RunSummary) => {
        const score = scores.find(score => score.run_id === run.run_id && score.name === key);
        return score ? scoreValue(score, reducer) : reducer === "last" ? run.metric_summaries.find(metric => metric.name === key)?.last ?? null : null;
      };
      const leftMetric = find(left);
      const rightMetric = find(right);
      if (leftMetric === null) return rightMetric === null ? 0 : 1;
      if (rightMetric === null) return -1;
      return (leftMetric - rightMetric) * direction;
    });

    return sorted;
  }, [filterText, runs, scores, reducer, sortColumn, sortDirection]);

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
          <select aria-label="Metric reducer" value={reducer} onChange={event => onReducerChange(event.target.value as Reducer)}>
            <option value="last">Last observation</option><option value="best">Best (defined goal)</option><option value="at_step">At training step</option>
          </select>
          {reducer === "at_step" ? <input type="number" step="any" aria-label="Comparison step" value={comparisonStep ?? ""} onChange={event => onStepChange(event.target.value === "" ? null : Number(event.target.value))} /> : null}
          {reducer === "at_step" ? <label>Segment<input aria-label="Comparison segment" value={comparisonSegment} onChange={event => onSegmentChange(event.target.value)} placeholder="Name, or __global__" /><small>Multiple segments need an explicit name.</small></label> : null}
          <select aria-label="Baseline run" value={baseline} onChange={event => onBaselineChange(event.target.value)}><option value="">No baseline</option>{runs.map(run => <option key={run.run_id} value={run.run_id}>{run.run_id}</option>)}</select>
          <button type="button" onClick={() => {
            const rows = [["Run", ...selectedHyperparameters, ...selectedMetrics.map(key => `${key} (${reducer})`)],
              ...filteredRuns.map(run => [run.run_id, ...selectedHyperparameters.map(key => hyperValue(run,key)), ...selectedMetrics.map(key => String(value(run,key) ?? ""))])];
            downloadText(rows.map(row => row.map(cell => `"${cell.replaceAll('"', '""')}"`).join(",")).join("\r\n"), "comparison.csv", "text/csv;charset=utf-8");
          }}>Export table CSV</button>
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
              <th>Run</th><th>Status</th>
              {selectedHyperparameters.map((key) => (
                <th key={`hp-${key}`}>{key}</th>
              ))}
              {selectedMetrics.map((key) => (
                <th key={`metric-${key}`}>{key} ({reducer === "at_step" ? `step ${comparisonStep ?? "?"}` : reducer})</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {filteredRuns.map((run) => (
              <tr key={run.run_id}>
                <td>{run.run_id}{baseline === run.run_id ? " · baseline" : ""}</td>
                <td>{records.find(record => record.data.run_id === run.run_id)?.data.display_status ?? "unknown"}</td>
                {selectedHyperparameters.map((key) => (
                  <td key={`hp-${run.run_id}-${key}`}>{hyperValue(run, key)}</td>
                ))}
                {selectedMetrics.map((key) => (
                  <td key={`metric-${run.run_id}-${key}`}>
                    {value(run,key)?.toPrecision(5) ?? "—"}
                    {(() => {
                      const reference = runs.find(run => run.run_id === baseline);
                      const score = scores.find(score => score.run_id === run.run_id && score.name === key);
                      const referenceScore = scores.find(score => score.run_id === baseline && score.name === key);
                      if (!reference || run.run_id === baseline || !score || !referenceScore || score.unit !== referenceScore.unit || score.goal !== referenceScore.goal) return null;
                      const delta = improvement(value(run,key), value(reference,key), score.goal);
                      return delta ? <small className={delta.absolute >= 0 ? "metric-improvement" : "metric-regression"}>{delta.absolute >= 0 ? "+" : ""}{delta.absolute.toPrecision(3)} improvement{delta.percent === null ? "" : ` (${delta.percent.toFixed(1)}%)`}</small> : null;
                    })()}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
