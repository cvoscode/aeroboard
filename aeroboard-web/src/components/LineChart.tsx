import { useEffect, useMemo, useRef, useState } from 'react';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';
import type { TimeSeriesData } from '../hooks/useAeroboard';
import { formatReferenceValue } from '../lib/format';

export type XAxisType = 'wall' | 'rel' | 'step';

interface LineChartProps {
    title: string;
    seriesData: { name: string; fullData: TimeSeriesData; color?: string }[];
    themeKey: 'light' | 'dark';
    xAxisType: XAxisType;
    smoothing?: number;
    removeOutliers?: boolean;
    xRange?: [number, number] | null;
    /** Optional horizontal reference line (e.g. the optimal/target value). */
    referenceValue?: number | null;
    height?: number;
}

const DEFAULT_COLORS = ['#00d2ff', '#ff4c6b', '#00e676', '#ffeb3b', '#e040fb'];

/** Dashed horizontal line drawn straight onto uPlot's canvas: no extra series or DOM. */
function drawReferenceLine(u: uPlot, value: number | null, color: string) {
    if (value == null || !Number.isFinite(value)) return;
    const { left, top, width, height } = u.bbox;
    const y = Math.round(u.valToPos(value, 'y', true)) + 0.5;
    if (!Number.isFinite(y) || y < top || y > top + height) return;

    const ctx = u.ctx;
    const dpr = uPlot.pxRatio;
    ctx.save();
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5 * dpr;
    ctx.setLineDash([6 * dpr, 4 * dpr]);
    ctx.beginPath();
    ctx.moveTo(left, y);
    ctx.lineTo(left + width, y);
    ctx.stroke();

    ctx.setLineDash([]);
    ctx.fillStyle = color;
    ctx.font = `600 ${11 * dpr}px system-ui, sans-serif`;
    ctx.textAlign = 'right';
    const label = `optimal ${formatReferenceValue(value)}`;
    const labelAbove = y - top > 16 * dpr;
    ctx.textBaseline = labelAbove ? 'bottom' : 'top';
    ctx.fillText(label, left + width - 4 * dpr, labelAbove ? y - 3 * dpr : y + 3 * dpr);
    ctx.restore();
}

const xRelFormat = (v: number) => {
    const h = Math.floor(v / 3600).toString().padStart(2, '0');
    const m = Math.floor((v % 3600) / 60).toString().padStart(2, '0');
    const s = Math.floor(v % 60).toString().padStart(2, '0');
    return `${h}:${m}:${s}`;
};

type ChartTheme = { axis: string; grid: string; reference: string };

function readChartTheme(): ChartTheme {
    if (typeof window === 'undefined') {
        return { axis: '#9ba1a6', grid: 'rgba(255,255,255,0.05)', reference: '#ffb300' };
    }
    const styles = getComputedStyle(document.documentElement);
    return {
        axis: styles.getPropertyValue('--plot-axis').trim() || '#9ba1a6',
        grid: styles.getPropertyValue('--plot-grid').trim() || 'rgba(255,255,255,0.05)',
        reference: styles.getPropertyValue('--plot-reference').trim() || '#ffb300',
    };
}

/** Stable identity for the mutable TimeSeriesData buffers, so a cleared store never aliases old data. */
const seriesIds = new WeakMap<TimeSeriesData, number>();
let nextSeriesId = 1;
function seriesId(data: TimeSeriesData): number {
    let id = seriesIds.get(data);
    if (id === undefined) {
        id = nextSeriesId++;
        seriesIds.set(data, id);
    }
    return id;
}

export function LineChart({
    title, seriesData, themeKey, xAxisType,
    smoothing = 0, removeOutliers = false,
    xRange = null,
    referenceValue = null,
    height = 250
}: LineChartProps) {
    const containerRef = useRef<HTMLDivElement>(null);
    const plotRef = useRef<uPlot | null>(null);
    const suppressSyncRef = useRef(false);  // prevent feedback loop
    const [localXRange, setLocalXRange] = useState<[number, number] | null>(null);
    const referenceValueRef = useRef<number | null>(referenceValue);
    referenceValueRef.current = referenceValue;

    const colorKey = seriesData.map(s => s.color ?? '').join(',');
    const hasSmoothingKey = smoothing > 0;
    // Which series are plotted: changing it resets the user's zoom.
    const seriesKey = seriesData.map(s => `${s.name}#${seriesId(s.fullData)}`).join('|');
    // What data is plotted: the store mutates buffers in place, so the point count is
    // the cheap change marker. Parents rebuild `seriesData` on every render, so keying
    // on this avoids reprocessing (and redrawing) when nothing actually changed.
    const dataSignature = seriesData.map(s => `${s.name}#${seriesId(s.fullData)}:${s.fullData.count}`).join('|');
    const axisRevisionKey = `${themeKey}:${xAxisType}:${removeOutliers ? 'trim' : 'full'}`;
    // CSS variables only change with the theme; reading them per render forces a style recalc.
    const chartTheme = useMemo(readChartTheme, [themeKey]);

    const seriesDataRef = useRef(seriesData);
    seriesDataRef.current = seriesData;

    const processedData = useMemo(
        () => buildProcessedData(seriesDataRef.current, xAxisType, smoothing, xRange),
        // eslint-disable-next-line react-hooks/exhaustive-deps
        [dataSignature, xAxisType, smoothing, xRange]
    );

    const focusedRange = useMemo(
        () => getFocusedRanges(processedData, removeOutliers, referenceValue),
        [processedData, removeOutliers, referenceValue]
    );
    const focusedRangeRef = useRef(focusedRange);
    focusedRangeRef.current = focusedRange;

    // A different set of series or a different axis invalidates any manual zoom.
    // Live points arriving for the same series keep it.
    useEffect(() => {
        setLocalXRange(null);
    }, [seriesKey, axisRevisionKey]);

    // Apply scale changes synchronously (uPlot otherwise commits, and fires its
    // setScale hook, in a microtask) so programmatic zooms aren't read back as user zooms.
    const applyQuietly = (plot: uPlot, fn: () => void) => {
        suppressSyncRef.current = true;
        try {
            plot.batch(fn);
        } finally {
            suppressSyncRef.current = false;
        }
    };

    // ── Effect 1: Create & destroy the uPlot instance ────────────────────────
    // Only structural changes rebuild the canvas; data updates go through Effect 2.
    useEffect(() => {
        const container = containerRef.current;
        if (!container || seriesData.length === 0) return;

        const xDateFormat = uPlot.fmtDate("{HH}:{mm}:{ss}");

        const series: uPlot.Series[] = [
            {
                label: xAxisType === 'wall' ? 'Wall' : xAxisType === 'rel' ? 'Rel' : 'Step',
                value: (_u, v) => {
                    if (v == null) return '--';
                    if (xAxisType === 'wall') return xDateFormat(new Date(v * 1000));
                    if (xAxisType === 'rel') return xRelFormat(v);
                    return v.toFixed(0);
                }
            }
        ];

        seriesData.forEach((s, i) => {
            const color = s.color || DEFAULT_COLORS[i % DEFAULT_COLORS.length];
            if (hasSmoothingKey) {
                series.push({
                    label: `${s.name} raw`,
                    stroke: color + '80',
                    width: 1,
                    spanGaps: true,
                    points: { show: false }
                });
            }
            series.push({
                label: s.name,
                stroke: color,
                width: 2.5,
                spanGaps: true,
                points: { show: false }
            });
        });

        const opts: uPlot.Options = {
            title,
            width: container.clientWidth,
            height,
            padding: [10, 10, 0, 10],
            scales: {
                y: {
                    auto: true,
                    range: (_u, min, max) => focusedRangeRef.current.focusedYRange ?? [min, max]
                }
            },
            axes: [
                {
                    stroke: chartTheme.axis,
                    grid: { stroke: chartTheme.grid, width: 1 },
                    space: 50,
                    values: xAxisType !== 'wall'
                        ? (_u, vals) => vals.map(v => v == null ? '' : xAxisType === 'rel' ? xRelFormat(v) : v.toFixed(0))
                        : undefined
                },
                {
                    stroke: chartTheme.axis,
                    grid: { stroke: chartTheme.grid, width: 1 },
                    space: 40
                }
            ],
            series,
            cursor: {
                drag: { setScale: true },
                points: { show: false },
                // Double-click resets via React state (below) instead of uPlot's
                // autoScaleX, which would ignore the outlier-trimmed default range.
                bind: { dblclick: () => null },
            },
            select: {
                show: true,
                over: true,
                left: 0,
                width: 0,
                top: 0,
                height: 0,
            },
            legend: { show: false },
            hooks: {
                draw: [u => drawReferenceLine(u, referenceValueRef.current, chartTheme.reference)],
                ready: [
                    u => {
                        u.over.addEventListener('dblclick', () => setLocalXRange(null));
                    }
                ],
                setScale: [
                    (u, key) => {
                        // status 0 = initial layout, not a user action.
                        if (key !== 'x' || suppressSyncRef.current || u.status === 0) return;
                        const { min, max } = u.scales.x;
                        if (min != null && max != null) setLocalXRange([min, max]);
                    }
                ]
            }
        };

        const plot = new uPlot(opts, processedData.alignedData, container);
        plotRef.current = plot;

        // Track the container, not the window: the sidebar and width-mode toggles
        // resize charts without any window resize event.
        let lastWidth = container.clientWidth;
        const observer = new ResizeObserver(() => {
            const width = container.clientWidth;
            if (width <= 0 || width === lastWidth) return;
            lastWidth = width;
            applyQuietly(plot, () => plot.setSize({ width, height }));
        });
        observer.observe(container);
        return () => {
            observer.disconnect();
            plot.destroy();
            plotRef.current = null;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [title, seriesData.length, colorKey, chartTheme, xAxisType, height, hasSmoothingKey, axisRevisionKey]);

    // ── Effect 2: Push data and ranges into the existing instance ────────────
    // Also covers reference-line edits: focusedRange depends on referenceValue, and
    // the synchronous batch commit repaints (running the draw hook).
    useEffect(() => {
        const plot = plotRef.current;
        if (!plot || seriesData.length === 0) return;

        // Priority: global window slider > user's manual zoom > outlier focus > full extent.
        const preferred = xRange || localXRange || (removeOutliers ? focusedRange.focusedXRange : null) || focusedRange.defaultXRange;

        applyQuietly(plot, () => {
            plot.setData(processedData.alignedData, false); // keep scales; set below
            if (preferred && preferred[0] !== preferred[1]) {
                plot.setScale('x', { min: preferred[0], max: preferred[1] });
            }
            if (focusedRange.focusedYRange) {
                plot.setScale('y', { min: focusedRange.focusedYRange[0], max: focusedRange.focusedYRange[1] });
            }
        });
    }, [processedData, focusedRange, removeOutliers, xRange, localXRange, seriesData.length, title, colorKey, chartTheme, height, hasSmoothingKey, axisRevisionKey]);

    return <div key={axisRevisionKey} ref={containerRef} className="uplot-wrapper" />;
}

// ────────────────────────────────────────────────────────────────────────────
// Data Processing
// ────────────────────────────────────────────────────────────────────────────
interface ProcessedSeries {
    x: number[];
    raw: number[];
    smooth: number[];
}

interface ProcessedChartData {
    processed: ProcessedSeries[];
    alignedData: uPlot.AlignedData;
}

function buildProcessedData(
    seriesData: { fullData: TimeSeriesData }[],
    xAxis: XAxisType,
    smoothing: number,
    xRange: [number, number] | null
): ProcessedChartData {
    if (seriesData.length === 0) return { processed: [], alignedData: [[]] };

    const processed = seriesData.map(s => {
        const { wallTimes, relTimes, values, count: n } = s.fullData;
        const xArr = xAxis === 'wall' ? wallTimes : xAxis === 'rel' ? relTimes : s.fullData.plotX;

        const resX: number[] = [];
        const rawV: number[] = [];
        const smoothV: number[] = [];
        let ema = 0, first = true;

        for (let i = 0; i < n; i++) {
            const x = xArr[i];
            const v = values[i];
            if (xAxis === 'step' && x < 0) continue;
            resX.push(x);
            rawV.push(v);
            if (smoothing > 0) {
                if (first) { ema = v; first = false; } else { ema = ema * smoothing + v * (1 - smoothing); }
                smoothV.push(ema);
            }
        }
        return { x: resX, raw: rawV, smooth: smoothV };
    });

    // Merge + sort all X values
    const xSet = new Set<number>();
    for (const p of processed) for (const x of p.x) xSet.add(x);
    const allX = Array.from(xSet).sort((a, b) => a - b);

    // Apply window filter on the unified axis
    const filteredX = xRange
        ? allX.filter(x => x >= xRange[0] && x <= xRange[1])
        : allX;

    if (filteredX.length === 0) return { processed, alignedData: [filteredX] };

    const result: uPlot.AlignedData = [filteredX];
    for (const p of processed) {
        const xToIdx = new Map<number, number>();
        for (let i = 0; i < p.x.length; i++) xToIdx.set(p.x[i], i);

        const alignedRaw = filteredX.map(x => { const i = xToIdx.get(x); return i !== undefined ? p.raw[i] : null; });
        const alignedSmooth = smoothing > 0
            ? filteredX.map(x => { const i = xToIdx.get(x); return i !== undefined ? p.smooth[i] : null; })
            : null;

        if (alignedSmooth) result.push(alignedRaw as number[]);
        result.push((alignedSmooth ?? alignedRaw) as number[]);
    }
    return { processed, alignedData: result };
}

function getFocusedRanges(
    processedData: ProcessedChartData,
    removeOutliers: boolean,
    referenceValue: number | null = null
): { defaultXRange: [number, number] | null; focusedXRange: [number, number] | null; focusedYRange: [number, number] | null } {
    const visiblePoints: { x: number; y: number }[] = [];

    for (const series of processedData.processed) {
        for (let i = 0; i < series.x.length; i++) {
            visiblePoints.push({
                x: series.x[i],
                y: series.smooth[i] ?? series.raw[i],
            });
        }
    }

    if (visiblePoints.length === 0) {
        return { defaultXRange: null, focusedXRange: null, focusedYRange: null };
    }

    let minAllX = Infinity;
    let maxAllX = -Infinity;
    for (const point of visiblePoints) {
        if (point.x < minAllX) minAllX = point.x;
        if (point.x > maxAllX) maxAllX = point.x;
    }
    const defaultXRange: [number, number] = [minAllX, maxAllX];

    // Sorting is only needed to trim outliers; the default path is a single pass.
    let clippedPoints = visiblePoints;
    if (removeOutliers) {
        const sortedY = [...visiblePoints].sort((a, b) => a.y - b.y);
        const lowIdx = Math.floor((sortedY.length - 1) * 0.1);
        const highIdx = Math.ceil((sortedY.length - 1) * 0.9);
        clippedPoints = sortedY.slice(lowIdx, highIdx + 1);
    }

    if (clippedPoints.length === 0) {
        return { defaultXRange, focusedXRange: defaultXRange, focusedYRange: null };
    }

    let minY = Infinity;
    let maxY = -Infinity;
    let minX = Infinity;
    let maxX = -Infinity;
    for (const point of clippedPoints) {
        if (point.y < minY) minY = point.y;
        if (point.y > maxY) maxY = point.y;
        if (point.x < minX) minX = point.x;
        if (point.x > maxX) maxX = point.x;
    }
    // Keep the reference line in view even when it lies outside the data.
    if (referenceValue != null && Number.isFinite(referenceValue)) {
        minY = Math.min(minY, referenceValue);
        maxY = Math.max(maxY, referenceValue);
    }

    const yPad = minY === maxY ? (Math.abs(minY || 1) * 0.05 || 1) : (maxY - minY) * (removeOutliers ? 0.03 : 0.08);
    const focusedYRange: [number, number] = [minY - yPad, maxY + yPad];

    return {
        defaultXRange,
        focusedXRange: removeOutliers ? [minX, maxX] : defaultXRange,
        focusedYRange,
    };
}
