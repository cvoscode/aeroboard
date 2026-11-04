import { MetricName } from "./MetricName";
import type { Score } from "../hooks/useWorkspace";

interface ObjectiveOverviewProps {
  scores: Score[];
  /** Name the run on each tile; a single run is already named in the page header. */
  showRun: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function targetStatus(score: Score) {
  if (score.target === null) return null;
  const reached = score.time_to_target_ms !== null;
  return (
    <span
      className={reached ? "metric-improvement" : undefined}
      title={reached ? `Target reached after ${(score.time_to_target_ms! / 1000).toFixed(1)}s` : "Target not reached"}
    >
      {reached ? "Reached" : "Not reached"}
    </span>
  );
}

export function ObjectiveOverview({ scores, showRun, open, onOpenChange }: ObjectiveOverviewProps) {
  const tracked = scores.filter(score => score.goal);
  if (!tracked.length) return null;

  const targeted = tracked.filter(score => score.target !== null);
  const reached = targeted.filter(score => score.time_to_target_ms !== null).length;

  return (
    <details
      className="objective-overview"
      open={open}
      onToggle={event => {
        const next = event.currentTarget.open;
        if (next !== open) onOpenChange(next);
      }}
    >
      <summary>
        Metric objectives
        <span>{tracked.length} tracked{targeted.length ? ` · ${reached} of ${targeted.length} targets reached` : ""}</span>
      </summary>
      <div className="scorecard-grid">
        {tracked.map(score => (
          <div className="metric-scorecard" key={`${score.run_id}:${score.name}`}>
            <div className="metric-scorecard-heading">
              <span title={score.name}><MetricName name={score.name} /></span>
              <small title={score.goal === "minimize" ? "Lower is better" : "Higher is better"}>{score.goal === "minimize" ? "MIN" : "MAX"}</small>
            </div>
            {showRun ? <small className="scorecard-run" title={score.run_id}>{score.run_id}</small> : null}
            <div className="scorecard-value">
              <strong>{score.best?.toPrecision(5) ?? "—"}{score.unit ? ` ${score.unit}` : ""}</strong>
              {targetStatus(score)}
            </div>
            <span>Best at step {score.best_step ?? "unknown"} · last {score.last?.toPrecision(5) ?? "non-finite / missing"}</span>
          </div>
        ))}
      </div>
    </details>
  );
}
