import { useEffect, useMemo, useRef, useState } from 'react';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';
import type { TimeSeriesData } from '../hooks/useAeroboard';
import { getFocusedRanges, type ChartSeries } from '../lib/chartRanges';
import { RUN_COLORS } from '../lib/runColors';
import { formatReferenceValue } from '../lib/format';
import { createExponentialTransform, logAxisTicks, type YAxisScale } from '../lib/chartScale';
import { fitLinearTrend, type LinearTrend } from '../lib/linearTrend';

export type XAxisType = 'wall' | 'rel' | 'step';
export interface ChartMarker { id: string; x: number; label: string }

interface LineChartProps {
    yAxisScale?: YAxisScale;
    showTrend?: boolean;
    markers?: ChartMarker[];
    onMarkerSelect?: (id: string) => void;
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

const stepFormat = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });

/** Evaluate the fitted model between samples so nonlinear scales display the correct curve. */
function buildTrendPath(u: uPlot, fit: LinearTrend | null): uPlot.Series.Paths | null {
    if (!fit) return null;
    const min = Math.max(fit.minX, u.scales.x.min ?? fit.minX);
    const max = Math.min(fit.maxX, u.scales.x.max ?? fit.maxX);
    if (min >= max) return null;
    const steps = u.scales.y.distr === 1 ? 1 : Math.max(16, Math.ceil(u.bbox.width / uPlot.pxRatio / 8));
    const stroke = new Path2D();
    let drawing = false;
    for (let i = 0; i <= steps; i++) {
        const x = min + (max - min) * i / steps;
        const y = fit.predict(x);
        const px = u.valToPos(x, 'x', true);
        const py = u.valToPos(y, 'y', true);
        if (!Number.isFinite(py) || u.scales.y.distr === 3 && y <= 0) { drawing = false; continue; }
        if (drawing) stroke.lineTo(px, py); else stroke.moveTo(px, py);
        drawing = true;
    }
    const clip = new Path2D();
    clip.rect(u.bbox.left, u.bbox.top, u.bbox.width, u.bbox.height);
    return {stroke, clip};
}

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
    ctx.font = `700 ${11 * dpr}px ${CHART_FONT_FAMILY}`;
    ctx.textAlign = 'right';
    const label = `optimal ${formatReferenceValue(value)}`;
    const labelAbove = y - top > 16 * dpr;
    ctx.textBaseline = labelAbove ? 'bottom' : 'top';
    ctx.fillText(label, left + width - 4 * dpr, labelAbove ? y - 3 * dpr : y + 3 * dpr);
    ctx.restore();
}

const CHART_FONT_FAMILY = "'B612', system-ui, sans-serif";

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
    yAxisScale = 'linear', showTrend = false, markers = [], onMarkerSelect,
    title, seriesData, themeKey, xAxisType,
    smoothing = 0, removeOutliers = false,
    xRange = null,
    referenceValue = null,
    height = 250
}: LineChartProps) {
    const logarithmic = yAxisScale === 'log';
    const containerRef = useRef<HTMLDivElement>(null);
    const plotRef = useRef<uPlot | null>(null);
    const suppressSyncRef = useRef(false);  // prevent feedback loop
    const [localXRange, setLocalXRange] = useState<[number, number] | null>(null);
    const referenceValueRef = useRef<number | null>(referenceValue);
    referenceValueRef.current = logarithmic && referenceValue !== null && referenceValue <= 0 ? null : referenceValue;
    const markersRef = useRef(markers); markersRef.current = markers;
    const markerActionRef = useRef(onMarkerSelect); markerActionRef.current = onMarkerSelect;
    const markerSignature = markers.map(marker => `${marker.id}:${marker.x}:${marker.label}`).join("|");

    const colorKey = seriesData.map(s => s.color ?? '').join(',');
    const hasSmoothingKey = smoothing > 0;
    // Which series are plotted: changing it resets the user's zoom.
    const seriesKey = seriesData.map(s => `${s.name}#${seriesId(s.fullData)}`).join('|');
    // What data is plotted: the store mutates buffers in place, so the point count is
    // the cheap change marker. Parents rebuild `seriesData` on every render, so keying
    // on this avoids reprocessing (and redrawing) when nothing actually changed.
    const dataSignature = seriesData.map(s => `${s.name}#${seriesId(s.fullData)}:${s.fullData.count}`).join('|');
    const axisRevisionKey = `${themeKey}:${xAxisType}:${removeOutliers ? 'trim' : 'full'}:${yAxisScale}`;
    // CSS variables only change with the theme; reading them per render forces a style recalc.
    const chartTheme = useMemo(readChartTheme, [themeKey]);

    const seriesDataRef = useRef(seriesData);
    seriesDataRef.current = seriesData;

    const processedData = useMemo(
        () => buildProcessedData(seriesDataRef.current, xAxisType, smoothing, xRange, logarithmic),
        // eslint-disable-next-line react-hooks/exhaustive-deps
        [dataSignature, xAxisType, smoothing, xRange, logarithmic]
    );

    const viewRange = xRange ?? localXRange;
    const dataRanges = useMemo(
        () => getFocusedRanges(processedData.processed, removeOutliers, logarithmic && referenceValue !== null && referenceValue <= 0 ? null : referenceValue, logarithmic, viewRange),
        [processedData, removeOutliers, referenceValue, logarithmic, viewRange]
    );
    const trendRange = viewRange ?? (removeOutliers ? dataRanges.focusedXRange : null) ?? dataRanges.defaultXRange;
    const trendFits = useMemo(() => showTrend ? processedData.processed.map(series => fitLinearTrend(series.x, series.raw, {
        xRange: trendRange,
        yRange: removeOutliers ? dataRanges.focusedYRange : null,
    })) : [], [showTrend, processedData, trendRange, removeOutliers, dataRanges.focusedYRange]);
    const trendFitsRef = useRef(trendFits);
    trendFitsRef.current = trendFits;
    const plottedData = useMemo(() => {
        if (!showTrend) return processedData.alignedData;
        const xValues = processedData.alignedData[0];
        const result: uPlot.AlignedData = [xValues];
        let column = 1;
        for (const fit of trendFits) {
            if (hasSmoothingKey) result.push(processedData.alignedData[column++]);
            result.push(processedData.alignedData[column++]);
            result.push(Array.from(xValues, x => {
                if (!fit || x < fit.minX || x > fit.maxX) return null;
                const y = fit.predict(x);
                return Number.isFinite(y) && (!logarithmic || y > 0) ? y : null;
            }));
        }
        return result;
    }, [processedData, showTrend, trendFits, hasSmoothingKey, logarithmic]);
    const focusedRange = useMemo(() => {
        if (!dataRanges.focusedYRange || !showTrend) return dataRanges;
        let [min, max] = dataRanges.focusedYRange;
        for (const fit of trendFits) {
            if (!fit) continue;
            for (const x of [fit.minX, fit.maxX]) {
                const y = fit.predict(x);
                if (logarithmic && y <= 0) continue;
                min = Math.min(min, y);
                max = Math.max(max, y);
            }
        }
        return {...dataRanges, focusedYRange: [min, max] as [number, number]};
    }, [dataRanges, showTrend, trendFits, logarithmic]);
    const focusedRangeRef = useRef(focusedRange);
    focusedRangeRef.current = focusedRange;
    const exponentialTransform = useMemo(() => {
        const [min, max] = focusedRange.focusedYRange ?? [0, 1];
        return createExponentialTransform(min, max);
    }, [focusedRange]);
    const exponentialTransformRef = useRef(exponentialTransform);
    exponentialTransformRef.current = exponentialTransform;

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

        const trendSeries = new Map<number, number>();
        seriesData.forEach((s, i) => {
            const color = s.color || RUN_COLORS[i % RUN_COLORS.length];
            if (hasSmoothingKey) {
                series.push({
                    label: `${s.name} raw`,
                    stroke: color + '80',
                    width: 1,
                    spanGaps: true,
                    points: { show: (_u, _seriesIdx, first, last) => first === last, size: 5, fill: color + '80' }
                });
            }
            series.push({
                label: hasSmoothingKey ? `${s.name} smoothed` : s.name,
                stroke: color,
                width: 2.5,
                spanGaps: true,
                points: { show: (_u, _seriesIdx, first, last) => first === last, size: 5, fill: color }
            });
            if (showTrend) {
                trendSeries.set(series.length, i);
                series.push({label: `${s.name} trend`, stroke: color, width: 1.75, dash: [6, 4], points: {show: false},
                    paths: u => buildTrendPath(u, trendFitsRef.current[i])});
            }
        });

        // Update the tooltip directly from uPlot so pointer movement never rerenders the dashboard.
        const tooltip = document.createElement('div');
        tooltip.className = 'chart-tooltip';
        tooltip.setAttribute('role', 'tooltip');
        tooltip.hidden = true;
        const tooltipHeading = document.createElement('div');
        tooltipHeading.className = 'chart-tooltip-heading';
        tooltip.append(tooltipHeading);
        const tooltipRows = series.slice(1).map((entry, index) => {
            const trendIndex = trendSeries.get(index + 1);
            const row = document.createElement('div');
            row.className = 'chart-tooltip-row';
            const swatch = document.createElement('span');
            swatch.className = `chart-tooltip-swatch${trendIndex !== undefined ? ' is-trend' : ''}`;
            swatch.style.backgroundColor = trendIndex !== undefined ? 'transparent' : String(entry.stroke);
            swatch.style.color = String(entry.stroke);
            const label = document.createElement('span');
            label.className = 'chart-tooltip-label';
            label.textContent = String(entry.label);
            const value = document.createElement('span');
            value.className = 'chart-tooltip-value';
            row.append(swatch, label, value);
            tooltip.append(row);
            return { row, value, trendIndex };
        });
        container.append(tooltip);

        const updateTooltip = (u: uPlot) => {
            const { idx, left, top } = u.cursor;
            const x = idx == null ? null : u.data[0][idx];
            if (idx == null || x == null || left == null || top == null || left < 0 || top < 0 || left > u.over.clientWidth || top > u.over.clientHeight) {
                tooltip.hidden = true;
                return;
            }
            let hasValues = false;
            tooltipRows.forEach(({ row, value, trendIndex }, index) => {
                const y = u.data[index + 1]?.[idx];
                row.hidden = u.series[index + 1].show === false || trendIndex !== undefined && !trendFitsRef.current[trendIndex];
                value.textContent = y != null && Number.isFinite(y) ? formatReferenceValue(y) : '—';
                if (!row.hidden && y != null && Number.isFinite(y)) hasValues = true;
            });
            tooltip.hidden = !hasValues;
            if (!hasValues) return;
            tooltipHeading.textContent = xAxisType === 'wall'
                ? `Wall time · ${new Date(x * 1000).toLocaleString(undefined, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3 })}`
                : xAxisType === 'rel'
                    ? `Elapsed · ${xRelFormat(x)}${x % 1 ? (x % 1).toFixed(3).slice(1) : ''}`
                    : `Step ${formatReferenceValue(x)}`;
            const plotBounds = u.over.getBoundingClientRect();
            const containerBounds = container.getBoundingClientRect();
            const cursorX = plotBounds.left - containerBounds.left + left;
            const cursorY = plotBounds.top - containerBounds.top + top;
            const tooltipX = cursorX + 12 + tooltip.offsetWidth > container.clientWidth ? cursorX - tooltip.offsetWidth - 12 : cursorX + 12;
            const tooltipY = cursorY + 12 + tooltip.offsetHeight > container.clientHeight ? cursorY - tooltip.offsetHeight - 12 : cursorY + 12;
            tooltip.style.left = `${Math.max(4, Math.min(tooltipX, container.clientWidth - tooltip.offsetWidth - 4))}px`;
            tooltip.style.top = `${Math.max(4, Math.min(tooltipY, container.clientHeight - tooltip.offsetHeight - 4))}px`;
        };

        const opts: uPlot.Options = {
            width: container.clientWidth,
            height,
            padding: [10, 10, 0, 10],
            scales: {
                x: { time: xAxisType === 'wall' },
                y: {
                    distr: logarithmic ? 3 : yAxisScale === 'exponential' ? 100 : 1,
                    fwd: value => exponentialTransformRef.current.fwd(value),
                    bwd: value => exponentialTransformRef.current.bwd(value),
                    auto: true,
                    range: (_u, min, max) => focusedRangeRef.current.focusedYRange ?? (logarithmic ? [0.1,1] : [min,max])
                }
            },
            axes: [
                {
                    stroke: chartTheme.axis,
                    font: `11px ${CHART_FONT_FAMILY}`,
                    grid: { stroke: chartTheme.grid, width: 1 },
                    space: 80,
                    values: xAxisType !== 'wall'
                        ? (_u, vals) => vals.map(v => v == null ? '' : xAxisType === 'rel' ? xRelFormat(v) : stepFormat.format(v))
                        : undefined
                },
                {
                    stroke: chartTheme.axis,
                    font: `11px ${CHART_FONT_FAMILY}`,
                    grid: { stroke: chartTheme.grid, width: 1 },
                    space: 40,
                    size: yAxisScale === 'exponential' ? 80 : logarithmic ? 60 : 50,
                    // uPlot's default log filter would hide ticks that aren't near a decade.
                    filter: logarithmic ? (_u, splits) => splits : undefined,
                    // Evenly spaced ticks on the transformed axis, labeled in original units.
                    splits: yAxisScale === 'exponential' ? (u, _axis, min, max) => {
                        const transform = exponentialTransformRef.current;
                        const low = transform.fwd(min);
                        const high = transform.fwd(max);
                        const count = Math.max(2, Math.floor(u.bbox.height / uPlot.pxRatio / 48));
                        return Array.from({length: count + 1}, (_, i) => transform.bwd(low + (high - low) * i / count));
                    } : logarithmic ? (_u, _axis, min, max) => logAxisTicks(min, max) : undefined,
                    values: yAxisScale !== 'linear' ? (_u, values) => values.map(value => value == null ? '' : formatReferenceValue(value)) : undefined,
                }
            ],
            series,
            cursor: {
                drag: { setScale: true },
                points: { show: true },
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
                setCursor: [updateTooltip],
                draw: [u => {
                    drawReferenceLine(u, referenceValueRef.current, chartTheme.reference);
                    const ctx = u.ctx;ctx.save();ctx.strokeStyle=chartTheme.reference;ctx.setLineDash([3*uPlot.pxRatio,3*uPlot.pxRatio]);
                    for(const marker of markersRef.current) {
                        const x=u.valToPos(marker.x,'x',true);
                        if(x<u.bbox.left || x>u.bbox.left+u.bbox.width) continue;
                        ctx.beginPath();ctx.moveTo(x,u.bbox.top);ctx.lineTo(x,u.bbox.top+u.bbox.height);ctx.stroke();
                    }
                    ctx.restore();
                }],
                ready: [
                    u => {
                        u.over.addEventListener('mouseleave', () => { tooltip.hidden = true; });
                        u.over.addEventListener('dblclick', () => setLocalXRange(null));
                        u.over.addEventListener('click', event => {
                            const position=event.clientX-u.over.getBoundingClientRect().left;
                            const marker=markersRef.current.find(marker => Math.abs(u.valToPos(marker.x,'x')-position)<8);
                            if(marker) markerActionRef.current?.(marker.id);
                        });
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

        const plot = new uPlot(opts, plottedData, container);
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
            tooltip.remove();
            plot.destroy();
            plotRef.current = null;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [title, seriesKey, seriesData.length, colorKey, chartTheme, xAxisType, height, hasSmoothingKey, showTrend, axisRevisionKey]);

    // ── Effect 2: Push data and ranges into the existing instance ────────────
    // Also covers reference-line edits: focusedRange depends on referenceValue, and
    // the synchronous batch commit repaints (running the draw hook).
    useEffect(() => {
        const plot = plotRef.current;
        if (!plot || seriesData.length === 0) return;

        // Priority: global window slider > user's manual zoom > outlier focus > full extent.
        const preferred = xRange || localXRange || (removeOutliers ? focusedRange.focusedXRange : null) || focusedRange.defaultXRange;

        applyQuietly(plot, () => {
            plot.setData(plottedData, false); // keep scales; set below
            if (preferred && preferred[0] !== preferred[1]) {
                plot.setScale('x', { min: preferred[0], max: preferred[1] });
            }
            if (focusedRange.focusedYRange) {
                plot.setScale('y', { min: focusedRange.focusedYRange[0], max: focusedRange.focusedYRange[1] });
            }
        });
    }, [plottedData, focusedRange, removeOutliers, xRange, localXRange, seriesKey, seriesData.length, title, colorKey, chartTheme, height, hasSmoothingKey, showTrend, axisRevisionKey, markerSignature]);

    return <div key={axisRevisionKey} ref={containerRef} className="uplot-wrapper" />;
}

// ────────────────────────────────────────────────────────────────────────────
// Data Processing
// ────────────────────────────────────────────────────────────────────────────
interface ProcessedChartData {
    processed: ChartSeries[];
    alignedData: uPlot.AlignedData;
}

function buildProcessedData(
    seriesData: { fullData: TimeSeriesData }[],
    xAxis: XAxisType,
    smoothing: number,
    xRange: [number, number] | null,
    logarithmic = false
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
            if (!Number.isFinite(v) || logarithmic && v <= 0) {
                resX.push(x); rawV.push(NaN);if(smoothing>0)smoothV.push(NaN);continue;
            }
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

        const alignedRaw = filteredX.map(x => { const i = xToIdx.get(x); return i !== undefined && Number.isFinite(p.raw[i]) ? p.raw[i] : null; });
        const alignedSmooth = smoothing > 0
            ? filteredX.map(x => { const i = xToIdx.get(x); return i !== undefined && Number.isFinite(p.smooth[i]) ? p.smooth[i] : null; })
            : null;

        if (alignedSmooth) result.push(alignedRaw as number[]);
        result.push((alignedSmooth ?? alignedRaw) as number[]);
    }
    return { processed, alignedData: result };
}
