import { ComparisonTable } from './ComparisonTable';
import { ParallelCoordinates } from './ParallelCoordinates';
import type { RunSummary } from '../hooks/useAeroboard';
import type { Score, RunRecord } from '../hooks/useWorkspace';
import type { Reducer } from '../lib/comparison';
import { AggregateComparison } from './AggregateComparison';

interface ComparePanelsProps {
  runs: RunSummary[];
  selectedHyperparameters: string[];
  selectedMetrics: string[];
  runColors: Record<string, string>;
  scores: Score[];
  reducer: Reducer;
  onReducerChange: (value: Reducer) => void;
  baseline: string;
  onBaselineChange: (value: string) => void;
  comparisonStep: number | null;
  onStepChange: (value: number | null) => void;
  records: RunRecord[];
  comparisonSegment: string;
  onSegmentChange: (value: string) => void;
}

export default function ComparePanels({ runs, selectedHyperparameters, selectedMetrics, runColors, scores, reducer, onReducerChange, baseline, onBaselineChange, comparisonStep, onStepChange, records, comparisonSegment, onSegmentChange }: ComparePanelsProps) {
  const reference = runs.find(run => run.run_id === baseline);
  return (
    <div className="compare-stack">
      <ComparisonTable
        runs={runs}
        selectedHyperparameters={selectedHyperparameters}
        selectedMetrics={selectedMetrics}
        scores={scores} reducer={reducer} onReducerChange={onReducerChange}
        baseline={baseline} onBaselineChange={onBaselineChange}
        comparisonStep={comparisonStep} onStepChange={onStepChange} records={records}
        comparisonSegment={comparisonSegment} onSegmentChange={onSegmentChange}
      />
      {reference ? <div className="comparison-card"><h3>Configuration differences from {baseline}</h3>
        {runs.filter(run => run.run_id !== baseline).map(run => {
          const keys = Array.from(new Set([...reference.hyperparameters, ...run.hyperparameters].map(parameter => parameter.key)));
          const changed = keys.filter(key => reference.hyperparameters.find(p => p.key === key)?.value !== run.hyperparameters.find(p => p.key === key)?.value);
          const baseProvenance = records.find(record => record.data.run_id === baseline)?.data.provenance ?? {};
          const provenance = records.find(record => record.data.run_id === run.run_id)?.data.provenance ?? {};
          const changedProvenance = Array.from(new Set([...Object.keys(baseProvenance), ...Object.keys(provenance)])).filter(key => JSON.stringify(baseProvenance[key]) !== JSON.stringify(provenance[key]));
          return <div key={run.run_id}><strong>{run.run_id}</strong><ul>{changed.map(key => <li key={key}>{key}: {reference.hyperparameters.find(p => p.key === key)?.value ?? "missing"} → {run.hyperparameters.find(p => p.key === key)?.value ?? "missing"}</li>)}{changedProvenance.map(key => <li key={`provenance:${key}`}>Provenance {key}: {JSON.stringify(baseProvenance[key]) ?? "missing"} → {JSON.stringify(provenance[key]) ?? "missing"}</li>)}</ul>{changed.length + changedProvenance.length === 0 ? <p>No configuration differences.</p> : null}</div>;
        })}
      </div> : null}
      <AggregateComparison runIds={runs.map(run => run.run_id)} metrics={selectedMetrics} />
      <ParallelCoordinates
        runs={runs}
        selectedHyperparameters={selectedHyperparameters}
        selectedMetrics={selectedMetrics}
        runColors={runColors}
      />
    </div>
  );
}
