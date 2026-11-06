import { useState } from "react";
import { createRoot } from "react-dom/client";
import { ComparisonTable } from "../../src/components/ComparisonTable";
import type { RunSummary } from "../../src/hooks/aeroboard/types";
import type { RunRecord, Score } from "../../src/hooks/useWorkspace";
import type { Reducer } from "../../src/lib/comparison";
import "../../src/index.css";
import "../../src/App.css";
import "../../src/workspace.css";

const keys = ["config", ...Array.from({ length: 99 }, (_, index) => `parameter_${index}`)];
const metrics = ["val/loss"];
const scores: Score[] = [];
const records: RunRecord[] = [];
const runs: RunSummary[] = Array.from({ length: 5000 }, (_, index) => ({
  run_id: `run-${String(index).padStart(5, "0")}`,
  hyperparameters: keys.map((key, column) => ({ key,
    value: column === 0 ? "{'dim': 384, 'dropout': 0.1, 'checkpointing': True}" : String((index + 1) * column),
    value_type: column === 0 ? "string" : "int" })),
  metric_summaries: [{ name: "val/loss", last: index / 5000, min: 0, max: 1 }],
}));

export default function GridFixture() {
  const [reducer, setReducer] = useState<Reducer>("last");
  const [baseline, setBaseline] = useState("");
  const [step, setStep] = useState<number | null>(null);
  const [segment, setSegment] = useState("");
  return <main style={{ padding: 24 }}><ComparisonTable runs={runs} selectedHyperparameters={keys} selectedMetrics={metrics}
    scores={scores} reducer={reducer} onReducerChange={setReducer} baseline={baseline} onBaselineChange={setBaseline}
    comparisonStep={step} onStepChange={setStep} records={records} comparisonSegment={segment} onSegmentChange={setSegment} /></main>;
}

createRoot(document.getElementById("root")!).render(<GridFixture />);
