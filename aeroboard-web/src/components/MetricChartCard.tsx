import { memo, useCallback, useMemo } from "react";
import { chartBaseName } from "../lib/dashboardDefaults";
import { RUN_COLORS } from "../lib/runColors";
import { useChartSettings } from "./chartSettingsContext";
import { SortableMetricChart } from "./SortableMetricChart";
import type { YAxisScale } from "../lib/chartScale";

const EMPTY_MARKERS: import("./LineChart").ChartMarker[]=[];
const STAGE_COLORS: Record<string, string> = { train: RUN_COLORS[0], val: RUN_COLORS[1], test: RUN_COLORS[2] };

export const MetricChartCard = memo(function MetricChartCard({ groupKey, targets }: { groupKey: string; targets: string[] }) {
  const { settings, actions, markers } = useChartSettings();
  const { currentMode, selectedRuns, getChartData, runColors } = settings;
  const seriesData = useMemo(() => {
    const entries = currentMode === "single"
      ? targets.map(metric => ({ run: selectedRuns[0], metric, name: metric.split("/")[0], color: STAGE_COLORS[metric.split("/")[0]] }))
      : selectedRuns.map(run => ({ run, metric: groupKey, name: run, color: runColors[run] }));
    return entries.flatMap(entry => {
      const fullData = getChartData(entry.run, entry.metric);
      return fullData ? [{ name: entry.name, fullData, color: entry.color }] : [];
    });
  }, [currentMode, targets, selectedRuns, getChartData, runColors, groupKey]);
  const onScale = useCallback((scale: YAxisScale) => actions.scale(groupKey, scale), [actions, groupKey]);
  const onTrend = useCallback(() => actions.trend(groupKey), [actions, groupKey]);
  const onHide = useCallback(() => actions.hide(groupKey), [actions, groupKey]);
  const onFavorite = useCallback(() => actions.favorite(groupKey), [actions, groupKey]);
  const onCollapse = useCallback(() => actions.collapse(groupKey), [actions, groupKey]);
  const onTarget = useCallback((value: number | null) => actions.target(groupKey, value), [actions, groupKey]);
  if (!seriesData.length) return null;
  const metric = chartBaseName(groupKey);
  return <SortableMetricChart
    id={groupKey} groupKey={groupKey} seriesData={seriesData}
    theme={settings.theme} xAxisType={settings.xAxisType} smoothing={settings.smoothing}
    removeOutliers={settings.removeOutliers} xRange={settings.xRange} widthMode={settings.widthMode}
    favorite={!!settings.favorites[groupKey]} onToggleFavorite={onFavorite}
    collapsed={settings.collapsed[metric] ?? !settings.favorites[groupKey]} onToggleCollapse={onCollapse}
    optimalValue={settings.optimalValues[groupKey] ?? null} onOptimalValueChange={onTarget}
    yAxisScale={settings.chartScales[metric] ?? "linear"} onScaleChange={onScale}
    showTrend={!!settings.trendCharts[metric]} onToggleTrend={onTrend} onHide={onHide}
    markers={markers.get(settings.xAxisType==="rel"?seriesData[0].fullData.wallTimes[0]:0) ?? EMPTY_MARKERS} onMarkerSelect={actions.event}
  />;
});
