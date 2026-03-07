import { ComparisonTable } from './ComparisonTable';
import { ParallelCoordinates } from './ParallelCoordinates';
import type { RunSummary } from '../hooks/useAeroboard';

interface ComparePanelsProps {
  runs: RunSummary[];
  selectedHyperparameters: string[];
  selectedMetrics: string[];
  runColors: Record<string, string>;
}

export default function ComparePanels({ runs, selectedHyperparameters, selectedMetrics, runColors }: ComparePanelsProps) {
  return (
    <div className="compare-stack">
      <ComparisonTable
        runs={runs}
        selectedHyperparameters={selectedHyperparameters}
        selectedMetrics={selectedMetrics}
      />
      <ParallelCoordinates
        runs={runs}
        selectedHyperparameters={selectedHyperparameters}
        selectedMetrics={selectedMetrics}
        runColors={runColors}
      />
    </div>
  );
}
