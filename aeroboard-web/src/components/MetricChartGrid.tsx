import { DndContext, KeyboardSensor, PointerSensor, closestCenter, pointerWithin, useSensor, useSensors, type CollisionDetection, type DragEndEvent } from "@dnd-kit/core";
import { SortableContext, rectSortingStrategy, sortableKeyboardCoordinates } from "@dnd-kit/sortable";

import type { TimeSeriesData } from "../hooks/useAeroboard";
import type { XAxisType } from "./LineChart";
import type { WidthMode } from "../hooks/useDashboardPreferences";
import { SortableMetricChart } from "./SortableMetricChart";
import { chartBaseName } from "../lib/dashboardDefaults";
import type { RunEvent } from "../hooks/useWorkspace";

const STAGE_COLORS: Record<string, string> = {
  train: "#00d2ff",
  val: "#ff4c6b",
  test: "#00e676",
};

/** Cards differ in height, so the drop follows the pointer; the keyboard has none and uses card centers. */
const dropTarget: CollisionDetection = (args) => {
  const underPointer = pointerWithin(args);
  return underPointer.length > 0 ? underPointer : closestCenter(args);
};

interface MetricChartGridProps {
  logCharts: Record<string, boolean>;
  onToggleLog: (key: string) => void;
  onHideChart: (key: string) => void;
  events: RunEvent[];
  onEventSelect: (event: RunEvent) => void;
  visibleMetricKeys: string[];
  groupedMetrics: Record<string, string[]>;
  currentMode: "single" | "compare";
  effectiveSelectedRuns: string[];
  getChartData: (runId: string, name: string) => TimeSeriesData | null;
  mergedRunColors: Record<string, string>;
  theme: "light" | "dark";
  xAxisType: XAxisType;
  smoothing: number;
  removeOutliers: boolean;
  xRange: [number, number] | null;
  widthMode: WidthMode;
  favoriteCharts: Record<string, boolean>;
  collapsedCharts: Record<string, boolean>;
  singleRunRevision: string;
  setFavoriteCharts: React.Dispatch<React.SetStateAction<Record<string, boolean>>>;
  setCollapsedCharts: React.Dispatch<React.SetStateAction<Record<string, boolean>>>;
  onReorderChart: (active: string, over: string) => void;
  onPinChart: (key: string) => void;
  /** Effective target line per chart (user override, else configured default). */
  optimalValues: Record<string, number | null>;
  setOptimalValues: React.Dispatch<React.SetStateAction<Record<string, number | null>>>;
}

export function MetricChartGrid({
  logCharts, onToggleLog, onHideChart, events, onEventSelect,
  visibleMetricKeys,
  groupedMetrics,
  currentMode,
  effectiveSelectedRuns,
  getChartData,
  mergedRunColors,
  theme,
  xAxisType,
  smoothing,
  removeOutliers,
  xRange,
  widthMode,
  favoriteCharts,
  collapsedCharts,
  singleRunRevision,
  setFavoriteCharts,
  setCollapsedCharts,
  onReorderChart,
  onPinChart,
  optimalValues,
  setOptimalValues,
}: MetricChartGridProps) {
  const sensors = useSensors(
    // The card header starts drags too, so a plain click on it must not count as one.
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const isChartCollapsed = (groupKey: string) => collapsedCharts[chartBaseName(groupKey)] ?? !favoriteCharts[groupKey];

  // Removal is stored as null, not deleted, so a configured default target stays off.
  const setOptimalValue = (groupKey: string, value: number | null) => {
    setOptimalValues((prev) => ({ ...prev, [chartBaseName(groupKey)]: value }));
  };

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (over && active.id !== over.id) onReorderChart(String(active.id), String(over.id));
  };

  return (
    <DndContext sensors={sensors} collisionDetection={dropTarget} onDragEnd={handleDragEnd}>
      <SortableContext items={visibleMetricKeys} strategy={rectSortingStrategy}>
        <div className={`grid-layout grid-layout--${widthMode}`}>
          {visibleMetricKeys.map((groupKey) => {
            const targets = groupedMetrics[groupKey];
            if (!targets) return null;

            let seriesData: { name: string; fullData: TimeSeriesData; color?: string }[] = [];

            if (currentMode === "single") {
              const run = effectiveSelectedRuns[0];
              seriesData = targets
                .map((metric) => {
                  const stage = metric.split("/")[0];
                  const data = getChartData(run, metric);
                  if (!data) return null;
                  return { name: stage, fullData: data, color: STAGE_COLORS[stage] };
                })
                .filter((series): series is { name: string; fullData: TimeSeriesData; color: string } => series !== null);
            } else {
              seriesData = effectiveSelectedRuns
                .map((runId) => {
                  const data = getChartData(runId, groupKey);
                  if (!data) return null;
                  return { name: runId, fullData: data, color: mergedRunColors[runId] };
                })
                .filter((series): series is { name: string; fullData: TimeSeriesData; color: string } => series !== null);
            }

            if (seriesData.length === 0) return null;

            return (
              <SortableMetricChart
                key={groupKey}
                id={groupKey}
                groupKey={groupKey}
                seriesData={seriesData}
                theme={theme}
                xAxisType={xAxisType}
                smoothing={smoothing}
                removeOutliers={removeOutliers}
                xRange={xRange}
                widthMode={widthMode}
                revisionKey={`${singleRunRevision}:${groupKey}:${xAxisType}:${removeOutliers ? "trim" : "full"}`}
                favorite={!!favoriteCharts[groupKey]}
                onToggleFavorite={() => {
                  setFavoriteCharts((prev) => ({ ...prev, [chartBaseName(groupKey)]: !favoriteCharts[groupKey] }));
                  if (!favoriteCharts[groupKey]) onPinChart(groupKey);
                }}
                collapsed={isChartCollapsed(groupKey)}
                onToggleCollapse={() => setCollapsedCharts((prev) => ({ ...prev, [chartBaseName(groupKey)]: !isChartCollapsed(groupKey) }))}
                optimalValue={optimalValues[groupKey] ?? null}
                onOptimalValueChange={(value) => setOptimalValue(groupKey, value)}
                logarithmic={!!logCharts[chartBaseName(groupKey)]} onToggleLog={() => onToggleLog(groupKey)} onHide={() => onHideChart(groupKey)}
                markers={events.flatMap(event => {
                  const position = xAxisType === "step" ? event.data.step : xAxisType === "wall" ? event.data.timestamp_ms / 1000 : seriesData[0] ? event.data.timestamp_ms / 1000 - seriesData[0].fullData.wallTimes[0] : null;
                  return position === null ? [] : [{id: event.id, x: position, label: event.data.label}];
                })}
                onMarkerSelect={id => {const event = events.find(event => event.id === id);if(event) onEventSelect(event);}}
              />
            );
          })}
        </div>
      </SortableContext>
    </DndContext>
  );
}
