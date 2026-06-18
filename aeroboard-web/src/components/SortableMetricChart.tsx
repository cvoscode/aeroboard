import type { CSSProperties } from "react";
import { ChevronDown, ChevronRight, GripVertical, Star } from "lucide-react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";

import { LineChart, type XAxisType } from "./LineChart";
import type { TimeSeriesData } from "../hooks/useAeroboard";
import type { WidthMode } from "../hooks/useDashboardPreferences";

interface SortableMetricChartProps {
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
}

export function SortableMetricChart({
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
}: SortableMetricChartProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });

  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 100 : 1,
    opacity: isDragging ? 0.8 : 1,
    gridColumn: widthMode === "full" ? "span 12" : widthMode === "wide" ? "span 6" : "span 4",
  };

  return (
    <div
      ref={setNodeRef}
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
        </div>

        <div className="chart-actions">
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
            height={280}
          />
        </div>
      )}
    </div>
  );
}
