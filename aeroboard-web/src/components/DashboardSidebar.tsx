import {
  Activity,
  Clock,
  Columns,
  Hash,
  Layers,
  List,
  Maximize2,
  Moon,
  RotateCcw,
  Search,
  Settings2,
  Sliders,
  Sun,
  Trash2,
  ChevronDown,
  ChevronRight,
} from "lucide-react";

import { ColorPicker } from "./ColorPicker";
import { DualRangeSlider } from "./DualRangeSlider";
import type { XAxisType } from "./LineChart";
import type { ExperimentGroup } from "../lib/runGroups";
import type { WidthMode } from "../hooks/useDashboardPreferences";

interface DashboardSidebarProps {
  sidebarOpen: boolean;
  onClose: () => void;
  theme: "light" | "dark";
  onThemeToggle: () => void;
  nextTheme: "light" | "dark";
  effectiveActiveTab: "scalars" | "images" | "analysis";
  mode: "single" | "compare";
  onChangeMode: (mode: "single" | "compare") => void;
  runs: string[];
  isDeletingRuns: boolean;
  onDeleteAllRuns: () => void;
  runFilter: string;
  onRunFilterChange: (value: string) => void;
  filteredExperimentGroups: ExperimentGroup[];
  effectiveSelectedRuns: string[];
  collapsedExperiments: Record<string, boolean>;
  onToggleExperimentCollapsed: (name: string) => void;
  onToggleRun: (runId: string) => void;
  mergedRunColors: Record<string, string>;
  onRunColorChange: (runId: string, color: string) => void;
  xAxisType: XAxisType;
  onXAxisTypeChange: (type: XAxisType) => void;
  smoothing: number;
  onSmoothingChange: (value: number) => void;
  removeOutliers: boolean;
  onRemoveOutliersChange: (value: boolean) => void;
  widthMode: WidthMode;
  onWidthModeChange: (mode: WidthMode) => void;
  bounds: [number, number];
  globalXRange: [number, number] | null;
  onGlobalXRangeChange: (value: [number, number] | null) => void;
  availableHyperparameters: string[];
  selectedCompareHyperparameters: string[];
  onToggleSelection: (value: string, type: "hyper" | "metric") => void;
  onToggleAllHyperparameters: () => void;
  availableMetricSummaries: string[];
  selectedCompareMetrics: string[];
  onToggleAllMetrics: () => void;
  hasAnyImages: boolean;
  imageSeriesOptions: { name: string }[];
  selectedImageName: string;
  onSelectedImageNameChange: (value: string) => void;
}

export function DashboardSidebar({
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
  runFilter,
  onRunFilterChange,
  filteredExperimentGroups,
  effectiveSelectedRuns,
  collapsedExperiments,
  onToggleExperimentCollapsed,
  onToggleRun,
  mergedRunColors,
  onRunColorChange,
  xAxisType,
  onXAxisTypeChange,
  smoothing,
  onSmoothingChange,
  removeOutliers,
  onRemoveOutliersChange,
  widthMode,
  onWidthModeChange,
  bounds,
  globalXRange,
  onGlobalXRangeChange,
  availableHyperparameters,
  selectedCompareHyperparameters,
  onToggleSelection,
  onToggleAllHyperparameters,
  availableMetricSummaries,
  selectedCompareMetrics,
  onToggleAllMetrics,
  hasAnyImages,
  imageSeriesOptions,
  selectedImageName,
  onSelectedImageNameChange,
}: DashboardSidebarProps) {
  return (
    <>
      <div className={`sidebar-backdrop ${sidebarOpen ? "is-open" : ""}`} onClick={onClose} />
      <aside id="dashboard-sidebar" className={`sidebar ${sidebarOpen ? "is-open" : ""}`}>
        <div className="sidebar-header">
          <div className="logo-box"><Activity size={24} /></div>
          <div className="sidebar-brand-copy">
            <h1 className="logo-text">Aeroboard</h1>
          </div>
          <button
            type="button"
            className="theme-toggle-btn"
            onClick={onThemeToggle}
            aria-label={`Switch to ${nextTheme} theme`}
            title={`Switch to ${nextTheme} theme`}
          >
            {theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
            <span className="theme-toggle-label">{theme === "dark" ? "Light" : "Dark"}</span>
          </button>
        </div>

        <div className="sidebar-section">
          <div className="section-header"><Layers size={14} /><h3>Mode</h3></div>
          <div className="mode-toggle">
            {effectiveActiveTab === "analysis" ? (
              <button className={mode === "compare" ? "active" : ""} onClick={() => onChangeMode("compare")}>
                <Columns size={14} /> Compare
              </button>
            ) : (
              <>
                <button className={mode === "single" ? "active" : ""} onClick={() => onChangeMode("single")}>
                  <Search size={14} /> Single
                </button>
                <button className={mode === "compare" ? "active" : ""} onClick={() => onChangeMode("compare")}>
                  <Columns size={14} /> Compare
                </button>
              </>
            )}
          </div>
        </div>

        <div className="sidebar-section">
          <div className="section-header"><List size={14} /><h3>Experiments</h3></div>
          <div className="run-actions-row">
            <button className="danger-btn" onClick={onDeleteAllRuns} disabled={runs.length === 0 || isDeletingRuns}>
              <Trash2 size={13} /> {isDeletingRuns ? "Deleting…" : "Delete All Runs"}
            </button>
          </div>
          <div className="run-filter-wrap">
            <Search size={14} className="run-filter-icon" />
            <input
              className="run-filter-input"
              type="search"
              value={runFilter}
              onChange={(event) => onRunFilterChange(event.target.value)}
              placeholder="Filter by run name or version"
              aria-label="Filter runs"
            />
          </div>
          <div className="run-list scrollbar-hidden">
            {filteredExperimentGroups.map((group) => {
              const isCollapsed = !!collapsedExperiments[group.experimentName];
              const selectedCount = group.runs.filter((run) => effectiveSelectedRuns.includes(run.fullRunId)).length;

              return (
                <div key={group.experimentName} className="experiment-group">
                  <button className="experiment-group-header" onClick={() => onToggleExperimentCollapsed(group.experimentName)}>
                    <div className="experiment-group-title-wrap">
                      {isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
                      <div className="experiment-group-copy">
                        <span className="experiment-group-title" title={group.experimentName}>{group.experimentName}</span>
                        <small className="experiment-group-meta">
                          {group.runs.length} version{group.runs.length === 1 ? "" : "s"}{selectedCount ? ` • ${selectedCount} selected` : ""}
                        </small>
                      </div>
                    </div>
                    <span className="experiment-group-latest">latest {group.runs[0]?.versionLabel}</span>
                  </button>

                  {!isCollapsed ? (
                    <div className="experiment-runs">
                      {group.runs.map((run) => (
                        <div key={run.fullRunId} className={`run-card-wrapper ${effectiveSelectedRuns.includes(run.fullRunId) ? "active" : ""}`}>
                          <button
                            className="run-card-btn"
                            onClick={() => onToggleRun(run.fullRunId)}
                            title={run.fullRunId}
                            aria-label={run.fullRunId}
                          >
                            <div className="run-card-indicator" style={{ backgroundColor: mergedRunColors[run.fullRunId] }} />
                            <div className="run-card-copy">
                              <span className="run-name">{run.fullRunId}</span>
                              <small className="run-version-label run-version-pill">{run.versionLabel}</small>
                            </div>
                          </button>
                          {mode === "compare" ? (
                            <ColorPicker
                              color={mergedRunColors[run.fullRunId] || "#00d2ff"}
                              onChange={(color) => onRunColorChange(run.fullRunId, color)}
                            />
                          ) : null}
                        </div>
                      ))}
                    </div>
                  ) : null}
                </div>
              );
            })}

            {runs.length === 0 ? <p className="empty-hint">No runs detected</p> : null}
            {runs.length > 0 && filteredExperimentGroups.length === 0 ? <p className="empty-hint">No runs match that filter.</p> : null}
          </div>
        </div>

        {effectiveActiveTab === "scalars" ? (
          <div className="sidebar-section">
            <div className="section-header"><Settings2 size={14} /><h3>View Configuration</h3></div>
            <div className="control-card glass-card">
              <label>X-Axis Mode</label>
              <div className="axis-tabs">
                <button className={xAxisType === "wall" ? "active" : ""} onClick={() => onXAxisTypeChange("wall")}><Clock size={14} /></button>
                <button className={xAxisType === "rel" ? "active" : ""} onClick={() => onXAxisTypeChange("rel")}><RotateCcw size={14} /></button>
                <button className={xAxisType === "step" ? "active" : ""} onClick={() => onXAxisTypeChange("step")}><Hash size={14} /></button>
              </div>

              <div className="control-item">
                <div className="label-row">
                  <Sliders size={12} /> smoothing <span className="val">{Math.round(smoothing * 100)}%</span>
                </div>
                <input
                  type="range"
                  min="0"
                  max="0.95"
                  step="0.05"
                  value={smoothing}
                  onChange={(event) => onSmoothingChange(Number.parseFloat(event.target.value))}
                  className="styled-slider"
                />
              </div>

              <div className="control-item row">
                <label className="checkbox-wrap">
                  <input type="checkbox" checked={removeOutliers} onChange={(event) => onRemoveOutliersChange(event.target.checked)} />
                  <div className={`check-box ${removeOutliers ? "checked" : ""}`}><div className="inner" /></div>
                  <span>Remove Outliers</span>
                </label>
              </div>

              <div className="control-item">
                <div className="label-row">
                  <Columns size={12} /> plot width <span className="val">{widthMode}</span>
                </div>
                <div className="axis-tabs width-tabs">
                  <button className={widthMode === "compact" ? "active" : ""} onClick={() => onWidthModeChange("compact")}>Compact</button>
                  <button className={widthMode === "wide" ? "active" : ""} onClick={() => onWidthModeChange("wide")}>Wide</button>
                  <button className={widthMode === "full" ? "active" : ""} onClick={() => onWidthModeChange("full")}>Full</button>
                </div>
              </div>
            </div>
          </div>
        ) : null}

        {effectiveActiveTab === "scalars" ? (
          <div className="sidebar-section">
            <div className="section-header"><Maximize2 size={14} /><h3>Active Window</h3></div>
            <div className="control-card glass-card">
              {bounds[0] !== Infinity ? (
                <>
                  <DualRangeSlider
                    min={bounds[0]}
                    max={bounds[1]}
                    value={globalXRange ?? bounds}
                    onChange={onGlobalXRangeChange}
                    xAxisType={xAxisType}
                  />
                  <button className="reset-range-btn" onClick={() => onGlobalXRangeChange(null)}>
                    Reset Window
                  </button>
                </>
              ) : (
                <p className="empty-hint">Waiting for data...</p>
              )}
            </div>
          </div>
        ) : null}

        {effectiveActiveTab === "analysis" ? (
          <div className="sidebar-section">
            <div className="section-header"><Columns size={14} /><h3>Comparison Axes</h3></div>
            <div className="control-card glass-card selector-card">
              <div className="selector-header-row">
                <label>Hyperparameters</label>
                {availableHyperparameters.length > 0 ? (
                  <button type="button" className="selector-action-btn" onClick={onToggleAllHyperparameters}>
                    {selectedCompareHyperparameters.length === availableHyperparameters.length ? "Clear All" : "Select All"}
                  </button>
                ) : null}
              </div>
              <div className="chip-list">
                {availableHyperparameters.map((key) => (
                  <button
                    key={key}
                    className={selectedCompareHyperparameters.includes(key) ? "chip-btn active" : "chip-btn"}
                    onClick={() => onToggleSelection(key, "hyper")}
                  >
                    {key}
                  </button>
                ))}
                {availableHyperparameters.length === 0 ? <p className="empty-hint">No hyperparameters logged yet.</p> : null}
              </div>

              <div className="selector-header-row">
                <label>Metrics</label>
                {availableMetricSummaries.length > 0 ? (
                  <button type="button" className="selector-action-btn" onClick={onToggleAllMetrics}>
                    {selectedCompareMetrics.length === availableMetricSummaries.length ? "Clear All" : "Select All"}
                  </button>
                ) : null}
              </div>
              <div className="chip-list">
                {availableMetricSummaries.map((key) => (
                  <button
                    key={key}
                    className={selectedCompareMetrics.includes(key) ? "chip-btn active" : "chip-btn"}
                    onClick={() => onToggleSelection(key, "metric")}
                  >
                    {key}
                  </button>
                ))}
                {availableMetricSummaries.length === 0 ? <p className="empty-hint">No summary metrics available.</p> : null}
              </div>
            </div>
          </div>
        ) : null}

        {hasAnyImages && effectiveActiveTab === "images" ? (
          <div className="sidebar-section">
            <div className="section-header"><List size={14} /><h3>Image Streams</h3></div>
            <div className="control-card glass-card selector-card">
              {imageSeriesOptions.length > 0 ? (
                <select
                  className="image-series-select"
                  value={selectedImageName}
                  onChange={(event) => onSelectedImageNameChange(event.target.value)}
                >
                  {imageSeriesOptions.map((series) => (
                    <option key={series.name} value={series.name}>{series.name}</option>
                  ))}
                </select>
              ) : (
                <p className="empty-hint">Select a run with logged images.</p>
              )}
            </div>
          </div>
        ) : null}
      </aside>
    </>
  );
}
