import { uniqueMetrics } from "./dashboardDefaults.ts";

export interface MetricGroup {
  id: string;
  name: string;
  metrics: string[];
  hidden: boolean;
}

/** Each shared metric belongs to one group, across single-run and compare mode. */
export function parseMetricGroups(raw: unknown): MetricGroup[] {
  if (!Array.isArray(raw)) return [];
  const ids = new Set<string>();
  const assigned = new Set<string>();
  const groups: MetricGroup[] = [];
  for (const value of raw) {
    if (!value || typeof value !== "object") continue;
    const { id, name, metrics, hidden } = value;
    if (typeof id !== "string" || !id.trim() || ids.has(id) || typeof name !== "string" || !name.trim()) continue;
    const members = uniqueMetrics(Array.isArray(metrics) ? metrics.filter((metric): metric is string => typeof metric === "string" && !!metric.trim()) : [])
      .filter(metric => !assigned.has(metric));
    ids.add(id);
    members.forEach(metric => assigned.add(metric));
    groups.push({ id, name: name.trim(), metrics: members, hidden: hidden === true });
  }
  return groups;
}

/** Assigning a metric transfers it out of its previous group. */
export function saveMetricGroup(groups: MetricGroup[], group: MetricGroup): MetricGroup[] {
  const normalized = parseMetricGroups([group])[0];
  if (!normalized) return groups;
  const next = groups.map(existing => existing.id === normalized.id ? normalized : {
    ...existing, metrics: existing.metrics.filter(metric => !normalized.metrics.includes(metric)),
  });
  if (!groups.some(existing => existing.id === normalized.id)) next.push(normalized);
  return parseMetricGroups(next);
}

export function moveMetricGroup(groups: MetricGroup[], active: string, over: string): MetricGroup[] {
  const from = groups.findIndex(group => group.id === active);
  const to = groups.findIndex(group => group.id === over);
  if (from < 0 || to < 0 || from === to) return groups;
  const next = [...groups];
  next.splice(to, 0, ...next.splice(from, 1));
  return next;
}
