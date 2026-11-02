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
): DashboardDefaults {
  const present = runConfigs.filter((config): config is PartialDashboardDefaults => config !== null);
  const resolveNames = (field: "starred" | "order") => {
    const lists = present.map((config) => config[field]).filter((list): list is string[] => list !== undefined);
    return lists.length > 0
      ? Array.from(new Set(lists.flat()))
      : serverDefaults?.[field] ?? EMPTY_DASHBOARD_DEFAULTS[field];
  };

  // Keep layers separate until after alias matching: a server's exact "loss"
  // must not beat a run's "val/loss" on the grouped loss chart (or vice versa).
  const targets: Record<string, number> = {};
  const layers = [...present, serverDefaults];
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

function chartBaseName(name: string): string {
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
