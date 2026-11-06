import { DndContext, KeyboardSensor, PointerSensor, closestCenter, pointerWithin, useSensor, useSensors, type CollisionDetection, type DragEndEvent } from "@dnd-kit/core";
import { SortableContext, rectSortingStrategy, sortableKeyboardCoordinates, verticalListSortingStrategy } from "@dnd-kit/sortable";

import type { TimeSeriesData } from "../hooks/useAeroboard";
import type { XAxisType } from "./LineChart";
import type { WidthMode } from "../hooks/useDashboardPreferences";
import { SortableMetricChart } from "./SortableMetricChart";
import { chartBaseName } from "../lib/dashboardDefaults";
import { RUN_COLORS } from "../lib/runColors";
import type { RunEvent } from "../hooks/useWorkspace";
import type { YAxisScale } from "../lib/chartScale";
import type { MetricGroup } from "../lib/metricGroups";
import { SortableMetricGroup } from "./SortableMetricGroup";

const STAGE_COLORS: Record<string, string> = {
  train: RUN_COLORS[0],
  val: RUN_COLORS[1],
  test: RUN_COLORS[2],
};

/** Cards differ in height, so the drop follows the pointer; the keyboard has none and uses card centers. */
const dropTarget: CollisionDetection = (args) => {
  const underPointer = pointerWithin(args);
  return underPointer.length > 0 ? underPointer : closestCenter(args);
};

interface MetricChartGridProps {
  chartScales: Record<string, YAxisScale>;
  onScaleChange: (key: string, scale: YAxisScale) => void;
  trendCharts: Record<string, boolean>;
  onToggleTrend: (key: string) => void;
  onHideChart: (key: string) => void;
  events: RunEvent[];
  onEventSelect: (event: RunEvent) => void;
  visibleMetricKeys: string[];
  metricGroups: MetricGroup[];
  metricSearch: string;
  onToggleHiddenGroup: (id: string) => void;
  onReorderGroup: (active: string, over: string) => void;
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
  chartScales, onScaleChange, trendCharts, onToggleTrend, onHideChart, events, onEventSelect,
  visibleMetricKeys,
  metricGroups,
  metricSearch,
  onToggleHiddenGroup,
  onReorderGroup,
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

  const renderCharts = (keys: string[]) => (
    <DndContext sensors={sensors} collisionDetection={dropTarget} onDragEnd={handleDragEnd}>
      <SortableContext items={keys} strategy={rectSortingStrategy}>
        <div className={`grid-layout grid-layout--${widthMode}`}>
          {keys.map((groupKey) => {
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
                yAxisScale={chartScales[chartBaseName(groupKey)] ?? "linear"} onScaleChange={scale => onScaleChange(groupKey, scale)} onHide={() => onHideChart(groupKey)}
                showTrend={!!trendCharts[chartBaseName(groupKey)]} onToggleTrend={() => onToggleTrend(groupKey)}
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

  if (!metricGroups.length) return renderCharts(visibleMetricKeys);

  const assigned = new Set(metricGroups.flatMap(group => group.metrics));
  const ungrouped = visibleMetricKeys.filter(key => !assigned.has(chartBaseName(key)));
  const needle = metricSearch.trim().toLowerCase();
  const sections = metricGroups.map(group => ({
    group, keys: visibleMetricKeys.filter(key => group.metrics.includes(chartBaseName(key))),
  })).filter(({ group, keys }) => !needle || keys.length > 0 || group.name.toLowerCase().includes(needle));

  return (
    <div className="metric-group-stack">
      <DndContext sensors={sensors} collisionDetection={dropTarget} onDragEnd={({ active, over }) => {
        if (over && active.id !== over.id) onReorderGroup(String(active.id), String(over.id));
      }}>
        <SortableContext items={sections.map(({ group }) => group.id)} strategy={verticalListSortingStrategy}>
          {sections.map(({ group, keys }, index) => (
            <SortableMetricGroup key={group.id} group={group} chartCount={keys.length}
              canMoveUp={index > 0} canMoveDown={index < sections.length - 1}
              onToggleHidden={() => onToggleHiddenGroup(group.id)}
              onMoveUp={() => onReorderGroup(group.id, sections[index - 1].group.id)}
              onMoveDown={() => onReorderGroup(group.id, sections[index + 1].group.id)}>
              {keys.length ? renderCharts(keys) : <p className="metric-group-empty">No visible charts in the selected runs.</p>}
            </SortableMetricGroup>
          ))}
        </SortableContext>
      </DndContext>
      {ungrouped.length ? (
        <section className="metric-group" aria-label="Ungrouped metrics">
          <h2 className="ungrouped-metrics-title">Ungrouped</h2>
          {renderCharts(ungrouped)}
        </section>
      ) : null}
    </div>
  );
}
