import { useEffect, useRef, useState } from 'react';
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
    sharedXRange?: [number, number] | null;
    onRangeChange?: (range: [number, number]) => void;
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
    xRange = null, sharedXRange = null, onRangeChange,
    height = 250
}: LineChartProps) {
    const containerRef = useRef<HTMLDivElement>(null);
    const plotRef = useRef<uPlot | null>(null);
    const suppressSyncRef = useRef(false);  // prevent feedback loop
    const [localXRange, setLocalXRange] = useState<[number, number] | null>(null);

    const colorKey = seriesData.map(s => s.color ?? '').join(',');
    const hasSmoothingKey = smoothing > 0;

    // Reset EVERYTHING when axis type changes.
    // This is crucial to avoid applying e.g. "step 100" zoom to "wall time" axis.
    useEffect(() => {
        setLocalXRange(null);
    }, [xAxisType]);

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
            legend: { show: false },
            hooks: {
                ready: [
                    u => {
                        u.over.addEventListener('dblclick', () => {
                            setLocalXRange(null);
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
                                if (onRangeChange) {
                                    onRangeChange([min, max]);
                                }
                            }
                        }
                    }
                ]
            }
        };

        const data = buildData(seriesData, xAxisType, smoothing, removeOutliers, null);
        const plot = new uPlot(opts, data, containerRef.current);
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
    }, [title, seriesData.length, colorKey, xAxisType, height, hasSmoothingKey]);

    // ── Effect 2: Update data whenever filter/data changes ────────────────────
    useEffect(() => {
        const plot = plotRef.current;
        if (!plot || seriesData.length === 0) return;

        // Data is only filtered by the global master window (xRange from slider)
        const data = buildData(seriesData, xAxisType, smoothing, removeOutliers, xRange);
        plot.setData(data, false); // Don't reset scales automatically

        // Final effective range preference: 
        // 1. Mandatory Global Window (xRange)
        // 2. Synchronized shared range from another chart (sharedXRange)
        // 3. User's manual local zoom (localXRange)
        const effRange = xRange || sharedXRange || localXRange;

        if (effRange && effRange[0] !== effRange[1]) {
            suppressSyncRef.current = true;
            plot.setScale('x', { min: effRange[0], max: effRange[1] });
            suppressSyncRef.current = false;
        } else if (!xRange && !sharedXRange && !localXRange) {
            // Auto-fit mode
            const xs = data[0] as number[];
            if (xs.length > 0) {
                suppressSyncRef.current = true;
                plot.setScale('x', { min: xs[0], max: xs[xs.length - 1] });
                suppressSyncRef.current = false;
            }
        }
    }, [seriesData, xAxisType, smoothing, removeOutliers, xRange, sharedXRange, localXRange]);

    return <div ref={containerRef} className="uplot-wrapper" />;
}

// ────────────────────────────────────────────────────────────────────────────
// Data Processing
// ────────────────────────────────────────────────────────────────────────────
function buildData(
    seriesData: { fullData: TimeSeriesData }[],
    xAxis: XAxisType,
    smoothing: number,
    removeOutliers: boolean,
    xRange: [number, number] | null
): uPlot.AlignedData {
    if (seriesData.length === 0) return [[]];

    const processed = seriesData.map(s => {
        const { wallTimes, relTimes, steps, values, count: n } = s.fullData;
        const xArr = xAxis === 'wall' ? wallTimes : xAxis === 'rel' ? relTimes : steps;

        let mean = 0, std = Infinity;
        if (removeOutliers && n > 2) {
            let sum = 0, sumSq = 0;
            for (let i = 0; i < n; i++) { sum += values[i]; sumSq += values[i] ** 2; }
            mean = sum / n;
            std = Math.sqrt(Math.max(0, sumSq / n - mean ** 2));
        }

        const resX: number[] = [];
        const rawV: number[] = [];
        const smoothV: number[] = [];
        let ema = 0, first = true;

        for (let i = 0; i < n; i++) {
            const x = xArr[i];
            const v = values[i];
            if (xAxis === 'step' && x < 0) continue;
            if (removeOutliers && isFinite(std) && std > 0 && Math.abs(v - mean) > 3 * std) continue;
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

    if (filteredX.length === 0) return [filteredX];

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
    return result;
}
