import { memo, useCallback, useEffect, useRef, useState, useSyncExternalStore, type CSSProperties, type FormEvent } from "react";
import { seriesVersion, subscribeSeries } from "../hooks/aeroboard/seriesSubscriptions";
import { RUN_COLORS } from "../lib/runColors";
import { ChevronDown, ChevronRight, Crosshair, Download, EyeOff, GripVertical, Star, TrendingUp } from "lucide-react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import { LineChart, type ChartMarker, type XAxisType } from "./LineChart";
import { MetricName } from "./MetricName";
import { exportChart } from "../lib/export";
import type { TimeSeriesData } from "../hooks/useAeroboard";
import type { WidthMode } from "../hooks/useDashboardPreferences";
import type { YAxisScale } from "../lib/chartScale";

interface SortableMetricChartProps {
  yAxisScale: YAxisScale;
  onScaleChange: (scale: YAxisScale) => void;
  showTrend: boolean;
  onToggleTrend: () => void;
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
  onToggleCollapse: () => void;
  collapsed: boolean;
  favorite: boolean;
  onToggleFavorite: () => void;
  optimalValue: number | null;
  onOptimalValueChange: (value: number | null) => void;
}

export const SortableMetricChart = memo(function SortableMetricChart({
  yAxisScale, onScaleChange, showTrend, onToggleTrend, onHide, markers, onMarkerSelect,
  id,
  groupKey,
  seriesData,
  theme,
  xAxisType,
  smoothing,
  removeOutliers,
  xRange,
  widthMode,
  onToggleCollapse,
  collapsed,
  favorite,
  onToggleFavorite,
  optimalValue,
  onOptimalValueChange,
}: SortableMetricChartProps) {
  const { attributes, listeners, setNodeRef, setActivatorNodeRef, transform, transition, isDragging } = useSortable({ id });
  const cardRef = useRef<HTMLDivElement | null>(null);
  const [nearViewport,setNearViewport]=useState(true);
  useEffect(()=>{
    if(!cardRef.current||typeof IntersectionObserver==='undefined')return;
    const observer=new IntersectionObserver(([entry])=>setNearViewport(entry.isIntersecting),{rootMargin:'400px'});
    observer.observe(cardRef.current);return ()=>observer.disconnect();
  },[]);
  const subscribe=useCallback((listener:()=>void)=>{
    if(!nearViewport)return ()=>{};
    const cleanup=seriesData.map(series=>subscribeSeries(series.fullData,listener));
    return ()=>cleanup.forEach(fn=>fn());
  },[seriesData,nearViewport]);
  const snapshot=useCallback(()=>seriesData.reduce((total,series)=>total+seriesVersion(series.fullData),0),[seriesData]);
  const dataVersion=useSyncExternalStore(subscribe,snapshot,snapshot);
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
      <div
        className="chart-header"
        onPointerDown={event => {
          if (!(event.target as Element).closest("button, input, select, a")) listeners?.onPointerDown(event);
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
              <span className="legend-dot" style={{ backgroundColor: series.color || RUN_COLORS[0] }} />
              <span className="legend-label" title={series.name}>{series.name}</span>
              <span className="legend-val">
                {series.fullData.values[series.fullData.count - 1]?.toFixed(4)}
              </span>
            </div>
          ))}
          {showTrend ? <span className="trend-legend"><span aria-hidden="true" />Linear trend</span> : null}
        </div>

        <div className="chart-actions">
          <select className="chart-scale-select" aria-label={`Y-axis scale for ${groupKey}`} value={yAxisScale}
            title={yAxisScale === "exponential" ? "Exponential: expands differences near the high end" : "Y-axis scale"}
            onChange={event => onScaleChange(event.target.value as YAxisScale)}>
            <option value="linear">Linear</option>
            <option value="log">Log</option>
            <option value="exponential">Exponential</option>
          </select>
          <button type="button" className={`chart-icon-btn ${showTrend ? "is-active" : ""}`}
            aria-label={`Linear trend for ${groupKey}`} aria-pressed={showTrend} onClick={onToggleTrend}
            title={showTrend ? "Hide linear trends" : "Show linear trends fitted to raw samples in the visible window"}>
            <TrendingUp size={14} />
          </button>
          <button type="button" className="chart-icon-btn" aria-label="PNG" title="Download chart as PNG" disabled={collapsed} onClick={() => exportChart(cardRef.current,`${groupKey} · ${xAxisType} · ${yAxisScale}${showTrend ? " · linear trend" : ""} · smoothing ${smoothing} · ${removeOutliers ? "trimmed" : "all values"}`)}><Download size={14} /></button>
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

      {markers.length ? <div className="chart-event-markers">{markers.slice(-12).map(marker => <button key={marker.id} type="button" onClick={() => onMarkerSelect(marker.id)} title={marker.label}>◆ {marker.label}</button>)}</div> : null}
      {!collapsed && nearViewport && (
        <div className="plot-container">
          <LineChart
            title={groupKey}
            seriesData={seriesData}
            dataVersion={dataVersion}
            themeKey={theme}
            xAxisType={xAxisType}
            smoothing={smoothing}
            removeOutliers={removeOutliers}
            xRange={xRange}
            referenceValue={optimalValue}
            yAxisScale={yAxisScale} markers={markers} onMarkerSelect={onMarkerSelect}
            showTrend={showTrend}
            height={280}
          />
        </div>
      )}
    </div>
  );
});
