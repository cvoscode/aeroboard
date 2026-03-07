import { useEffect, useMemo, useRef, useState } from 'react';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';
import type { TimeSeriesData } from '../hooks/useAeroboard';

export type XAxisType = 'wall' | 'rel' | 'step';

interface LineChartProps {
    title: string;
    seriesData: { name: string; fullData: TimeSeriesData; color?: string }[];
    xAxisType: XAxisType;
    smoothing?: number;
    removeOutliers?: boolean;
    xRange?: [number, number] | null;
    height?: number;
}

const DEFAULT_COLORS = ['#00d2ff', '#ff4c6b', '#00e676', '#ffeb3b', '#e040fb'];

const xRelFormat = (v: number) => {
    const h = Math.floor(v / 3600).toString().padStart(2, '0');
    const m = Math.floor((v % 3600) / 60).toString().padStart(2, '0');
    const s = Math.floor(v % 60).toString().padStart(2, '0');
    return `${h}:${m}:${s}`;
};

export function LineChart({
    title, seriesData, xAxisType,
    smoothing = 0, removeOutliers = false,
    xRange = null,
    height = 250
}: LineChartProps) {
    const containerRef = useRef<HTMLDivElement>(null);
    const plotRef = useRef<uPlot | null>(null);
    const suppressSyncRef = useRef(false);  // prevent feedback loop
    const [localXRange, setLocalXRange] = useState<[number, number] | null>(null);
    const [seriesSignature, setSeriesSignature] = useState('');

    const colorKey = seriesData.map(s => s.color ?? '').join(',');
    const hasSmoothingKey = smoothing > 0;
    const dataSignature = seriesData.map(s => `${s.name}:${s.fullData.count}`).join('|');
    const axisRevisionKey = `${xAxisType}:${removeOutliers ? 'trim' : 'full'}`;

    const processedData = useMemo(
        () => buildProcessedData(seriesData, xAxisType, smoothing, xRange),
        [seriesData, xAxisType, smoothing, xRange]
    );

    const focusedRange = useMemo(
        () => getFocusedRanges(processedData, removeOutliers),
        [processedData, removeOutliers]
    );

    const defaultRange = focusedRange.defaultXRange;

    // Reset EVERYTHING when axis type changes.
    // This is crucial to avoid applying e.g. "step 100" zoom to "wall time" axis.
    useEffect(() => {
        setLocalXRange(null);
    }, [axisRevisionKey]);

    useEffect(() => {
        setLocalXRange(null);
        setSeriesSignature(dataSignature);
    }, [dataSignature, axisRevisionKey]);

    // ── Effect 1: Create & destroy the uPlot instance ────────────────────────
    useEffect(() => {
        if (!containerRef.current || seriesData.length === 0) return;

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
            width: containerRef.current.clientWidth,
            height,
            padding: [10, 10, 0, 10],
            scales: {
                y: {
                    auto: true,
                    range: (_u, min, max) => {
                        const range = focusedRange.focusedYRange;
                        if (range) return range;
                        return [min, max];
                    }
                }
            },
            axes: [
                {
                    stroke: '#9ba1a6',
                    grid: { stroke: 'rgba(255,255,255,0.05)', width: 1 },
                    space: 50,
                    values: xAxisType !== 'wall'
                        ? (_u, vals) => vals.map(v => v == null ? '' : xAxisType === 'rel' ? xRelFormat(v) : v.toFixed(0))
                        : undefined
                },
                {
                    stroke: '#9ba1a6',
                    grid: { stroke: 'rgba(255,255,255,0.05)', width: 1 },
                    space: 40
                }
            ],
            series,
            cursor: {
                drag: { setScale: true },
                points: { show: false }
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
                ready: [
                    u => {
                        u.over.addEventListener('dblclick', () => {
                            setLocalXRange(null);
                            if (!xRange && defaultRange && defaultRange[0] !== defaultRange[1]) {
                                suppressSyncRef.current = true;
                                u.setScale('x', { min: defaultRange[0], max: defaultRange[1] });
                                suppressSyncRef.current = false;
                            }
                        });
                    }
                ],
                setScale: [
                    (u, key) => {
                        if (key === 'x' && !suppressSyncRef.current) {
                            const { min, max } = u.scales.x;
                            if (min != null && max != null) {
                                // If uPlot's internal auto-resize triggered (not from drag),
                                // it might report full range. We usually want to know if it's user-driven.
                                setLocalXRange([min, max]);
                            }
                        }
                    }
                ]
            }
        };

        const plot = new uPlot(opts, processedData.alignedData, containerRef.current);
        plotRef.current = plot;

        const onResize = () => {
            if (containerRef.current)
                plot.setSize({ width: containerRef.current.clientWidth, height });
        };
        window.addEventListener('resize', onResize);
        return () => {
            window.removeEventListener('resize', onResize);
            plot.destroy();
            plotRef.current = null;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [title, seriesData.length, colorKey, xAxisType, height, hasSmoothingKey, axisRevisionKey, processedData.alignedData, focusedRange.focusedYRange]);

    // ── Effect 2: Update data whenever filter/data changes ────────────────────
    useEffect(() => {
        const plot = plotRef.current;
        if (!plot || seriesData.length === 0) return;

        // Data is only filtered by the global master window (xRange from slider)
        plot.setData(processedData.alignedData, false); // Don't reset scales automatically

        // Final effective range preference: 
        // 1. Mandatory Global Window (xRange)
        // 2. User's manual local zoom (localXRange)
        const effRange = xRange || localXRange || (removeOutliers ? focusedRange.focusedXRange : null);
        const shouldResetToDefault = seriesSignature !== dataSignature;

        if (effRange && effRange[0] !== effRange[1]) {
            suppressSyncRef.current = true;
            plot.setScale('x', { min: effRange[0], max: effRange[1] });
            suppressSyncRef.current = false;
        } else if ((shouldResetToDefault || (!xRange && !localXRange)) && defaultRange && defaultRange[0] !== defaultRange[1]) {
            suppressSyncRef.current = true;
            plot.setScale('x', { min: defaultRange[0], max: defaultRange[1] });
            suppressSyncRef.current = false;
        }
        if (focusedRange.focusedYRange) {
            suppressSyncRef.current = true;
            plot.setScale('y', { min: focusedRange.focusedYRange[0], max: focusedRange.focusedYRange[1] });
            suppressSyncRef.current = false;
        }

        plot.redraw(false, true);
    }, [processedData, removeOutliers, xRange, localXRange, defaultRange, dataSignature, seriesSignature, focusedRange, seriesData.length]);

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
    removeOutliers: boolean
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

    const xValues = visiblePoints.map(point => point.x);
    const defaultXRange: [number, number] = [Math.min(...xValues), Math.max(...xValues)];

    const sortedY = [...visiblePoints].sort((a, b) => a.y - b.y);
    const trimFraction = removeOutliers ? 0.1 : 0;
    const lowIdx = Math.floor((sortedY.length - 1) * trimFraction);
    const highIdx = Math.ceil((sortedY.length - 1) * (1 - trimFraction));
    const clippedPoints = sortedY.slice(lowIdx, highIdx + 1);

    if (clippedPoints.length === 0) {
        return { defaultXRange, focusedXRange: defaultXRange, focusedYRange: null };
    }

    const ys = clippedPoints.map(point => point.y);
    const xs = clippedPoints.map(point => point.x);
    const minY = Math.min(...ys);
    const maxY = Math.max(...ys);
    const minX = Math.min(...xs);
    const maxX = Math.max(...xs);

    const yPad = minY === maxY ? (Math.abs(minY || 1) * 0.05 || 1) : (maxY - minY) * (removeOutliers ? 0.03 : 0.08);
    const focusedYRange: [number, number] = [minY - yPad, maxY + yPad];

    return {
        defaultXRange,
        focusedXRange: removeOutliers ? [minX, maxX] : defaultXRange,
        focusedYRange,
    };
}
