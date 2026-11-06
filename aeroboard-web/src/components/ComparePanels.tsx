import { ComparisonTable } from './ComparisonTable';
import { ParallelCoordinates } from './ParallelCoordinates';
import type { RunSummary } from '../hooks/useAeroboard';
import type { Score, RunRecord } from '../hooks/useWorkspace';
import type { Reducer } from '../lib/comparison';
import { AggregateComparison } from './AggregateComparison';
import { ConfigurationDifferences } from './ConfigurationDifferences';

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
      <ConfigurationDifferences runs={runs} baseline={baseline} records={records} />
      <AggregateComparison runIds={runs.map(run => run.run_id)} metrics={selectedMetrics} />
      <ParallelCoordinates
        runs={runs}
        selectedHyperparameters={selectedHyperparameters}
        selectedMetrics={selectedMetrics}
        runColors={runColors}
        scores={scores}
        reducer={reducer}
      />
    </div>
  );
}
