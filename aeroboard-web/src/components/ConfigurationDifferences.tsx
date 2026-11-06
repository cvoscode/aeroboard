import type { RunSummary } from "../hooks/useAeroboard";
import type { RunRecord } from "../hooks/useWorkspace";
import { configurationChanges, parseConfiguration } from "../lib/configuration";
import { ConfigurationValue } from "./ConfigurationValue";
import { MetricName } from "./MetricName";

export function ConfigurationDifferences({ runs, baseline, records }: { runs: RunSummary[]; baseline: string; records: RunRecord[] }) {
  const reference = runs.find(run => run.run_id === baseline);
  if (!reference) return null;
  const parameters = (run: RunSummary) => Object.fromEntries(run.hyperparameters.map(entry => [entry.key, parseConfiguration(entry.value)]));
  const baseParameters = parameters(reference);
  const baseProvenance = records.find(record => record.data.run_id === baseline)?.data.provenance ?? {};

  return <div className="comparison-card configuration-differences">
    <div className="comparison-card-header"><div>
      <h3>Configuration differences</h3>
      <p>Baseline: {baseline}. Expand a run to inspect changed fields.</p>
    </div></div>
    {runs.filter(run => run.run_id !== baseline).map(run => {
      const provenance = records.find(record => record.data.run_id === run.run_id)?.data.provenance ?? {};
      const changes = [
        ...configurationChanges(baseParameters, parameters(run)),
        ...configurationChanges({ provenance: baseProvenance }, { provenance }),
      ];
      return <details key={run.run_id} className="configuration-run-diff">
        <summary><span><MetricName name={run.run_id} /></span><small>{changes.length} changed field{changes.length === 1 ? "" : "s"}</small></summary>
        {changes.length ? <div className="comparison-table-scroll" role="region" tabIndex={0} aria-label={`Configuration changes for ${run.run_id}`}>
          <table className="comparison-table configuration-diff-table">
            <thead><tr><th scope="col">Field</th><th scope="col">Baseline value</th><th scope="col">Run value</th></tr></thead>
            <tbody>{changes.map(change => <tr key={change.key}>
              <th scope="row"><MetricName name={change.key} /></th>
              <td><ConfigurationValue value={change.before} label={`${change.key} baseline value`} /></td>
              <td><ConfigurationValue value={change.after} label={`${change.key} run value`} /></td>
            </tr>)}</tbody>
          </table>
        </div> : <p className="comparison-scroll-hint">No configuration differences.</p>}
      </details>;
    })}
  </div>;
}
