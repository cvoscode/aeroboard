export interface ChartSeries {
  x: number[];
  raw: number[];
  smooth: number[];
}

export interface ChartRanges {
  defaultXRange: [number, number] | null;
  focusedXRange: [number, number] | null;
  focusedYRange: [number, number] | null;
}

interface Point { x: number; y: number }

/** Finite points of one line, in drawing order. */
function linePoints(x: number[], y: number[]): Point[] {
  const points: Point[] = [];
  for (let i = 0; i < y.length; i++) {
    if (Number.isFinite(y[i])) points.push({x: x[i], y: y[i]});
  }
  return points;
}

/**
 * What a line shows inside an x window: its points there, plus its height where it
 * crosses each edge. Without the crossings, sparsely logged metrics get clipped.
 * Log axes draw straight lines in log space, so interpolate there.
 */
export function pointsInWindow(line: Point[], [low, high]: [number, number], logarithmic = false): Point[] {
  const interpolate = (a: Point, b: Point, x: number) => {
    const t = (x - a.x) / (b.x - a.x);
    return logarithmic ? Math.exp(Math.log(a.y) + t * (Math.log(b.y) - Math.log(a.y))) : a.y + t * (b.y - a.y);
  };
  const visible: Point[] = [];
  line.forEach((point, index) => {
    if (point.x >= low && point.x <= high) visible.push(point);
    const previous = line[index - 1];
    if (!previous) return;
    for (const edge of [low, high]) {
      if ((previous.x < edge && point.x > edge) || (previous.x > edge && point.x < edge)) {
        visible.push({x: edge, y: interpolate(previous, point, edge)});
      }
    }
  });
  return visible;
}

/**
 * Axis ranges for a chart. Pass `viewRange` when the user has zoomed: the y-range then
 * fits what is on screen, including raw lines behind smoothing. A target line outside
 * that view is left off-screen, because keeping it would undo the zoom on any chart
 * whose target sits far from its data.
 */
export function getFocusedRanges(
  series: ChartSeries[],
  removeOutliers: boolean,
  referenceValue: number | null = null,
  logarithmic = false,
  viewRange: [number, number] | null = null,
): ChartRanges {
  const allPoints: Point[] = [];
  for (const line of series) {
    for (let i = 0; i < line.x.length; i++) {
      const y = line.smooth[i] ?? line.raw[i];
      if (Number.isFinite(y)) allPoints.push({x: line.x[i], y});
    }
  }

  if (allPoints.length === 0) {
    return {defaultXRange: null, focusedXRange: null, focusedYRange: null};
  }

  let minAllX = Infinity;
  let maxAllX = -Infinity;
  for (const point of allPoints) {
    if (point.x < minAllX) minAllX = point.x;
    if (point.x > maxAllX) maxAllX = point.x;
  }
  const defaultXRange: [number, number] = [minAllX, maxAllX];

  const zoomed = viewRange !== null && (viewRange[0] > minAllX || viewRange[1] < maxAllX);
  const windowed = zoomed
    ? series.flatMap(line => [line.raw, line.smooth].flatMap(values => pointsInWindow(linePoints(line.x, values), viewRange, logarithmic)))
    : [];
  // A window between two samples of every line has nothing to fit to.
  const fitted = windowed.length > 0;
  let points = fitted ? windowed : allPoints;

  // Sorting is only needed to trim outliers; the default path is a single pass.
  if (removeOutliers) {
    const sortedY = [...points].sort((a, b) => a.y - b.y);
    const lowIdx = Math.floor((sortedY.length - 1) * 0.1);
    const highIdx = Math.ceil((sortedY.length - 1) * 0.9);
    points = sortedY.slice(lowIdx, highIdx + 1);
  }

  let minY = Infinity;
  let maxY = -Infinity;
  let minX = Infinity;
  let maxX = -Infinity;
  for (const point of points) {
    if (point.y < minY) minY = point.y;
    if (point.y > maxY) maxY = point.y;
    if (point.x < minX) minX = point.x;
    if (point.x > maxX) maxX = point.x;
  }
  // Keep the reference line in view even when it lies outside the data.
  if (!fitted && referenceValue != null && Number.isFinite(referenceValue)) {
    minY = Math.min(minY, referenceValue);
    maxY = Math.max(maxY, referenceValue);
  }

  const yPad = minY === maxY ? (Math.abs(minY || 1) * 0.05 || 1) : (maxY - minY) * (removeOutliers ? 0.03 : 0.08);
  const logPad = minY === maxY ? 1.1 : Math.pow(maxY / minY, 0.08);
  const focusedYRange: [number, number] = logarithmic ? [minY / logPad, maxY * logPad] : [minY - yPad, maxY + yPad];

  return {
    defaultXRange,
    focusedXRange: removeOutliers ? [minX, maxX] : defaultXRange,
    focusedYRange,
  };
}
