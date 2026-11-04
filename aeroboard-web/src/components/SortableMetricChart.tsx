import { useRef, useState, type CSSProperties, type FormEvent } from "react";
import { ChevronDown, ChevronRight, Crosshair, Download, EyeOff, GripVertical, Star } from "lucide-react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import { LineChart, type ChartMarker, type XAxisType } from "./LineChart";
import { MetricName } from "./MetricName";
import { exportChart } from "../lib/export";
import type { TimeSeriesData } from "../hooks/useAeroboard";
import type { WidthMode } from "../hooks/useDashboardPreferences";

interface SortableMetricChartProps {
  logarithmic: boolean;
  onToggleLog: () => void;
  onHide: () => void;
  markers: ChartMarker[];
  onMarkerSelect: (id: string) => void;
  id: string;
  groupKey: string;
  seriesData: { name: string; fullData: TimeSeriesData; color?: string }[];
  theme: "light" | "dark";
  xAxisType: XAxisType;
  smoothing: number;
  removeOutliers: boolean;
  xRange: [number, number] | null;
  widthMode: WidthMode;
  revisionKey: string;
  onToggleCollapse: () => void;
  collapsed: boolean;
  favorite: boolean;
  onToggleFavorite: () => void;
  optimalValue: number | null;
  onOptimalValueChange: (value: number | null) => void;
}

export function SortableMetricChart({
  logarithmic, onToggleLog, onHide, markers, onMarkerSelect,
  id,
  groupKey,
  seriesData,
  theme,
  xAxisType,
  smoothing,
  removeOutliers,
  xRange,
  widthMode,
  revisionKey,
  onToggleCollapse,
  collapsed,
  favorite,
  onToggleFavorite,
  optimalValue,
  onOptimalValueChange,
}: SortableMetricChartProps) {
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } = useSortable({ id });
  const cardRef = useRef<HTMLDivElement | null>(null);
  const [editingOptimal, setEditingOptimal] = useState(false);
  const [optimalDraft, setOptimalDraft] = useState("");

  const openOptimalEditor = () => {
    setOptimalDraft(optimalValue === null ? "" : String(optimalValue));
    setEditingOptimal(true);
  };

  const submitOptimal = (event: FormEvent) => {
    event.preventDefault();
    const trimmed = optimalDraft.trim();
    if (trimmed === "") {
      onOptimalValueChange(null);
      setEditingOptimal(false);
      return;
    }
    const parsed = Number(trimmed);
    if (!Number.isFinite(parsed)) return;
    onOptimalValueChange(parsed);
    setEditingOptimal(false);
  };

  const optimalDraftInvalid = optimalDraft.trim() !== "" && !Number.isFinite(Number(optimalDraft.trim()));

  const style: CSSProperties = {
    // Translate only: scaling would squash a chart swapping with a card of another height.
    transform: CSS.Translate.toString(transform),
    transition,
    zIndex: isDragging ? 100 : 1,
    opacity: isDragging ? 0.8 : 1,
  };

  return (
    <div
      ref={node => {setNodeRef(node);cardRef.current = node;}}
      style={style}
      className={`chart-card width-${widthMode} ${collapsed ? "collapsed" : ""} ${favorite ? "favorite" : ""}`}
    >
      <div className="chart-card-glow" />
      <div
        className="chart-header"
        onPointerDown={event => {
          if (!(event.target as Element).closest("button, input, a")) listeners?.onPointerDown(event);
        }}
      >
        <div className="chart-header-left">
          <button className="collapse-toggle" onClick={onToggleCollapse} aria-label={`${collapsed ? "Expand" : "Collapse"} ${groupKey}`} aria-expanded={!collapsed}>
            {collapsed ? <ChevronRight size={18} /> : <ChevronDown size={18} />}
          </button>
          <div className="title-area">
            <h3 className="chart-title" title={groupKey} aria-label={groupKey}><MetricName name={groupKey} /></h3>
            <small className="chart-meta">{seriesData.length} {seriesData.length === 1 ? "trace" : "traces"}</small>
          </div>
        </div>

        <div className="chart-legend">
          {seriesData.map((series) => (
            <div key={series.name} className="legend-item">
              <span className="legend-dot" style={{ backgroundColor: series.color || "#00d2ff" }} />
              <span className="legend-label" title={series.name}>{series.name}</span>
              <span className="legend-val" style={{ color: series.color || "var(--accent-secondary)" }}>
                {series.fullData.values[series.fullData.count - 1]?.toFixed(4)}
              </span>
            </div>
          ))}
        </div>

        <div className="chart-actions">
          <button type="button" className="chip-btn" aria-pressed={logarithmic} onClick={onToggleLog} title="Toggle logarithmic y-axis">{logarithmic ? "Log" : "Linear"}</button>
          <button type="button" className="chart-icon-btn" aria-label="PNG" title="Download chart as PNG" disabled={collapsed} onClick={() => exportChart(cardRef.current,`${groupKey} · ${xAxisType} · smoothing ${smoothing} · ${removeOutliers ? "trimmed" : "all values"}`)}><Download size={14} /></button>
          <button type="button" className="chart-icon-btn" aria-label={`Hide ${groupKey}`} onClick={onHide} title="Hide this chart"><EyeOff size={14} /></button>
          <button
            className={`favorite-toggle optimal-toggle ${optimalValue !== null ? "is-active" : ""}`}
            type="button"
            onClick={() => (editingOptimal ? setEditingOptimal(false) : openOptimalEditor())}
            title={optimalValue !== null ? "Edit optimal value line" : "Add optimal value line"}
            aria-pressed={optimalValue !== null}
            aria-expanded={editingOptimal}
          >
            <Crosshair size={16} />
          </button>
          <button
            className={`favorite-toggle ${favorite ? "is-favorite" : ""}`}
            type="button"
            onClick={onToggleFavorite}
            title={favorite ? "Unfavorite chart" : "Favorite chart"}
            aria-pressed={favorite}
          >
            <Star size={16} fill={favorite ? "currentColor" : "none"} />
          </button>
          <div ref={setActivatorNodeRef} className="drag-handle-subtle" {...attributes} onKeyDown={event => listeners?.onKeyDown(event)}>
            <GripVertical size={16} />
          </div>
        </div>
      </div>

      {editingOptimal ? (
        <form className="optimal-editor" onSubmit={submitOptimal}>
          <label htmlFor={`optimal-${id}`}>Optimal value</label>
          <input
            id={`optimal-${id}`}
            className={`optimal-input ${optimalDraftInvalid ? "is-invalid" : ""}`}
            type="text"
            inputMode="decimal"
            value={optimalDraft}
            placeholder="e.g. 0.95 (empty to remove)"
            autoFocus
            aria-invalid={optimalDraftInvalid}
            onChange={(event) => setOptimalDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") setEditingOptimal(false);
            }}
          />
          <button type="submit" className="selector-action-btn" disabled={optimalDraftInvalid}>Set</button>
          {optimalValue !== null ? (
            <button
              type="button"
              className="selector-action-btn"
              onClick={() => {
                onOptimalValueChange(null);
                setEditingOptimal(false);
              }}
            >
              Remove
            </button>
          ) : null}
        </form>
      ) : null}

      {logarithmic ? <p className="workspace-hint">Log scale shows positive finite samples only.{optimalValue !== null && optimalValue <= 0 ? " The nonpositive target line is hidden." : ""}</p> : null}
      {markers.length ? <div className="chart-event-markers">{markers.slice(-12).map(marker => <button key={marker.id} type="button" onClick={() => onMarkerSelect(marker.id)} title={marker.label}>◆ {marker.label}</button>)}</div> : null}
      {!collapsed && (
        <div className="plot-container">
          <LineChart
            key={revisionKey}
            title={groupKey}
            seriesData={seriesData}
            themeKey={theme}
            xAxisType={xAxisType}
            smoothing={smoothing}
            removeOutliers={removeOutliers}
            xRange={xRange}
            referenceValue={optimalValue}
            logarithmic={logarithmic} markers={markers} onMarkerSelect={onMarkerSelect}
            height={280}
          />
        </div>
      )}
    </div>
  );
}
