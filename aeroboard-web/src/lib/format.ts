/** Compact, stable label for a user-entered reference value (e.g. 0.95, 1e-4). */
export function formatReferenceValue(value: number): string {
  return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(6)));
}

/** Short elapsed time for dense lists: "now", "12m", "3h", "5d", then a date. */
export function formatAge(timestampMs: number, nowMs: number): string {
  const minutes = Math.floor((nowMs - timestampMs) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  return new Date(timestampMs).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
