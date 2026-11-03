import { useEffect, useRef, useState, type FormEvent } from "react";
import { Settings2, X } from "lucide-react";
import { workspaceRequest, type RunRecord, type RunEvent, type SavedView, type MetricDefinition, type useWorkspace } from "../hooks/useWorkspace";

type Workspace = ReturnType<typeof useWorkspace>;
interface Props {
  workspace: Workspace; projectId: string; viewId: string; selectedRuns: string[]; metrics: string[];
  settings: Record<string, unknown>;
  runBrowser: Record<string, unknown>;
  onRunBrowserChange: (settings: Record<string, unknown>) => void;
  onProjectChange: (id: string) => void;
  onViewChange: (view: SavedView | null) => void;
  onSelectRuns: (ids: string[]) => void;
  onEventsChange: (events: RunEvent[]) => void;
  onEventSelect: (event: RunEvent) => void;
  onImportPreferences: () => void;
  onReloadPreferences: () => void;
}

export function WorkspacePanel({workspace, projectId, viewId, selectedRuns, metrics, settings, runBrowser, onRunBrowserChange,
  onProjectChange, onViewChange, onSelectRuns, onEventsChange, onEventSelect, onImportPreferences, onReloadPreferences}: Props) {
  const settingsDialog = useRef<HTMLDialogElement>(null);
  const [name, setName] = useState("");
  const [projectName, setProjectName] = useState("");
  const search = typeof runBrowser.search === "string" ? runBrowser.search : "";
  const status = typeof runBrowser.status === "string" ? runBrowser.status : "";
  const includeArchived = runBrowser.includeArchived === true;
  const filters = Array.isArray(runBrowser.filters) ? runBrowser.filters : [];
  const [filterField, setFilterField] = useState("params.learning_rate");
  const [filterOp, setFilterOp] = useState("lt");
  const [filterValue, setFilterValue] = useState("0.01");
  const [matching, setMatching] = useState<{key: string; project: string; ids: string[]} | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [checked, setChecked] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const activeView = workspace.views.find(view => view.id === viewId);
  const [viewRevision, setViewRevision] = useState({id: "", revision: 0});
  if (activeView && viewRevision.id !== activeView.id) {
    setViewRevision({id: activeView.id, revision: activeView.revision});
    setName("");
  }
  const project = workspace.projects.find(project => project.id === projectId);
  const selected = workspace.records.find(run => run.data.run_id === selectedRuns[0]);
  const filterKey = JSON.stringify(filters);
  useEffect(() => {
    if (filterKey === "[]") return;
    const abort = new AbortController();
    const load = async () => {
      const matches: string[] = [];
      let offset: number | null = 0;
      while (offset !== null) {
        const params = new URLSearchParams({project: projectId, archived: "true", filters: filterKey, offset: String(offset), limit: "1000"});
        const page: {rows: RunRecord[]; next_offset: number | null} = await workspaceRequest(`/run-records?${params}`, "GET", undefined, abort.signal);
        matches.push(...page.rows.map(run => run.id)); offset = page.next_offset;
      }
      if (!abort.signal.aborted) setMatching({key: filterKey, project: projectId, ids: matches});
    };
    void load().catch(error => {if (!abort.signal.aborted) setMessage(String(error));});
    return () => abort.abort();
  }, [filterKey, projectId, workspace.updated]);
  const records = workspace.records.filter(run => run.parent === projectId && (includeArchived || !run.data.archived)
    && (filters.length === 0 || matching?.key === filterKey && matching.project === projectId && matching.ids.includes(run.id))
    && (!status || run.data.display_status === status)
    && (!search || JSON.stringify(run.data).toLowerCase().includes(search.toLowerCase())));
  const act = async (work: () => Promise<void>) => {
    setBusy(true); setMessage("");
    try {await work();} catch (error) {setMessage(String(error));} finally {setBusy(false);}
  };
  const save = (copy: boolean) => act(async () => {
    const title = name.trim() || activeView?.data.name;
    if (!title) throw new Error("Enter a view name.");
    const saved = await workspace.mutate<SavedView>(copy || !activeView ? `/projects/${encodeURIComponent(projectId)}/views` : `/views/${activeView.id}`,
      copy || !activeView ? "POST" : "PATCH", {name: title, settings, ...(copy || !activeView ? {} : {revision: viewRevision.revision})});
    setViewRevision({id: saved.id, revision: saved.revision});
    onViewChange(saved); setName(""); setMessage("View saved.");
  });
  const selectRuns = (ids: string[]) => {
    onSelectRuns(ids);
    settingsDialog.current?.close();
  };
  if (!workspace.capabilities) return workspace.error ? <div className="status-banner" role="status">Workspace unavailable: {workspace.error}</div> : null;
  return <nav className="workspace-navigation" aria-label="Project navigation">
      <label>Project<select aria-label="Project" value={projectId} onChange={event => {setViewRevision({id: "",revision: 0});setName("");onProjectChange(event.target.value);}}>
        {workspace.projects.map(project => <option key={project.id} value={project.id}>{project.data.name}</option>)}
      </select></label>
      <label>View<select aria-label="View" value={viewId} onChange={event => {
        const view = workspace.views.find(view => view.id === event.target.value) ?? null;
        setViewRevision({id: view?.id ?? "",revision: view?.revision ?? 0});setName("");onViewChange(view);
      }}>
        <option value="">Configured defaults</option>
        {workspace.views.map(view => <option key={view.id} value={view.id}>{view.data.name}{project?.data.default_view === view.id ? " (project default)" : ""}</option>)}
      </select></label>
      <button type="button" className="workspace-settings-toggle" aria-label="Project and view settings"
        title="Project and view settings" aria-haspopup="dialog" onClick={() => settingsDialog.current?.showModal()}>
        <Settings2 size={16} />
      </button>
    <dialog ref={settingsDialog} className="workspace-settings-dialog" aria-labelledby="workspace-settings-title"
      onClick={event => {if (event.target === event.currentTarget) event.currentTarget.close();}}>
    <section className="workspace-panel" aria-label="Project workspace">
      <div className="workspace-settings-heading">
        <div><h2 id="workspace-settings-title">Project & views</h2><p>{project?.data.name ?? projectId}</p></div>
        <button type="button" aria-label="Close project settings" onClick={() => settingsDialog.current?.close()}><X size={18} /></button>
      </div>
      <h3 className="workspace-settings-section-title">Saved view</h3>
    <div className="workspace-toolbar">
      <input aria-label="View name" placeholder="Name for saved view" value={name} onChange={event => setName(event.target.value)} />
      <button type="button" disabled={busy} onClick={() => void save(false)}>Save view</button>
      <button type="button" disabled={busy} onClick={() => void save(true)}>Save as new</button>
      {activeView && project ? <>
        <button type="button" disabled={busy} title="Reload the saved view and clear local layout overrides" onClick={() => void act(async () => {
          const latest = await workspaceRequest<SavedView>(`/views/${activeView.id}`);
          setViewRevision({id: latest.id,revision: latest.revision});setName("");onReloadPreferences();onViewChange(latest);workspace.refresh();setMessage("Latest saved view loaded.");
        })}>Reload view</button>
        <button type="button" disabled={busy} onClick={() => void act(async () => {
          await workspace.mutate(`/projects/${project.id}`, "PATCH", {revision: project.revision, default_view: activeView.id}); setMessage("Project default updated.");
        })}>Make default</button>
        <button type="button" disabled={busy} onClick={() => void act(async () => {
          await workspace.mutate(`/views/${activeView.id}?revision=${activeView.revision}`, "DELETE"); onViewChange(null);
        })}>Delete view</button>
      </> : null}
      <button type="button" onClick={() => void act(async () => {
        const url = new URL(window.location.href); url.searchParams.set("project", projectId);
        url.searchParams.set("view", viewId || "__none__");
        url.searchParams.set("runs", selectedRuns.join(","));
        await navigator.clipboard.writeText(url.toString()); setMessage("Link copied. It opens on this deployment.");
      })}>Copy view link</button>
    </div>
    {!workspace.capabilities.persistent ? <p className="workspace-hint">Live-only workspace: metadata and views disappear when the server restarts.</p> : null}
    {workspace.error || message ? <p role="status" className="workspace-message">{message || workspace.error}</p> : null}
    <details className="workspace-details">
      <summary>Runs and project settings · {records.length} {records.length === 1 ? "run" : "runs"}</summary>
      <div className="workspace-toolbar">
        <input aria-label="New project name" placeholder="New project ID" value={projectName} onChange={event => setProjectName(event.target.value)} />
        <button type="button" disabled={busy || !projectName.trim()} onClick={() => void act(async () => {
          const result = await workspace.mutate<{id: string}>("/projects", "POST", {id: projectName.trim(), name: projectName.trim()}); onProjectChange(result.id); setProjectName("");
        })}>Create project</button>
        {projectId === "default" ? <button type="button" onClick={onImportPreferences}>Import previous browser layout</button> : null}
      </div>
      <div className="workspace-toolbar">
        <input type="search" aria-label="Search run metadata" placeholder="Search tags, notes, provenance…" value={search} onChange={event => onRunBrowserChange({...runBrowser, search: event.target.value})} />
        <select aria-label="Run status filter" value={status} onChange={event => onRunBrowserChange({...runBrowser, status: event.target.value})}>
          <option value="">All statuses</option>{["running", "stale", "completed", "failed", "cancelled", "unknown"].map(status => <option key={status}>{status}</option>)}
        </select>
        <label className="workspace-checkbox"><input type="checkbox" checked={includeArchived} onChange={event => onRunBrowserChange({...runBrowser, includeArchived: event.target.checked})} />Include archived</label>
        <button type="button" disabled={checked.length === 0} onClick={() => selectRuns(checked)}>Compare selected ({checked.length})</button>
      </div>
      <form className="workspace-toolbar" onSubmit={event => {
        event.preventDefault(); let value: unknown = filterValue;
        try {value = JSON.parse(filterValue);} catch { /* Plain text is a string filter. */ }
        onRunBrowserChange({...runBrowser, filters: [...filters, {field: filterField, op: filterOp, value}]});
      }}>
        <label>Filter field<input required aria-label="Filter field" value={filterField} onChange={event => setFilterField(event.target.value)} placeholder="params.lr or metrics.val/loss.best" /></label>
        <select aria-label="Filter operator" value={filterOp} onChange={event => setFilterOp(event.target.value)}>{["eq","ne","lt","lte","gt","gte","contains","exists","missing"].map(op => <option key={op}>{op}</option>)}</select>
        <input aria-label="Filter value" value={filterValue} onChange={event => setFilterValue(event.target.value)} />
        <button disabled={filters.length >= 20}>Add filter</button>
        {filters.length ? <button type="button" onClick={() => onRunBrowserChange({...runBrowser, filters: []})}>Clear filters ({filters.length})</button> : null}
      </form>
      {filters.length ? <p className="workspace-hint">All filters must match: {filterKey}</p> : null}
      <div className="workspace-run-list"><table><thead><tr><th>Select</th><th>Run</th><th>Status</th><th>Started</th><th>Tags</th><th /></tr></thead><tbody>
        {records.map(run => <tr key={run.id}>
          <td><input type="checkbox" aria-label={`Select ${run.data.display_name}`} checked={checked.includes(run.data.run_id)} onChange={event => setChecked(previous => event.target.checked ? [...previous, run.data.run_id] : previous.filter(id => id !== run.data.run_id))} /></td>
          <td><button type="button" onClick={() => selectRuns([run.data.run_id])}>{run.data.display_name}</button><small>{run.data.run_id}</small></td>
          <td><span className={`run-status run-status-${run.data.display_status}`}>{run.data.display_status}</span></td>
          <td>{run.data.started_at_ms ? new Date(run.data.started_at_ms).toLocaleString() : "Unknown"}</td><td>{run.data.tags.join(", ")}</td>
          <td><button type="button" disabled={busy} onClick={() => void act(async () => {await workspace.mutate(`/run-records/${run.id}`, "PATCH", {revision: run.data.metadata_revision, archived: !run.data.archived});})}>{run.data.archived ? "Unarchive" : "Archive"}</button>
            {workspace.capabilities?.persistent ? deleting === run.id ? <><button type="button" disabled={busy} onClick={() => void act(async () => {await workspace.mutate(`/run-records/${run.id}?revision=${run.data.metadata_revision}`, "DELETE");setDeleting(null);setChecked(previous => previous.filter(id => id !== run.data.run_id));})}>Delete permanently</button><button type="button" onClick={() => setDeleting(null)}>Cancel</button></> : <button type="button" onClick={() => setDeleting(run.id)}>Delete run</button> : null}
          </td>
        </tr>)}
      </tbody></table></div>
    </details>
    {selected ? <RunDetails key={selected.id} run={selected} workspace={workspace} metrics={metrics}
      definitions={workspace.definitions} onEventsChange={onEventsChange} onEventSelect={event => {onEventSelect(event);settingsDialog.current?.close();}} /> : null}
    </section>
    </dialog>
  </nav>;
}

function RunDetails({run, workspace, metrics, definitions, onEventsChange, onEventSelect}: {
  run: RunRecord; workspace: Workspace; metrics: string[]; definitions: MetricDefinition[];
  onEventsChange: (events: RunEvent[]) => void; onEventSelect: (event: RunEvent) => void;
}) {
  const [displayName, setDisplayName] = useState(run.data.display_name);
  const [notes, setNotes] = useState(run.data.notes);
  const [tags, setTags] = useState(run.data.tags.join(", "));
  const [metadataRevision, setMetadataRevision] = useState(run.data.metadata_revision);
  const [metric, setMetric] = useState("");
  const [definitionParent, setDefinitionParent] = useState(run.id);
  const [goal, setGoal] = useState("");
  const [target, setTarget] = useState("");
  const [unit, setUnit] = useState("");
  const [eventLabel, setEventLabel] = useState("");
  const [eventStep, setEventStep] = useState("");
  const [editing, setEditing] = useState<RunEvent | null>(null);
  const [events, setEvents] = useState<RunEvent[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const abort = new AbortController();
    workspaceRequest<{rows: RunEvent[]}>(`/events?run_id=${run.id}&limit=1000`, "GET", undefined, abort.signal)
      .then(result => {if (!abort.signal.aborted) {setEvents(result.rows); onEventsChange(result.rows);}})
      .catch(error => {if (!abort.signal.aborted) setError(String(error));});
    return () => abort.abort();
  }, [run.id, workspace.updated, onEventsChange]);
  const submit = (work: () => Promise<void>) => async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError("");
    try {await work();} catch (error) {setError(String(error));} finally {setBusy(false);}
  };
  const loadDefinition = (name: string, parent = definitionParent) => {
    setMetric(name);
    const definition = definitions.find(d => d.parent === parent && d.data.name === name) ?? (parent === run.id ? definitions.find(d => d.parent === run.parent && d.data.name === name) : undefined);
    setGoal(definition?.data.goal ?? ""); setTarget(definition?.data.target == null ? "" : String(definition.data.target)); setUnit(definition?.data.unit ?? "");
  };
  return <details className="workspace-details">
    <summary><span className={`run-status run-status-${run.data.display_status}`}>{run.data.display_status}</span> {run.data.display_name} · attempt {run.data.attempt}</summary>
    <p className="workspace-hint">Resume ID: <code>{run.id}</code>{run.data.last_heartbeat_ms ? ` · Last heartbeat ${new Date(run.data.last_heartbeat_ms).toLocaleTimeString()}` : ""}</p>
    {run.data.started_at_ms ? <p className="workspace-hint">Duration: {Math.max(0, Math.round(((run.data.ended_at_ms ?? Date.now()) - run.data.started_at_ms) / 1000))} seconds</p> : null}
    {run.data.error ? <p role="status">{run.data.error}</p> : null}
    {run.data.delivery ? <p className="workspace-hint">Telemetry delivery: {run.data.delivery.status} · {run.data.delivery.committed_batches} committed batches{run.data.delivery.final_sequence !== null ? ` / ${run.data.delivery.final_sequence} expected` : ""}{run.data.delivery.dropped_rows ? ` · ${run.data.delivery.dropped_rows} dropped rows` : ""}</p> : null}
    <div className="workspace-columns">
      <form onSubmit={submit(async () => {const saved = await workspace.mutate<RunRecord>(`/run-records/${run.id}`, "PATCH", {revision: metadataRevision, display_name: displayName, notes, tags: tags.split(",").map(tag => tag.trim()).filter(Boolean)});setMetadataRevision(saved.data.metadata_revision);})}>
        <h3>Run metadata</h3>
        <label>Display name<input required value={displayName} onChange={event => setDisplayName(event.target.value)} /></label>
        <label>Tags, separated by commas<input value={tags} onChange={event => setTags(event.target.value)} /></label>
        <label>Notes<textarea value={notes} onChange={event => setNotes(event.target.value)} /></label>
        <button disabled={busy}>Save metadata</button>
        <button type="button" onClick={() => {setDisplayName(run.data.display_name);setNotes(run.data.notes);setTags(run.data.tags.join(", "));setMetadataRevision(run.data.metadata_revision);}}>Reload metadata</button>
        <details><summary>Captured provenance</summary><pre>{JSON.stringify(run.data.provenance, null, 2)}</pre></details>
      </form>
      <form onSubmit={submit(async () => {
        const previous = definitions.find(d => d.parent === definitionParent && d.data.name === metric);
        await workspace.mutate("/metric-definitions", "POST", {parent: definitionParent, name: metric, goal: goal || null, target: target.trim() ? Number(target) : null, unit: unit || null, ...(previous ? {revision: previous.revision} : {})});
      })}>
        <h3>Metric objective</h3>
        <label>Definition scope<select value={definitionParent} onChange={event => {setDefinitionParent(event.target.value);loadDefinition(metric,event.target.value);}}><option value={run.id}>This run</option><option value={run.parent}>Project default</option></select></label>
        <label>Full metric name<input required list="workspace-metrics" value={metric} onChange={event => loadDefinition(event.target.value)} /></label>
        <datalist id="workspace-metrics">{metrics.map(name => <option key={name} value={name} />)}</datalist>
        <label>Goal<select value={goal} onChange={event => setGoal(event.target.value)}><option value="">Unspecified</option><option value="minimize">Minimize</option><option value="maximize">Maximize</option></select></label>
        <label>Target<input type="number" step="any" value={target} onChange={event => setTarget(event.target.value)} /></label>
        <label>Unit<input placeholder="e.g. fraction, ms" value={unit} onChange={event => setUnit(event.target.value)} /></label>
        <p className="workspace-hint">Objectives rank runs. Chart target edits only change the display.</p><button disabled={busy}>Save objective</button>
      </form>
      <div><form onSubmit={submit(async () => {
        if (editing) await workspace.mutate(`/events/${editing.id}`, "PATCH", {revision: editing.revision, label: eventLabel});
        else await workspace.mutate("/events", "POST", {id: crypto.randomUUID(), run_uid: run.id, label: eventLabel,
          kind: "note", timestamp_ms: Date.now(), step: eventStep.trim() ? Number(eventStep) : null, details: {}, media_ids: []});
        setEventLabel("");setEditing(null);
      })}>
        <h3>Events and annotations</h3>
        <label>Observation<input required value={eventLabel} onChange={event => setEventLabel(event.target.value)} /></label>
        <label>Training step<input type="number" step="any" disabled={editing !== null} value={eventStep} onChange={event => setEventStep(event.target.value)} /></label>
        <button disabled={busy}>{editing ? "Save annotation" : "Add note"}</button>
        {editing ? <button type="button" onClick={() => {setEditing(null);setEventLabel("");}}>Cancel edit</button> : null}
      </form><ul className="workspace-events">{events.map(event => <li key={event.id}>
        <button type="button" onClick={() => onEventSelect(event)}>{event.data.step == null ? new Date(event.data.timestamp_ms).toLocaleTimeString() : `Step ${event.data.step}`} · {event.data.label}</button>
        {event.data.media_ids.map(id => <a key={id} href={`/api/assets/${encodeURIComponent(id)}/content`} target="_blank" rel="noreferrer">Open media</a>)}
        <button type="button" aria-label={`Edit annotation ${event.data.label}`} onClick={() => {setEditing(event);setEventLabel(event.data.label);setEventStep(event.data.step == null ? "" : String(event.data.step));}}>Edit</button>
        <button type="button" aria-label={`Delete annotation ${event.data.label}`} onClick={() => void workspace.mutate(`/events/${event.id}?revision=${event.revision}`, "DELETE").catch(error => setError(String(error)))}>×</button>
      </li>)}</ul></div>
    </div>
    {error ? <p role="status">{error}</p> : null}
  </details>;
}
