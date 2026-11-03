import { useEffect, useState } from "react";
import { workspaceRequest } from "../hooks/useWorkspace";

interface Point {x: number; mean: number; stddev: number | null; n: number}
interface Group {label: string; runs: string[]; points: Point[]}
export function AggregateComparison({runIds, metrics}: {runIds: string[]; metrics: string[]}) {
  const [metric, setMetric] = useState("");
  const [groupBy, setGroupBy] = useState("");
  const [includeUnfinished, setIncludeUnfinished] = useState(false);
  const [alignment, setAlignment] = useState("step");
  const [groups, setGroups] = useState<Group[]>([]);
  const [excluded, setExcluded] = useState(0);
  const [error, setError] = useState("");
  const key = runIds.join(",");
  const activeMetric = metrics.includes(metric) ? metric : metrics[0] ?? "";
  useEffect(() => {
    if (!key || !activeMetric) return;
    const abort = new AbortController();
    const timer = setTimeout(() => {
      workspaceRequest<{groups: Group[]; excluded_runs: number}>("/comparison/aggregate", "POST", {run_ids: key.split(","), metric: activeMetric,
        group_by: groupBy.split(",").map(key => key.trim()).filter(Boolean), alignment, include_unfinished: includeUnfinished}, abort.signal)
        .then(response => {if (!abort.signal.aborted) {setGroups(response.groups);setExcluded(response.excluded_runs);setError("");}})
        .catch(error => {if (!abort.signal.aborted) setError(String(error));});
    },200);
    return () => {clearTimeout(timer);abort.abort();};
  },[key, activeMetric, groupBy, alignment, includeUnfinished]);
  if (!activeMetric) return null;
  return <div className="comparison-card"><h3>Repeated experiment comparison</h3>
    <div className="workspace-toolbar">
      <label>Metric<select value={activeMetric} onChange={event => setMetric(event.target.value)}>{metrics.map(metric => <option key={metric}>{metric}</option>)}</select></label>
      <label>Group by<input value={groupBy} placeholder="Automatic configuration groups" onChange={event => setGroupBy(event.target.value)} /></label>
      <label>Align by<select value={alignment} onChange={event => setAlignment(event.target.value)}><option value="step">Training step</option><option value="rel">Relative time</option><option value="wall">Wall time</option></select></label>
      <label className="workspace-checkbox"><input type="checkbox" checked={includeUnfinished} onChange={event => setIncludeUnfinished(event.target.checked)} />Include unfinished runs</label>
    </div>
    <p className="workspace-hint">Mean ± sample standard deviation · exact positions, no extrapolation · {excluded} runs excluded. The band is hidden when fewer than two runs contribute.</p>
    {error ? <p role="status">{error}</p> : groups.map(group => <GroupChart key={group.label} group={group} />)}
  </div>;
}
function GroupChart({group}: {group: Group}) {
  const [highlight, setHighlight] = useState<Point | null>(null);
  const points = group.points;
  if (!points.length) return <p>{group.label}: no observations at this alignment.</p>;
  const minX = Math.min(...points.map(p => p.x)), maxX = Math.max(...points.map(p => p.x));
  const minY = Math.min(...points.map(p => p.mean - (p.stddev ?? 0))), maxY = Math.max(...points.map(p => p.mean + (p.stddev ?? 0)));
  const x = (value: number) => 50 + (value-minX)/(maxX-minX || 1)*700;
  const y = (value: number) => 200 - (value-minY)/(maxY-minY || 1)*170;
  const line = points.map(p => `${x(p.x)},${y(p.mean)}`).join(" ");
  // Separate segments prevent a variance band from spanning n=1 or missing positions.
  const segments: Point[][] = []; let segment: Point[] = [];
  for (const point of points) {if (point.stddev === null) {if(segment.length) segments.push(segment);segment=[];} else segment.push(point);}
  if(segment.length) segments.push(segment);
  return <div className="aggregate-chart"><h4>{group.label}</h4>
    <svg viewBox="0 0 800 245" role="img" aria-label={`Mean and standard deviation for ${group.runs.length} runs`} onPointerMove={event => {
      const box = event.currentTarget.getBoundingClientRect(); const position = (event.clientX-box.left)/box.width*800;
      setHighlight(points.reduce((best,p) => Math.abs(x(p.x)-position)<Math.abs(x(best.x)-position)?p:best,points[0]));
    }}>
      <line x1="50" y1="200" x2="750" y2="200" stroke="currentColor" /><line x1="50" y1="30" x2="50" y2="200" stroke="currentColor" />
      {segments.map((segment,index) => <polygon key={index} fill="var(--accent-primary, #00d2ff)" opacity="0.2" points={[...segment.map(p => `${x(p.x)},${y(p.mean + p.stddev!)}`),...[...segment].reverse().map(p => `${x(p.x)},${y(p.mean - p.stddev!)}`)].join(" ")} />)}
      <polyline fill="none" stroke="var(--accent-primary, #00d2ff)" strokeWidth="2" points={line} />
      <text x="50" y="225" fill="currentColor" fontSize="12">{minX.toPrecision(4)}</text><text x="700" y="225" fill="currentColor" fontSize="12">{maxX.toPrecision(4)}</text>
      <text x="2" y="38" fill="currentColor" fontSize="11">{maxY.toPrecision(3)}</text><text x="2" y="200" fill="currentColor" fontSize="11">{minY.toPrecision(3)}</text>
      {highlight ? <circle cx={x(highlight.x)} cy={y(highlight.mean)} r="4" fill="var(--accent-primary, #00d2ff)" /> : null}
    </svg>
    <p>{highlight ? `Position ${highlight.x}: mean ${highlight.mean.toPrecision(5)} · SD ${highlight.stddev?.toPrecision(4) ?? "unavailable"} · n=${highlight.n}` : `${group.runs.length} runs · hover to inspect contributor counts`}</p>
  </div>;
}
