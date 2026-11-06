import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, Moon, Search, Sun, Trash2 } from "lucide-react";

import { ColorPicker } from "./ColorPicker";
import { formatAge } from "../lib/format";
import { RUN_COLORS } from "../lib/runColors";
import { runLabelInGroup, type ExperimentGroup, type RunEntry } from "../lib/runGroups";

interface DashboardSidebarProps {
  runStatuses: Record<string, string>;
  sidebarOpen: boolean;
  onClose: () => void;
  theme: "light" | "dark";
  onThemeToggle: () => void;
  nextTheme: "light" | "dark";
  effectiveActiveTab: "scalars" | "images" | "media" | "analysis";
  mode: "single" | "compare";
  onChangeMode: (mode: "single" | "compare") => void;
  runs: string[];
  isDeletingRuns: boolean;
  onDeleteAllRuns: () => void;
  canDeleteRuns: boolean;
  onDeleteRun: (runId: string) => void;
  onDeleteGroup: (experimentName: string) => void;
  runFilter: string;
  onRunFilterChange: (value: string) => void;
  filteredExperimentGroups: ExperimentGroup[];
  effectiveSelectedRuns: string[];
  collapsedExperiments: Record<string, boolean>;
  onToggleExperimentCollapsed: (name: string) => void;
  onToggleRun: (runId: string) => void;
  mergedRunColors: Record<string, string>;
  onRunColorChange: (runId: string, color: string) => void;
}

/** Keeps "12m"-style ages honest while the page sits open. */
function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}

function RunAge({ startedAtMs, now }: { startedAtMs: number; now: number }) {
  if (!startedAtMs) return null;
  const started = new Date(startedAtMs);
  return <time className="run-age" dateTime={started.toISOString()} title={`Started ${started.toLocaleString()}`}>{formatAge(startedAtMs, now)}</time>;
}

interface RunRowProps {
  run: RunEntry;
  label: string;
  status?: string;
  now: number;
  selected: boolean;
  color: string;
  comparing: boolean;
  canDelete: boolean;
  isDeleting: boolean;
  onToggle: () => void;
  onColorChange: (color: string) => void;
  onDelete: () => void;
}

function RunRow({ run, label, status, now, selected, color, comparing, canDelete, isDeleting, onToggle, onColorChange, onDelete }: RunRowProps) {
  const showColor = comparing && selected;
  return (
    <div className={`run-card-wrapper ${selected ? "active" : ""}`}>
      <button type="button" className="run-card-btn" onClick={onToggle} title={run.fullRunId} aria-label={run.fullRunId} aria-pressed={selected}>
        {/* Color only appears where it keys a line on the charts. */}
        <span className="run-card-indicator" style={showColor ? { backgroundColor: color } : undefined} />
        <span className="run-card-copy">
          <span className="run-name">{label}</span>
          {/* Finished runs are the normal case, so only exceptions earn a label. */}
          {status && status !== "completed" ? <small className={`run-status run-status-${status}`}>{status}</small> : null}
        </span>
        <RunAge startedAtMs={run.startedAtMs} now={now} />
      </button>
      {showColor ? <ColorPicker color={color} onChange={onColorChange} /> : null}
      {canDelete ? (
        <button type="button" className="run-delete-btn" disabled={isDeleting} onClick={onDelete}
          aria-label={`Delete run ${run.fullRunId}`} title="Delete this run">
          <Trash2 size={14} />
        </button>
      ) : null}
    </div>
  );
}

export function DashboardSidebar({
  runStatuses,
  sidebarOpen,
  onClose,
  theme,
  onThemeToggle,
  nextTheme,
  effectiveActiveTab,
  mode,
  onChangeMode,
  runs,
  isDeletingRuns,
  onDeleteAllRuns,
  canDeleteRuns,
  onDeleteRun,
  onDeleteGroup,
  runFilter,
  onRunFilterChange,
  filteredExperimentGroups,
  effectiveSelectedRuns,
  collapsedExperiments,
  onToggleExperimentCollapsed,
  onToggleRun,
  mergedRunColors,
  onRunColorChange,
}: DashboardSidebarProps) {
  const now = useNow();
  const renderRun = (run: RunEntry, label: string) => (
    <RunRow
      key={run.fullRunId}
      run={run}
      label={label}
      status={runStatuses[run.fullRunId]}
      now={now}
      selected={effectiveSelectedRuns.includes(run.fullRunId)}
      color={mergedRunColors[run.fullRunId] || RUN_COLORS[0]}
      comparing={mode === "compare"}
      canDelete={canDeleteRuns}
      isDeleting={isDeletingRuns}
      onToggle={() => onToggleRun(run.fullRunId)}
      onColorChange={(color) => onRunColorChange(run.fullRunId, color)}
      onDelete={() => onDeleteRun(run.fullRunId)}
    />
  );

  return (
    <>
      <div className={`sidebar-backdrop ${sidebarOpen ? "is-open" : ""}`} onClick={onClose} />
      <aside id="dashboard-sidebar" className={`sidebar ${sidebarOpen ? "is-open" : ""}`}>
        <div className="sidebar-header">
          <h1 className="logo-text">Aeroboard</h1>
          <button
            type="button"
            className="theme-toggle-btn"
            onClick={onThemeToggle}
            aria-label={`Switch to ${nextTheme} theme`}
            title={`Switch to ${nextTheme} theme`}
          >
            {theme === "dark" ? <Sun size={15} /> : <Moon size={15} />}
          </button>
        </div>

        <div className="run-tools">
          <div className="segmented mode-toggle" role="group" aria-label="Run selection">
            {effectiveActiveTab !== "analysis" ? (
              <button type="button" className={mode === "single" ? "active" : ""} aria-pressed={mode === "single"} onClick={() => onChangeMode("single")}>Single</button>
            ) : null}
            <button type="button" className={mode === "compare" ? "active" : ""} aria-pressed={mode === "compare"} onClick={() => onChangeMode("compare")}>Compare</button>
          </div>
          <div className="run-filter-wrap">
            <Search size={14} className="run-filter-icon" />
            <input
              className="run-filter-input"
              type="search"
              value={runFilter}
              onChange={(event) => onRunFilterChange(event.target.value)}
              placeholder="Filter runs"
              aria-label="Filter runs"
            />
          </div>
        </div>

        <div className="run-list scrollbar-hidden">
          {filteredExperimentGroups.map((group) => {
            // A group of one is just a run; a heading would only repeat its name.
            if (group.totalRuns === 1) return renderRun(group.runs[0], group.runs[0].fullRunId);

            const isCollapsed = !!collapsedExperiments[group.experimentName];
            const selectedCount = group.runs.filter((run) => effectiveSelectedRuns.includes(run.fullRunId)).length;

            return (
              <div key={group.experimentName} className="experiment-group">
                <div className="experiment-group-heading">
                  <button type="button" className="experiment-group-header" onClick={() => onToggleExperimentCollapsed(group.experimentName)} aria-expanded={!isCollapsed}>
                    {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                    <span className="experiment-group-copy">
                      <span className="experiment-group-title" title={group.experimentName}>{group.experimentName}</span>
                      <small className="experiment-group-meta">
                        {selectedCount ? `${selectedCount} of ${group.totalRuns} selected` : `${group.totalRuns} versions`}
                      </small>
                    </span>
                    <RunAge startedAtMs={group.latestStartedAtMs} now={now} />
                  </button>
                  {canDeleteRuns ? (
                    <button type="button" className="run-delete-btn" disabled={isDeletingRuns}
                      onClick={() => onDeleteGroup(group.experimentName)}
                      aria-label={`Delete run group ${group.experimentName}`} title="Delete all versions in this group">
                      <Trash2 size={14} />
                    </button>
                  ) : null}
                </div>

                {!isCollapsed ? (
                  <div className="experiment-runs">
                    {group.runs.map((run) => renderRun(run, runLabelInGroup(run)))}
                  </div>
                ) : null}
              </div>
            );
          })}

          {runs.length === 0 ? <p className="empty-hint">No runs detected</p> : null}
          {runs.length > 0 && filteredExperimentGroups.length === 0 ? <p className="empty-hint">No runs match that filter.</p> : null}
        </div>

        <div className="sidebar-footer">
          <button type="button" className="text-btn danger-btn" onClick={onDeleteAllRuns} disabled={runs.length === 0 || isDeletingRuns}>
            {isDeletingRuns ? "Deleting…" : "Delete all runs"}
          </button>
        </div>
      </aside>
    </>
  );
}
