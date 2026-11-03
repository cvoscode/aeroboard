import { useRef, useState, type CSSProperties, type FormEvent } from "react";
import { ChevronDown, ChevronRight, Crosshair, GripVertical, Star } from "lucide-react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import { LineChart, type ChartMarker, type XAxisType } from "./LineChart";
import { exportChart } from "../lib/export";
import { formatReferenceValue } from "../lib/format";
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
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
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
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 100 : 1,
    opacity: isDragging ? 0.8 : 1,
    gridColumn: widthMode === "full" ? "span 12" : widthMode === "wide" ? "span 6" : "span 4",
  };

  return (
    <div
      ref={node => {setNodeRef(node);cardRef.current = node;}}
      style={style}
      className={`chart-card width-${widthMode} ${collapsed ? "collapsed" : ""} ${favorite ? "favorite" : ""}`}
    >
      <div className="chart-card-glow" />
      <div className="chart-header">
        <div className="chart-header-left">
          <button className="collapse-toggle" onClick={onToggleCollapse}>
            {collapsed ? <ChevronRight size={18} /> : <ChevronDown size={18} />}
          </button>
          <div className="title-area">
            <h3 className="chart-title">{groupKey.toUpperCase()}</h3>
            <small className="chart-meta">{seriesData.length} traces</small>
          </div>
        </div>

        <div className="chart-legend">
          {seriesData.map((series) => (
            <div key={series.name} className="legend-item">
              <span className="legend-dot" style={{ backgroundColor: series.color || "#00d2ff" }} />
              <span className="legend-val" style={{ color: series.color || "var(--accent-secondary)" }}>
                {series.fullData.values[series.fullData.count - 1]?.toFixed(4)}
              </span>
              <span className="legend-label">{series.name}</span>
            </div>
          ))}
          {optimalValue !== null ? (
            <button type="button" className="legend-item legend-optimal" onClick={openOptimalEditor} title="Edit optimal value">
              <span className="legend-optimal-swatch" aria-hidden="true" />
              <span className="legend-val legend-optimal-val">{formatReferenceValue(optimalValue)}</span>
              <span className="legend-label">optimal</span>
            </button>
          ) : null}
        </div>

        <div className="chart-actions">
          <button type="button" className="chip-btn" aria-pressed={logarithmic} onClick={onToggleLog} title="Toggle logarithmic y-axis">{logarithmic ? "Log" : "Linear"}</button>
          <button type="button" className="chip-btn" disabled={collapsed} onClick={() => exportChart(cardRef.current,`${groupKey} · ${xAxisType} · smoothing ${smoothing} · ${removeOutliers ? "trimmed" : "all values"}`)}>PNG</button>
          <button type="button" className="chip-btn" onClick={onHide} title="Hide this chart">Hide</button>
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
          <div className="drag-handle-subtle" {...attributes} {...listeners}>
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
