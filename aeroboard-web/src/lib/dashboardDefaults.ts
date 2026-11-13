import type { HyperparameterEntry, RunSummary } from "../hooks/aeroboard/types";

/**
 * Initial dashboard view: which charts are starred, their order, and target lines.
 *
 * Layers, lowest to highest priority:
 *   1. server `aeroboard.toml` `[dashboard]` table (GET /api/config)
 *   2. per-run settings from the training script (`configure_dashboard` / `init(dashboard=...)`)
 *   3. whatever the user changes in the UI (localStorage)
 */
export interface DashboardDefaults {
  starred: string[];
  order: string[];
  targets: Record<string, number>;
}

export type PartialDashboardDefaults = Partial<DashboardDefaults>;

/** Reserved hyperparameter key the Python client uses to ship per-run settings. */
export const DASHBOARD_CONFIG_KEY = "__aeroboard_dashboard__";

export const EMPTY_DASHBOARD_DEFAULTS: DashboardDefaults = { starred: [], order: [], targets: {} };

function asNameList(raw: unknown): string[] | undefined {
  if (!Array.isArray(raw)) return undefined;
  return raw.filter((entry): entry is string => typeof entry === "string" && entry.trim() !== "");
}

function asTargets(raw: unknown): Record<string, number> | undefined {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return undefined;
  return Object.fromEntries(
    Object.entries(raw).filter((entry): entry is [string, number] => typeof entry[1] === "number" && Number.isFinite(entry[1])),
  );
}

/** Tolerant parse: keeps only the fields that are present and well-formed. */
export function parseDashboardDefaults(raw: unknown): PartialDashboardDefaults {
  if (!raw || typeof raw !== "object") return {};
  const source = raw as Record<string, unknown>;
  const result: PartialDashboardDefaults = {};
  const starred = asNameList(source.starred);
  const order = asNameList(source.order);
  const targets = asTargets(source.targets);
  if (starred) result.starred = starred;
  if (order) result.order = order;
  if (targets) result.targets = targets;
  return result;
}

/** Parse the JSON value of the reserved hyperparameter entry. */
export function parseRunDashboardConfigValue(value: string): PartialDashboardDefaults | null {
  try {
    return parseDashboardDefaults(JSON.parse(value));
  } catch {
    return null;
  }
}

/** True for the reserved entry, which must stay out of hyperparameter views. */
export function isDashboardConfigEntry(entry: Pick<HyperparameterEntry, "key">): boolean {
  return entry.key === DASHBOARD_CONFIG_KEY;
}

/** Split a run summary into its real hyperparameters and its dashboard settings. */
export function extractRunDashboardConfig(summary: RunSummary): { summary: RunSummary; config: PartialDashboardDefaults | null } {
  const entry = summary.hyperparameters.find(isDashboardConfigEntry);
  if (!entry) return { summary, config: null };
  return {
    summary: { ...summary, hyperparameters: summary.hyperparameters.filter((item) => !isDashboardConfigEntry(item)) },
    config: parseRunDashboardConfigValue(entry.value),
  };
}

/**
 * Resolve each layer against the visible chart keys before applying precedence.
 * Run lists replace server lists (and are unioned in compare mode). For each
 * chart's target, the first selected run with a match wins, then the server.
 */
export function resolveDashboardDefaults(
  serverDefaults: PartialDashboardDefaults | null,
  runConfigs: (PartialDashboardDefaults | null)[],
  groupKeys: string[],
  view?: PartialDashboardDefaults,
): DashboardDefaults {
  const present = runConfigs.filter((config): config is PartialDashboardDefaults => config !== null);
  const resolveNames = (field: "starred" | "order") => {
    if (view?.[field]) return view[field];
    const lists = present.map((config) => config[field]).filter((list): list is string[] => list !== undefined);
    return lists.length > 0
      ? Array.from(new Set(lists.flat()))
      : serverDefaults?.[field] ?? EMPTY_DASHBOARD_DEFAULTS[field];
  };

  // Keep layers separate until after alias matching: a server's exact "loss"
  // must not beat a run's "val/loss" on the grouped loss chart (or vice versa).
  const targets: Record<string, number> = {};
  const layers = [view, ...present, serverDefaults];
  for (const key of groupKeys) {
    for (const layer of layers) {
      const value = targetForChart(layer?.targets, key);
      if (value !== null) {
        targets[key] = value;
        break;
      }
    }
  }

  return { starred: resolveNames("starred"), order: resolveNames("order"), targets };
}

export function chartBaseName(name: string): string {
  const parts = name.split("/");
  return parts.length > 1 ? parts[1] : parts[0];
}

/**
 * Does a configured metric name refer to this chart? Single-run charts are keyed by
 * base name ("loss" groups train/loss + val/loss); compare charts by full name.
 * Configs may use either form.
 */
export function matchesChart(configuredName: string, groupKey: string): boolean {
  return configuredName === groupKey || chartBaseName(configuredName) === groupKey || chartBaseName(groupKey) === configuredName;
}

/**
 * Your chart settings belong to the metric, so single-run and compare mode share them.
 * Entries stored under a full name predate that, so a chart-name entry wins over them.
 */
export function settingsByMetric<T>(settings: Record<string, T>): Record<string, T> {
  const entries = Object.entries(settings);
  return Object.fromEntries([
    ...entries.filter(([name]) => chartBaseName(name) !== name).map(([name, value]) => [chartBaseName(name), value] as const),
    ...entries.filter(([name]) => chartBaseName(name) === name),
  ]);
}

export function uniqueMetrics(names: string[]): string[] {
  return Array.from(new Set(names.map(chartBaseName)));
}

/** Move a chart to the position of the one it was dropped on. Other entries keep their place. */
export function moveChart(order: string[], active: string, over: string): string[] {
  const from = order.indexOf(chartBaseName(active));
  const to = order.indexOf(chartBaseName(over));
  if (from < 0 || to < 0 || from === to) return order;
  const next = [...order];
  next.splice(to, 0, ...next.splice(from, 1));
  return next;
}

/** Place a chart straight after the last pinned chart. */
export function pinChart(order: string[], chart: string, pinned: string[]): string[] {
  const name = chartBaseName(chart);
  const next = order.filter((entry) => entry !== name);
  const lastPinned = next.reduce((last, entry, index) => (pinned.includes(entry) ? index : last), -1);
  next.splice(lastPinned + 1, 0, name);
  return next;
}

export function isStarredByDefault(defaults: DashboardDefaults, groupKey: string): boolean {
  return defaults.starred.some((name) => matchesChart(name, groupKey));
}

function targetForChart(targets: Record<string, number> | undefined, groupKey: string): number | null {
  if (!targets) return null;
  if (Object.hasOwn(targets, groupKey)) return targets[groupKey];
  const match = Object.keys(targets).find((name) => matchesChart(name, groupKey));
  return match === undefined ? null : targets[match];
}

/** Position in the configured order; unlisted charts sort after listed ones. */
export function defaultOrderRank(defaults: DashboardDefaults, groupKey: string): number {
  const index = defaults.order.findIndex((name) => matchesChart(name, groupKey));
  return index < 0 ? Number.POSITIVE_INFINITY : index;
}

/** Resolve server -> run -> saved view -> local choices in one pure operation. */
export function resolveChartPreferences({
  keys, server, runs, view, favorites, targets, removedTargets, order,
}: {
  keys: string[];
  server: PartialDashboardDefaults | null;
  runs: (PartialDashboardDefaults | null)[];
  view: PartialDashboardDefaults;
  favorites: Record<string, boolean>;
  targets: Record<string, number | null>;
  removedTargets: string[];
  order: string[];
}) {
  const defaults = resolveDashboardDefaults(server, runs, keys, view);
  const localFavorites = settingsByMetric(favorites);
  const localTargets = settingsByMetric(targets);
  const effectiveFavoriteCharts: Record<string, boolean> = {};
  const effectiveOptimalValues: Record<string, number | null> = {};
  for (const key of keys) {
    const metric = chartBaseName(key);
    effectiveFavoriteCharts[key] = localFavorites[metric] ?? isStarredByDefault(defaults, key);
    effectiveOptimalValues[key] = metric in localTargets ? localTargets[metric]
      : removedTargets.some(name => matchesChart(name, key)) ? null : defaults.targets[key] ?? null;
  }
  const savedOrder = uniqueMetrics(order.length ? order : view.order ?? []);
  const positions = new Map(savedOrder.map((name, index) => [name, index]));
  const position = (key: string) => positions.get(chartBaseName(key)) ?? Infinity;
  const placed = keys.filter(key => positions.has(chartBaseName(key)))
    .sort((a, b) => position(a) - position(b) || a.localeCompare(b));
  const unplaced = keys.filter(key => !positions.has(chartBaseName(key)))
    .sort((a, b) => defaultOrderRank(defaults, a) - defaultOrderRank(defaults, b) || a.localeCompare(b));
  const orderedMetricKeys = [...placed, ...unplaced.filter(key => effectiveFavoriteCharts[key]), ...unplaced.filter(key => !effectiveFavoriteCharts[key])];
  return { effectiveFavoriteCharts, effectiveOptimalValues, savedOrder, orderedMetricKeys, chartOrder: uniqueMetrics([...savedOrder, ...orderedMetricKeys]) };
}
