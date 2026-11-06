import { useRef, useState, type FormEvent } from "react";
import { Layers, Search, X } from "lucide-react";

import { MetricName } from "./MetricName";
import type { MetricGroup } from "../lib/metricGroups";

interface MetricGroupsDialogProps {
  groups: MetricGroup[];
  metrics: string[];
  onSave: (group: MetricGroup) => void;
  onDelete: (id: string) => void;
}

export function MetricGroupsDialog({ groups, metrics, onSave, onDelete }: MetricGroupsDialogProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [editingId, setEditingId] = useState("");
  const [name, setName] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");

  // Keep members from other runs available when editing a group.
  const options = [...new Set([...metrics, ...groups.flatMap(group => group.metrics)])].sort();
  const matches = options.filter(metric => metric.toLowerCase().includes(query.trim().toLowerCase()));
  const allSelected = matches.length > 0 && matches.every(metric => selected.includes(metric));
  const duplicateName = groups.some(group => group.id !== editingId && group.name.toLowerCase() === name.trim().toLowerCase());
  const canSave = !!name.trim() && selected.length > 0 && !duplicateName;

  const edit = (id: string) => {
    const group = groups.find(group => group.id === id);
    setEditingId(group?.id ?? "");
    setName(group?.name ?? "");
    setSelected(group?.metrics ?? []);
    setQuery("");
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!canSave) return;
    onSave({ id: editingId || crypto.randomUUID(), name: name.trim(), metrics: selected,
      hidden: groups.find(group => group.id === editingId)?.hidden ?? false });
    dialog.current?.close();
  };

  return (
    <>
      <button type="button" aria-haspopup="dialog" onClick={() => {edit("");dialog.current?.showModal();}}>
        <Layers size={14} aria-hidden="true" /> Metric groups{groups.length ? ` (${groups.length})` : ""}
      </button>
      <dialog ref={dialog} className="workspace-settings-dialog hidden-charts-dialog metric-groups-dialog"
        aria-labelledby="metric-groups-title"
        onClick={event => {if (event.target === event.currentTarget) event.currentTarget.close();}}>
        <form className="workspace-panel" onSubmit={submit}>
          <div className="workspace-settings-heading">
            <div>
              <h2 id="metric-groups-title">Metric groups</h2>
              <p>Group related charts to hide and move them together.</p>
            </div>
            <button type="button" aria-label="Close metric groups" onClick={() => dialog.current?.close()}><X size={18} /></button>
          </div>
          {groups.length ? (
            <label>Edit metric group
              <select aria-label="Edit metric group" value={editingId} onChange={event => edit(event.target.value)}>
                <option value="">New group</option>
                {groups.map(group => <option key={group.id} value={group.id}>{group.name}</option>)}
              </select>
            </label>
          ) : null}
          <label>Group name
            <input value={name} onChange={event => setName(event.target.value)} placeholder="e.g. Losses or Metrics"
              required autoFocus aria-invalid={duplicateName} aria-describedby={duplicateName ? "metric-group-name-error" : undefined} />
          </label>
          {duplicateName ? <p id="metric-group-name-error" role="status">A group with this name already exists.</p> : null}
          <div className="metric-search">
            <Search size={15} aria-hidden="true" />
            <input type="search" aria-label="Filter metrics for group" placeholder="Find charts to include…" value={query} onChange={event => setQuery(event.target.value)} />
          </div>
          <p className="metric-group-hint">Each chart belongs to one group. Selecting a chart moves it from its current group.</p>
          <ul className="hidden-charts-list">
            {matches.map(metric => {
              const owner = groups.find(group => group.id !== editingId && group.metrics.includes(metric));
              return (
                <li key={metric}>
                  <label className="hidden-charts-option">
                    <input type="checkbox" aria-label={`Include ${metric}`} checked={selected.includes(metric)}
                      onChange={() => setSelected(previous => previous.includes(metric) ? previous.filter(key => key !== metric) : [...previous, metric])} />
                    <span className="hidden-charts-name"><MetricName name={metric} /></span>
                    {owner ? <small>In {owner.name}</small> : !metrics.includes(metric) ? <small>Not in selected runs</small> : null}
                  </label>
                </li>
              );
            })}
            {matches.length ? null : <li className="hidden-charts-empty">No metrics match.</li>}
          </ul>
          <div className="hidden-charts-footer">
            <label className="hidden-charts-option">
              <input type="checkbox" checked={allSelected} disabled={!matches.length} onChange={() => setSelected(previous =>
                allSelected ? previous.filter(key => !matches.includes(key)) : [...new Set([...previous, ...matches])])} />
              Select all
            </label>
            <span className="metric-group-hint">{selected.length} selected</span>
            <div>
              {editingId ? <button type="button" onClick={() => {onDelete(editingId);dialog.current?.close();}}>Ungroup charts</button> : null}
              <button type="button" onClick={() => dialog.current?.close()}>Cancel</button>
              <button type="submit" className="hidden-charts-apply" disabled={!canSave}>{editingId ? "Save group" : "Create group"}</button>
            </div>
          </div>
        </form>
      </dialog>
    </>
  );
}
