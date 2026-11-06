export type YAxisScale = "linear" | "log" | "exponential";

export function parseChartScales(raw: unknown): Record<string, YAxisScale> {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return {};
  return Object.fromEntries(Object.entries(raw).filter((entry): entry is [string, YAxisScale] =>
    entry[1] === "linear" || entry[1] === "log" || entry[1] === "exponential"));
}

/**
 * An exponential axis gives small differences at the high end more screen space.
 * Normalize to the displayed range first, so values such as 1e6 never overflow exp().
 * Linear tails keep outliers outside the displayed range finite and invertible.
 * Data stays in its original units; only the value-to-pixel mapping changes.
 */
export function createExponentialTransform(min: number, max: number) {
  if (!Number.isFinite(min) || !Number.isFinite(max) || min >= max) {
    return {fwd: (value: number) => value, bwd: (value: number) => value};
  }
  const span = max - min;
  const normalize = Number.isFinite(span)
    ? (value: number) => (value - min) / span
    : (value: number) => (value / 2 - min / 2) / (max / 2 - min / 2);
  const restore = Number.isFinite(span)
    ? (value: number) => min + value * span
    : (value: number) => (1 - value) * min + value * max;
  const strength = 4;
  const extent = Math.expm1(strength);
  const lowerSlope = strength / extent;
  const upperSlope = strength * Math.exp(strength) / extent;
  return {
    fwd(value: number) {
      const x = normalize(value);
      if (x < 0) return x * lowerSlope;
      if (x > 1) return 1 + (x - 1) * upperSlope;
      return Math.expm1(strength * x) / extent;
    },
    bwd(value: number) {
      const x = value < 0 ? value / lowerSlope
        : value > 1 ? 1 + (value - 1) / upperSlope
        : Math.log1p(value * extent) / strength;
      return restore(x);
    },
  };
}

/**
 * Log-axis ticks that stay readable at any zoom. Decade ticks suit wide ranges, but a
 * range inside one decade may contain none, so that case uses evenly stepped round values.
 */
export function logAxisTicks(min: number, max: number): number[] {
  if (!(min > 0) || !(max > min) || !Number.isFinite(max)) return [];
  const decades = Math.log10(max / min);
  const ticks: number[] = [];
  if (decades < 1) {
    const roughStep = (max - min) / 5;
    const magnitude = 10 ** Math.floor(Math.log10(roughStep));
    const step = [1, 2, 5, 10].find(multiple => multiple * magnitude >= roughStep)! * magnitude;
    for (let k = Math.ceil(min / step); k * step <= max; k++) ticks.push(k * step);
    return ticks;
  }
  const multiples = decades < 2 ? [1, 2, 5] : [1];
  const stride = Math.max(1, Math.ceil(decades / 8));
  for (let exponent = Math.floor(Math.log10(min)); exponent <= Math.ceil(Math.log10(max)); exponent += stride) {
    for (const multiple of multiples) {
      const tick = multiple * 10 ** exponent;
      if (tick >= min && tick <= max) ticks.push(tick);
    }
  }
  return ticks;
}
