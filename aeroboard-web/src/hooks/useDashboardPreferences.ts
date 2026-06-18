import { usePersistentState } from "./usePersistentState";

export type WidthMode = "compact" | "wide" | "full";

const WIDTH_STORAGE_KEY = "aeroboard-width-mode";
const FAVORITES_STORAGE_KEY = "aeroboard-favorite-charts";
const COLLAPSED_STORAGE_KEY = "aeroboard-collapsed-charts";
const ORDER_STORAGE_KEY = "aeroboard-ordered-metrics";
const EXPERIMENTS_STORAGE_KEY = "aeroboard-collapsed-experiments";
const RUN_COLORS_STORAGE_KEY = "aeroboard-run-colors";

function asRecord(raw: unknown): Record<string, boolean> {
  if (!raw || typeof raw !== "object") return {};
  return Object.fromEntries(
    Object.entries(raw).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"),
  );
}

function asStringRecord(raw: unknown): Record<string, string> {
  if (!raw || typeof raw !== "object") return {};
  return Object.fromEntries(
    Object.entries(raw).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}

function asStringArray(raw: unknown): string[] {
  return Array.isArray(raw) ? raw.filter((entry): entry is string => typeof entry === "string") : [];
}

function asWidthMode(raw: unknown): WidthMode {
  return raw === "compact" || raw === "wide" || raw === "full" ? raw : "wide";
}

export function useDashboardPreferences() {
  const [widthMode, setWidthMode] = usePersistentState<WidthMode>(WIDTH_STORAGE_KEY, "wide", asWidthMode);
  const [favoriteCharts, setFavoriteCharts] = usePersistentState<Record<string, boolean>>(
    FAVORITES_STORAGE_KEY,
    {},
    asRecord,
  );
  const [collapsedCharts, setCollapsedCharts] = usePersistentState<Record<string, boolean>>(
    COLLAPSED_STORAGE_KEY,
    {},
    asRecord,
  );
  const [orderedMetrics, setOrderedMetrics] = usePersistentState<string[]>(
    ORDER_STORAGE_KEY,
    [],
    asStringArray,
  );
  const [collapsedExperiments, setCollapsedExperiments] = usePersistentState<Record<string, boolean>>(
    EXPERIMENTS_STORAGE_KEY,
    {},
    asRecord,
  );
  const [runColors, setRunColors] = usePersistentState<Record<string, string>>(
    RUN_COLORS_STORAGE_KEY,
    {},
    asStringRecord,
  );

  return {
    widthMode,
    setWidthMode,
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
  };
}
