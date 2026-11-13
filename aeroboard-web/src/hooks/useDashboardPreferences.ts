import { flags as asRecord, names as asStringArray } from "../lib/controls";
import { usePersistentState } from "./usePersistentState";

export type WidthMode = "compact" | "wide" | "full";

const FAVORITES_STORAGE_KEY = "aeroboard-favorite-charts";
const COLLAPSED_STORAGE_KEY = "aeroboard-collapsed-charts";
const ORDER_STORAGE_KEY = "aeroboard-ordered-metrics";
const EXPERIMENTS_STORAGE_KEY = "aeroboard-collapsed-experiments";
const RUN_COLORS_STORAGE_KEY = "aeroboard-run-colors";
const OPTIMAL_VALUES_STORAGE_KEY = "aeroboard-optimal-values";

function asStringRecord(raw: unknown): Record<string, string> {
  if (!raw || typeof raw !== "object") return {};
  return Object.fromEntries(
    Object.entries(raw).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}

/** `null` records an explicit "no line" so a configured default target stays removed. */
function asOptionalNumberRecord(raw: unknown): Record<string, number | null> {
  if (!raw || typeof raw !== "object") return {};
  return Object.fromEntries(
    Object.entries(raw).filter(
      (entry): entry is [string, number | null] =>
        entry[1] === null || (typeof entry[1] === "number" && Number.isFinite(entry[1])),
    ),
  );
}

export function useDashboardPreferences(scope = "legacy") {
  const scoped = (key: string) => scope === "legacy" ? key : `${key}:${scope}`;
  const [favoriteCharts, setFavoriteCharts] = usePersistentState<Record<string, boolean>>(
    scoped(FAVORITES_STORAGE_KEY),
    {},
    asRecord,
  );
  const [collapsedCharts, setCollapsedCharts] = usePersistentState<Record<string, boolean>>(
    scoped(COLLAPSED_STORAGE_KEY),
    {},
    asRecord,
  );
  const [orderedMetrics, setOrderedMetrics] = usePersistentState<string[]>(
    scoped(ORDER_STORAGE_KEY),
    [],
    asStringArray,
  );
  const [collapsedExperiments, setCollapsedExperiments] = usePersistentState<Record<string, boolean>>(
    scoped(EXPERIMENTS_STORAGE_KEY),
    {},
    asRecord,
  );
  const [runColors, setRunColors] = usePersistentState<Record<string, string>>(
    scoped(RUN_COLORS_STORAGE_KEY),
    {},
    asStringRecord,
  );

  // User overrides for per-chart target ("optimal value") lines, keyed by chart group key.
  // Absent key = use the configured default; null = user removed the line.
  const [optimalValues, setOptimalValues] = usePersistentState<Record<string, number | null>>(
    scoped(OPTIMAL_VALUES_STORAGE_KEY),
    {},
    asOptionalNumberRecord,
  );

  /** Drop the user's chart overrides so the configured initial view applies again. */
  const resetChartView = () => {
    setFavoriteCharts({});
    setCollapsedCharts({});
    setOrderedMetrics([]);
    setOptimalValues({});
  };

  const hasChartOverrides =
    Object.keys(favoriteCharts).length > 0 ||
    Object.keys(collapsedCharts).length > 0 ||
    orderedMetrics.length > 0 ||
    Object.keys(optimalValues).length > 0;

  return {
    favoriteCharts,
    setFavoriteCharts,
    collapsedCharts,
    setCollapsedCharts,
    orderedMetrics,
    setOrderedMetrics,
    collapsedExperiments,
    setCollapsedExperiments,
    runColors,
    setRunColors,
    optimalValues,
    setOptimalValues,
    resetChartView,
    hasChartOverrides,
  };
}
