import { useCallback, useSyncExternalStore } from "react";
import { seriesVersion,subscribeSeries } from "../hooks/aeroboard/seriesSubscriptions";
import type { TimeSeriesData } from "../hooks/useAeroboard";
import { DualRangeSlider } from "./DualRangeSlider";
import type { XAxisType } from "./LineChart";
import type { WidthMode } from "../hooks/useDashboardPreferences";

const X_AXES: { id: XAxisType; label: string; name: string }[] = [
  { id: "step", label: "Step", name: "Training step" },
  { id: "rel", label: "Elapsed", name: "Relative time" },
  { id: "wall", label: "Clock", name: "Wall time" },
];
const WIDTHS: { id: WidthMode; label: string }[] = [
  { id: "compact", label: "Compact" },
  { id: "wide", label: "Wide" },
  { id: "full", label: "Full" },
];

interface ChartControlsProps {
  xAxisType: XAxisType;
  onXAxisTypeChange: (type: XAxisType) => void;
  smoothing: number;
  onSmoothingChange: (value: number) => void;
  removeOutliers: boolean;
  onRemoveOutliersChange: (value: boolean) => void;
  widthMode: WidthMode;
  onWidthModeChange: (mode: WidthMode) => void;
  /** True when the user has changed stars, order, collapse state or target lines. */
  canResetChartView: boolean;
  onResetChartView: () => void;
  bounds: [number, number];
  series?: TimeSeriesData[];
  readBounds?: ()=>[number,number];
  globalXRange: [number, number] | null;
  onGlobalXRangeChange: (value: [number, number] | null) => void;
}

/** Settings for every chart on the page, kept beside the charts they change. */
export function ChartControls({
  xAxisType,
  onXAxisTypeChange,
  smoothing,
  onSmoothingChange,
  removeOutliers,
  onRemoveOutliersChange,
  widthMode,
  onWidthModeChange,
  canResetChartView,
  onResetChartView,
  bounds:initialBounds,
  series=[],readBounds,
  globalXRange,
  onGlobalXRangeChange,
}: ChartControlsProps) {
  const subscribe=useCallback((listener:()=>void)=>{const cleanup=series.map(data=>subscribeSeries(data,listener));return ()=>cleanup.forEach(fn=>fn());},[series]);
  const snapshot=useCallback(()=>series.map(seriesVersion).join(','),[series]);
  useSyncExternalStore(subscribe,snapshot,snapshot);
  const bounds=readBounds?.()??initialBounds;
  return (
    <section className="view-controls" aria-label="Chart settings">
      <div className="view-control">
        <span className="view-control-label">X axis</span>
        <div className="segmented" role="group" aria-label="X axis">
          {X_AXES.map(({ id, label, name }) => (
            <button key={id} type="button" className={xAxisType === id ? "active" : ""} aria-pressed={xAxisType === id}
              aria-label={name} title={name} onClick={() => onXAxisTypeChange(id)}>{label}</button>
          ))}
        </div>
      </div>

      <label className="view-control view-control-smoothing">
        <span className="view-control-label">Smoothing</span>
        <input
          type="range"
          min="0"
          max="0.95"
          step="0.05"
          value={smoothing}
          onChange={(event) => onSmoothingChange(Number.parseFloat(event.target.value))}
          className="styled-slider"
        />
        <output>{Math.round(smoothing * 100)}%</output>
      </label>

      {bounds[0] !== Infinity ? (
        <div className="view-control view-control-range">
          <span className="view-control-label">Range</span>
          <DualRangeSlider
            min={bounds[0]}
            max={bounds[1]}
            value={globalXRange ?? bounds}
            onChange={onGlobalXRangeChange}
            xAxisType={xAxisType}
          />
          {globalXRange ? <button type="button" className="text-btn" onClick={() => onGlobalXRangeChange(null)}>Reset</button> : null}
        </div>
      ) : null}

      <label className="view-control view-control-check">
        <input type="checkbox" checked={removeOutliers} onChange={(event) => onRemoveOutliersChange(event.target.checked)} />
        <span>Remove outliers</span>
      </label>

      <div className="view-control">
        <span className="view-control-label">Chart width</span>
        <div className="segmented" role="group" aria-label="Chart width">
          {WIDTHS.map(({ id, label }) => (
            <button key={id} type="button" className={widthMode === id ? "active" : ""} aria-pressed={widthMode === id}
              onClick={() => onWidthModeChange(id)}>{label}</button>
          ))}
        </div>
      </div>

      {canResetChartView ? (
        <button
          type="button"
          className="text-btn"
          onClick={onResetChartView}
          title="Clear your starred charts, order, collapsed state and target lines, and return to the configured initial view"
        >
          Reset chart layout
        </button>
      ) : null}
    </section>
  );
}

interface AxisPickerProps {
  label: string;
  empty: string;
  options: string[];
  selected: string[];
  onToggle: (value: string) => void;
  onToggleAll: () => void;
}

function AxisPicker({ label, empty, options, selected, onToggle, onToggleAll }: AxisPickerProps) {
  return (
    <div className="axis-picker">
      <div className="selector-header-row">
        <h3>{label}</h3>
        {options.length > 0 ? (
          <button type="button" className="text-btn" onClick={onToggleAll}>
            {selected.length === options.length ? "Clear all" : "Select all"}
          </button>
        ) : null}
      </div>
      <div className="chip-list">
        {options.map((key) => (
          <button key={key} type="button" className={selected.includes(key) ? "chip-btn active" : "chip-btn"}
            aria-pressed={selected.includes(key)} onClick={() => onToggle(key)}>
            {key}
          </button>
        ))}
        {options.length === 0 ? <p className="empty-hint">{empty}</p> : null}
      </div>
    </div>
  );
}

interface ComparisonAxesProps {
  availableHyperparameters: string[];
  selectedCompareHyperparameters: string[];
  onToggleSelection: (value: string, type: "hyper" | "metric") => void;
  onToggleAllHyperparameters: () => void;
  availableMetricSummaries: string[];
  selectedCompareMetrics: string[];
  onToggleAllMetrics: () => void;
}

/** Chooses which hyperparameters and metrics the comparison panels show. */
export function ComparisonAxes({
  availableHyperparameters,
  selectedCompareHyperparameters,
  onToggleSelection,
  onToggleAllHyperparameters,
  availableMetricSummaries,
  selectedCompareMetrics,
  onToggleAllMetrics,
}: ComparisonAxesProps) {
  return (
    <section className="view-controls comparison-axes" aria-label="Comparison axes">
      <AxisPicker label="Hyperparameters" empty="No hyperparameters logged yet." options={availableHyperparameters}
        selected={selectedCompareHyperparameters} onToggle={(value) => onToggleSelection(value, "hyper")} onToggleAll={onToggleAllHyperparameters} />
      <AxisPicker label="Metrics" empty="No summary metrics available." options={availableMetricSummaries}
        selected={selectedCompareMetrics} onToggle={(value) => onToggleSelection(value, "metric")} onToggleAll={onToggleAllMetrics} />
    </section>
  );
}
