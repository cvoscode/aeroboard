/** Compact, stable label for a user-entered reference value (e.g. 0.95, 1e-4). */
export function formatReferenceValue(value: number): string {
  return Number.isInteger(value) ? String(value) : String(Number(value.toPrecision(6)));
}
