import { useRef, useState } from "react";
import { Search, X } from "lucide-react";

import { MetricName } from "./MetricName";

interface HiddenChartsDialogProps {
  hidden: string[];
  /** Charts the selected runs can draw. Others are labelled, since showing them changes nothing yet. */
  available: string[];
  onShow: (keys: string[]) => void;
}

/** Filtering only earns its space once the list no longer fits at a glance. */
const FILTER_THRESHOLD = 8;

export function HiddenChartsDialog({ hidden, available, onShow }: HiddenChartsDialogProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");

  const needle = query.trim().toLowerCase();
  const matches = hidden.filter(key => key.toLowerCase().includes(needle));
  const allSelected = matches.length > 0 && matches.every(key => selected.includes(key));

  const toggle = (key: string) => setSelected(previous =>
    previous.includes(key) ? previous.filter(entry => entry !== key) : [...previous, key]);
  const toggleAll = () => setSelected(previous =>
    allSelected ? previous.filter(key => !matches.includes(key)) : [...new Set([...previous, ...matches])]);

  return (
    <>
      <button type="button" aria-haspopup="dialog" onClick={() => dialog.current?.showModal()}>
        Show hidden charts ({hidden.length})
      </button>
      <dialog
        ref={dialog}
        className="workspace-settings-dialog hidden-charts-dialog"
        aria-labelledby="hidden-charts-title"
        onClick={event => {if (event.target === event.currentTarget) event.currentTarget.close();}}
        onClose={() => {setSelected([]);setQuery("");}}
      >
        <section className="workspace-panel">
          <div className="workspace-settings-heading">
            <div>
              <h2 id="hidden-charts-title">Hidden charts</h2>
              <p>Choose the charts to bring back.</p>
            </div>
            <button type="button" aria-label="Close hidden charts" onClick={() => dialog.current?.close()}><X size={18} /></button>
          </div>

          {hidden.length > FILTER_THRESHOLD ? (
            <div className="metric-search">
              <Search size={15} aria-hidden="true" />
              <input type="search" aria-label="Filter hidden charts" placeholder="Filter hidden charts…" value={query} onChange={event => setQuery(event.target.value)} />
            </div>
          ) : null}

          <ul className="hidden-charts-list">
            {matches.map(key => (
              <li key={key}>
                <label className="hidden-charts-option">
                  <input type="checkbox" aria-label={key} checked={selected.includes(key)} onChange={() => toggle(key)} />
                  <span className="hidden-charts-name"><MetricName name={key} /></span>
                  {available.includes(key) ? null : <small>Not in selected runs</small>}
                </label>
              </li>
            ))}
            {matches.length ? null : <li className="hidden-charts-empty">No hidden charts match.</li>}
          </ul>

          <div className="hidden-charts-footer">
            <label className="hidden-charts-option">
              <input type="checkbox" checked={allSelected} disabled={!matches.length} onChange={toggleAll} />
              Select all
            </label>
            <div>
              <button type="button" onClick={() => dialog.current?.close()}>Cancel</button>
              <button
                type="button"
                className="hidden-charts-apply"
                disabled={!selected.length}
                onClick={() => {dialog.current?.close();onShow(selected);}}
              >
                Show selected{selected.length ? ` (${selected.length})` : ""}
              </button>
            </div>
          </div>
        </section>
      </dialog>
    </>
  );
}
