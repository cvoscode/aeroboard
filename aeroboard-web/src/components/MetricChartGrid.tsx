import { DndContext, KeyboardSensor, PointerSensor, closestCenter, useSensor, useSensors, type DragEndEvent } from "@dnd-kit/core";
import { SortableContext, arrayMove, rectSortingStrategy, sortableKeyboardCoordinates } from "@dnd-kit/sortable";

import type { TimeSeriesData } from "../hooks/useAeroboard";
import type { XAxisType } from "./LineChart";
import type { WidthMode } from "../hooks/useDashboardPreferences";
import { SortableMetricChart } from "./SortableMetricChart";

const STAGE_COLORS: Record<string, string> = {
  train: "#00d2ff",
  val: "#ff4c6b",
  test: "#00e676",
};

interface MetricChartGridProps {
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
  setOrderedMetrics: React.Dispatch<React.SetStateAction<string[]>>;
  /** Effective target line per chart (user override, else configured default). */
  optimalValues: Record<string, number | null>;
  setOptimalValues: React.Dispatch<React.SetStateAction<Record<string, number | null>>>;
}

export function MetricChartGrid({
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
  setOrderedMetrics,
  optimalValues,
  setOptimalValues,
}: MetricChartGridProps) {
  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const isChartCollapsed = (groupKey: string) => collapsedCharts[groupKey] ?? !favoriteCharts[groupKey];

  // Removal is stored as null, not deleted, so a configured default target stays off.
  const setOptimalValue = (groupKey: string, value: number | null) => {
    setOptimalValues((prev) => ({ ...prev, [groupKey]: value }));
  };

  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    const currentOrder = [...visibleMetricKeys];
    const oldIdx = currentOrder.indexOf(active.id as string);
    const newIdx = currentOrder.indexOf(over.id as string);
    if (oldIdx < 0 || newIdx < 0) return;

    setOrderedMetrics(arrayMove(currentOrder, oldIdx, newIdx));
  };

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
      <SortableContext items={visibleMetricKeys} strategy={rectSortingStrategy}>
        <div className="grid-layout">
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
                onToggleFavorite={() => setFavoriteCharts((prev) => ({ ...prev, [groupKey]: !favoriteCharts[groupKey] }))}
                collapsed={isChartCollapsed(groupKey)}
                onToggleCollapse={() => setCollapsedCharts((prev) => ({ ...prev, [groupKey]: !isChartCollapsed(groupKey) }))}
                optimalValue={optimalValues[groupKey] ?? null}
                onOptimalValueChange={(value) => setOptimalValue(groupKey, value)}
              />
            );
          })}
        </div>
      </SortableContext>
    </DndContext>
  );
}
