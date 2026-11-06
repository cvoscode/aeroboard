export interface LinearTrend {
  minX: number;
  maxX: number;
  slope: number;
  predict: (x: number) => number;
}

/** Least-squares fit in original units, centered to preserve small time/step differences. */
export function fitLinearTrend(
  x: ArrayLike<number>,
  y: ArrayLike<number>,
  {xRange, yRange}: {xRange?: [number, number] | null; yRange?: [number, number] | null} = {},
): LinearTrend | null {
  const length = Math.min(x.length, y.length);
  const valid = (i: number) => Number.isFinite(x[i]) && Number.isFinite(y[i])
    && (!xRange || x[i] >= xRange[0] && x[i] <= xRange[1])
    && (!yRange || y[i] >= yRange[0] && y[i] <= yRange[1]);
  let count = 0;
  let origin = 0;
  let meanX = 0;
  let meanY = 0;
  let minX = Infinity;
  let maxX = -Infinity;
  for (let i = 0; i < length; i++) {
    if (!valid(i)) continue;
    if (count === 0) origin = x[i];
    count++;
    meanX += (x[i] - origin - meanX) / count;
    meanY += (y[i] - meanY) / count;
    minX = Math.min(minX, x[i]);
    maxX = Math.max(maxX, x[i]);
  }
  if (count < 2 || minX === maxX) return null;
  let variance = 0;
  let covariance = 0;
  for (let i = 0; i < length; i++) {
    if (!valid(i)) continue;
    const dx = x[i] - origin - meanX;
    variance += dx * dx;
    covariance += dx * (y[i] - meanY);
  }
  const slope = covariance / variance;
  if (!Number.isFinite(slope) || !Number.isFinite(meanY)) return null;
  const predict = (value: number) => meanY + slope * (value - origin - meanX);
  if (!Number.isFinite(predict(minX)) || !Number.isFinite(predict(maxX))) return null;
  return {minX, maxX, slope, predict};
}
