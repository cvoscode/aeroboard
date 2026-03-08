import { useState, useEffect, useRef, useCallback } from 'react';
import * as arrow from 'apache-arrow';

export interface TimeSeriesData {
    wallTimes: Float64Array;
    relTimes: Float64Array;
    steps: Float64Array;
    plotX: Float64Array;
    values: Float64Array;
    count: number;
}

export interface HyperparameterEntry {
    key: string;
    value: string;
    value_type: string;
}

export interface MetricSummary {
    name: string;
    last: number;
    min: number;
    max: number;
}

export interface RunSummary {
    run_id: string;
    hyperparameters: HyperparameterEntry[];
    metric_summaries: MetricSummary[];
}

export interface ImageFrame {
    ts_ms: number;
    run_id: string;
    name: string;
    format: string;
    image_base64: string;
}

export interface ImageSeries {
    run_id: string;
    name: string;
    frames: ImageFrame[];
}

type Store = Record<string, Record<string, TimeSeriesData>>;
type RunSummaryStore = Record<string, RunSummary>;
type ImageStore = Record<string, Record<string, ImageSeries>>;

type MetricSummaryInternal = MetricSummary & { __ts?: number };

function makeEmptySeries(): TimeSeriesData {
    return {
        wallTimes: new Float64Array(4096),
        relTimes: new Float64Array(4096),
        steps: new Float64Array(4096).fill(-1),
        plotX: new Float64Array(4096),
        values: new Float64Array(4096),
        count: 0,
    };
}

function growFloat64Array(source: Float64Array, nextSize: number, fillValue?: number): Float64Array {
    const next = new Float64Array(nextSize);
    next.set(source);
    if (fillValue !== undefined && nextSize > source.length) {
        next.fill(fillValue, source.length);
    }
    return next;
}

function ensureSeriesCapacity(series: TimeSeriesData) {
    if (series.count < series.values.length) return;
    const nextSize = Math.max(1024, series.values.length * 2);
    series.wallTimes = growFloat64Array(series.wallTimes, nextSize);
    series.relTimes = growFloat64Array(series.relTimes, nextSize);
    series.steps = growFloat64Array(series.steps, nextSize, -1);
    series.plotX = growFloat64Array(series.plotX, nextSize);
    series.values = growFloat64Array(series.values, nextSize);
}

function ensureRunSummary(summaryStore: RunSummaryStore, runId: string): RunSummary {
    if (!summaryStore[runId]) {
        summaryStore[runId] = { run_id: runId, hyperparameters: [], metric_summaries: [] };
    }
    return summaryStore[runId];
}

function updateMetricSummary(summary: RunSummary, name: string, tsMs: number, value: number) {
    const metricEntries = summary.metric_summaries as MetricSummaryInternal[];
    const existing = metricEntries.find((entry) => entry.name === name);
    if (existing) {
        existing.min = Math.min(existing.min, value);
        existing.max = Math.max(existing.max, value);
        if ((existing.__ts ?? Number.NEGATIVE_INFINITY) <= tsMs) {
            existing.last = value;
            existing.__ts = tsMs;
        }
        return;
    }

    metricEntries.push({ name, last: value, min: value, max: value, __ts: tsMs });
}

function ingestPoint(
    store: Store,
    summaryStore: RunSummaryStore,
    startTimes: Record<string, number>,
    runId: string,
    name: string,
    tsMs: number,
    val: number,
    step: number | null,
): boolean {
    const wallTime = tsMs / 1000;

    if (!startTimes[runId]) startTimes[runId] = wallTime;
    const relTime = wallTime - startTimes[runId];

    if (!store[runId]) store[runId] = {};
    const runStore = store[runId];

    if (!runStore[name]) runStore[name] = makeEmptySeries();
    const series = runStore[name];

    ensureSeriesCapacity(series);

    const i = series.count;
    series.wallTimes[i] = wallTime;
    series.relTimes[i] = relTime;
    series.plotX[i] = step ?? relTime;
    series.values[i] = val;
    if (step !== null) series.steps[i] = step;
    series.count = i + 1;

    updateMetricSummary(ensureRunSummary(summaryStore, runId), name, tsMs, val);
    return true;
}

async function fetchPagedJson<T>(url: string): Promise<T[] | null> {
    try {
        const separator = url.includes('?') ? '&' : '?';
        const resp = await fetch(`${url}${separator}limit=10000&cursor=0`);
        if (!resp.ok) {
            throw new Error(`Request failed with status ${resp.status}`);
        }

        const json = await resp.json() as { rows?: T[]; next_cursor?: number | null; error?: string };
        if (!resp.ok && json.error) {
            console.error('Aeroboard API error:', json.error);
        }

        const rows: T[] = json.rows ?? [];
        let nextCursor = json.next_cursor ?? null;

        while (nextCursor !== null) {
            const pageResp = await fetch(`${url}${separator}limit=10000&cursor=${nextCursor}`);
            if (!pageResp.ok) {
                console.warn(`Aeroboard API page request failed with status ${pageResp.status}`);
                break;
            }
            const page = await pageResp.json() as { rows?: T[]; next_cursor?: number | null };
            rows.push(...(page.rows ?? []));
            nextCursor = page.next_cursor ?? null;

            if ((page.rows ?? []).length === 0 && nextCursor == null) {
                break;
            }
        }

        return rows;
    } catch (err) {
        const error = err instanceof Error ? err.message : String(err);
        console.warn(`[Aeroboard] Failed to fetch history from ${url}:`, error);
        return null;
    }
}

function setHyperparameter(summaryStore: RunSummaryStore, runId: string, key: string, value: string, valueType: string) {
    const summary = ensureRunSummary(summaryStore, runId);
    const existing = summary.hyperparameters.find((entry) => entry.key === key);
    if (existing) {
        existing.value = value;
        existing.value_type = valueType;
        return;
    }
    summary.hyperparameters.push({ key, value, value_type: valueType });
}

function upsertImageFrame(imageStore: ImageStore, frame: ImageFrame) {
    if (!imageStore[frame.run_id]) imageStore[frame.run_id] = {};
    if (!imageStore[frame.run_id][frame.name]) {
        imageStore[frame.run_id][frame.name] = { run_id: frame.run_id, name: frame.name, frames: [] };
    }

    const series = imageStore[frame.run_id][frame.name];
    const existingIndex = series.frames.findIndex((candidate) => candidate.ts_ms === frame.ts_ms);
    if (existingIndex >= 0) series.frames[existingIndex] = frame;
    else series.frames.push(frame);
    series.frames.sort((a, b) => a.ts_ms - b.ts_ms);
}

function clearStore(
    store: React.MutableRefObject<Store>,
    startTimes: React.MutableRefObject<Record<string, number>>,
    summaries: React.MutableRefObject<RunSummaryStore>,
    images: React.MutableRefObject<ImageStore>,
) {
    store.current = {};
    startTimes.current = {};
    summaries.current = {};
    images.current = {};
}

function bytesToBase64(bytes: Uint8Array): string {
    let binary = '';
    bytes.forEach((byte) => {
        binary += String.fromCharCode(byte);
    });
    return btoa(binary);
}

function getDefaultApiBase(): string {
    if (typeof window === 'undefined') return 'http://localhost:3000';
    return window.location.origin;
}

function getDefaultWsUrl(): string {
    if (typeof window === 'undefined') return 'ws://localhost:3000/ws/live';
    const protocol = window.location.protocol === 'https:' ? 'wss' : 'ws';
    return `${protocol}://${window.location.host}/ws/live`;
}

export interface UseAeroboardResult {
    connected: boolean;
    getChartData: (runId: string, name: string) => TimeSeriesData | null;
    getMetricsForRun: (runId: string) => string[];
    getGlobalXBounds: (targetRuns: string[], xAxis: 'wall' | 'rel' | 'step') => [number, number];
    getRunSummaries: (runIds: string[]) => RunSummary[];
    getHyperparameterKeys: (runIds: string[]) => string[];
    getMetricSummaryKeys: (runIds: string[]) => string[];
    getImageSeriesForRun: (runId: string) => ImageSeries[];
    getImageSeries: (runId: string, name: string) => ImageSeries | null;
    runs: string[];
    tick: number;
    deleteAllRuns: () => Promise<{ ok: boolean; reason?: string }>;
    loadHistory: () => Promise<void>;
}

export function useAeroboard(wsUrl: string = getDefaultWsUrl(), apiBase: string = getDefaultApiBase()) {
    const [connected, setConnected] = useState(false);
    const storeRef = useRef<Store>({});
    const summaryRef = useRef<RunSummaryStore>({});
    const imageStoreRef = useRef<ImageStore>({});
    const startTimes = useRef<Record<string, number>>({});
    const [runs, setRuns] = useState<string[]>([]);
    const [tick, setTick] = useState(0);
    const [reloadToken, setReloadToken] = useState(0);

    const refreshRuns = useCallback(() => {
        const allRuns = new Set<string>([
            ...Object.keys(summaryRef.current),
            ...Object.keys(storeRef.current),
            ...Object.keys(imageStoreRef.current),
        ]);
        setRuns(Array.from(allRuns).sort());
        setTick((t) => t + 1);
    }, []);

    const loadHistory = useCallback(async () => {
        try {
            const [historyResp, imageRows] = await Promise.all([
                fetch(`${apiBase}/api/history?limit=10000&cursor=0`),
                fetchPagedJson<ImageFrame>(`${apiBase}/api/history/images`),
            ]);

            if (!historyResp.ok) {
                console.warn('[Aeroboard] History request not OK:', historyResp.status);
            }

            if (historyResp.ok) {
                const firstPage = await historyResp.json() as {
                    rows: { ts_ms: number; run_id: string; name: string; value: number }[];
                    run_summaries?: RunSummary[];
                    next_cursor?: number | null;
                    error?: string;
                };
                const allRows = [...(firstPage.rows ?? [])];

                let nextCursor = firstPage.next_cursor ?? null;
                while (nextCursor !== null) {
                    const pageResp = await fetch(`${apiBase}/api/history?limit=10000&cursor=${nextCursor}`);
                    if (!pageResp.ok) {
                        console.warn('[Aeroboard] History page request failed:', pageResp.status);
                        break;
                    }
                    const page = await pageResp.json() as {
                        rows?: { ts_ms: number; run_id: string; name: string; value: number }[];
                        next_cursor?: number | null;
                    };
                    allRows.push(...(page.rows ?? []));
                    nextCursor = page.next_cursor ?? null;

                    if ((page.rows ?? []).length === 0 && nextCursor == null) {
                        break;
                    }
                }

                for (const summary of firstPage.run_summaries ?? []) {
                    summaryRef.current[summary.run_id] = summary;
                }

                if (allRows.length) {
                    allRows.sort((a, b) => a.ts_ms - b.ts_ms);
                    const stepMap: Record<string, Map<number, number>> = {};
                    for (const row of allRows) {
                        if (row.name === 'step' || row.name.endsWith('/step')) {
                            if (!stepMap[row.run_id]) stepMap[row.run_id] = new Map();
                            stepMap[row.run_id].set(row.ts_ms, row.value);
                        }
                    }

                    for (const row of allRows) {
                        if (row.name === 'step' || row.name.endsWith('/step')) continue;
                        const step = stepMap[row.run_id]?.get(row.ts_ms) ?? null;
                        ingestPoint(storeRef.current, summaryRef.current, startTimes.current, row.run_id, row.name, row.ts_ms, row.value, step);
                    }
                }
            }

            if (imageRows !== null) {
                for (const frame of imageRows) {
                    upsertImageFrame(imageStoreRef.current, frame);
                }
            }

            refreshRuns();
        } catch (e) {
            console.warn('[Aeroboard] Failed to load history:', e);
        }
    }, [apiBase, refreshRuns]);

    useEffect(() => {
        loadHistory();
    }, [apiBase, reloadToken, loadHistory]);

    useEffect(() => {
        const ws = new WebSocket(wsUrl);
        ws.binaryType = 'arraybuffer';

        ws.onopen = () => setConnected(true);
        ws.onclose = () => setConnected(false);

        ws.onerror = (event) => {
            console.error('[Aeroboard] WebSocket error:', event);
            setConnected(false);
        };

        ws.onmessage = async (event) => {
            try {
                if (!(event.data instanceof ArrayBuffer)) return;
                const table = arrow.tableFromIPC(event.data);

                let shouldUpdate = false;

                for (const batch of table.batches) {
                    const timeCol = batch.getChild('timestamp');
                    const runIdCol = batch.getChild('run_id');
                    if (!timeCol || !runIdCol) continue;

                    const nameCol = batch.getChild('name');
                    const valueCol = batch.getChild('value');
                    const imageCol = batch.getChild('image_data');
                    const formatCol = batch.getChild('format');
                    const keyCol = batch.getChild('key');
                    const valueTypeCol = batch.getChild('value_type');

                    if (nameCol && valueCol && !keyCol) {
                        let batchStep: number | null = null;
                        for (let i = 0; i < batch.numRows; i++) {
                            const metricName = String(nameCol.get(i));
                            if (metricName === 'step' || metricName.endsWith('/step')) {
                                batchStep = Number(valueCol.get(i));
                                break;
                            }
                        }

                        for (let i = 0; i < batch.numRows; i++) {
                            const metricName = String(nameCol.get(i));
                            if (metricName === 'step' || metricName.endsWith('/step')) continue;
                            const tsMs = Number(timeCol.get(i));
                            const runId = String(runIdCol.get(i));
                            const val = Number(valueCol.get(i));
                            if (ingestPoint(storeRef.current, summaryRef.current, startTimes.current, runId, metricName, tsMs, val, batchStep)) {
                                shouldUpdate = true;
                            }
                        }
                    } else if (nameCol && imageCol && formatCol) {
                        for (let i = 0; i < batch.numRows; i++) {
                            const payload = imageCol.get(i);
                            const bytes = payload instanceof Uint8Array ? payload : new Uint8Array(payload as ArrayBufferLike);
                            upsertImageFrame(imageStoreRef.current, {
                                ts_ms: Number(timeCol.get(i)),
                                run_id: String(runIdCol.get(i)),
                                name: String(nameCol.get(i)),
                                format: String(formatCol.get(i)),
                                image_base64: bytesToBase64(bytes),
                            });
                            shouldUpdate = true;
                        }
                    } else if (keyCol && valueCol && valueTypeCol) {
                        for (let i = 0; i < batch.numRows; i++) {
                            setHyperparameter(
                                summaryRef.current,
                                String(runIdCol.get(i)),
                                String(keyCol.get(i)),
                                String(valueCol.get(i)),
                                String(valueTypeCol.get(i)),
                            );
                            shouldUpdate = true;
                        }
                    }
                }

                if (shouldUpdate) refreshRuns();
            } catch (err) {
                console.error('[Aeroboard] Failed to parse Arrow batch from WS:', err);
            }
        };

        return () => ws.close();
    }, [wsUrl, refreshRuns]);

    const getChartData = useCallback((runId: string, name: string): TimeSeriesData | null => {
        if (!runId) return null;
        const series = storeRef.current[runId]?.[name];
        return series && series.count > 0 ? series : null;
    }, []);

    const getMetricsForRun = useCallback((runId: string): string[] => {
        if (!runId) return [];
        return Object.keys(storeRef.current[runId] ?? {});
    }, []);

    const getRunSummaries = useCallback((runIds: string[]): RunSummary[] => {
        return runIds
            .map((runId) => summaryRef.current[runId] ?? { run_id: runId, hyperparameters: [], metric_summaries: [] })
            .sort((a, b) => a.run_id.localeCompare(b.run_id));
    }, []);

    const getHyperparameterKeys = useCallback((runIds: string[]): string[] => {
        const keys = new Set<string>();
        for (const summary of getRunSummaries(runIds)) {
            summary.hyperparameters.forEach((entry) => keys.add(entry.key));
        }
        return Array.from(keys).sort();
    }, [getRunSummaries]);

    const getMetricSummaryKeys = useCallback((runIds: string[]): string[] => {
        const keys = new Set<string>();
        for (const summary of getRunSummaries(runIds)) {
            summary.metric_summaries.forEach((entry) => keys.add(entry.name));
        }
        return Array.from(keys).sort();
    }, [getRunSummaries]);

    const getImageSeriesForRun = useCallback((runId: string): ImageSeries[] => {
        return Object.values(imageStoreRef.current[runId] ?? {}).sort((a, b) => a.name.localeCompare(b.name));
    }, []);

    const getImageSeries = useCallback((runId: string, name: string): ImageSeries | null => {
        return imageStoreRef.current[runId]?.[name] ?? null;
    }, []);

    const getGlobalXBounds = useCallback((targetRuns: string[], xAxis: 'wall' | 'rel' | 'step'): [number, number] => {
        let min = Infinity;
        let max = -Infinity;
        for (const runId of targetRuns) {
            const runStore = storeRef.current[runId];
            if (!runStore) continue;
            for (const series of Object.values(runStore)) {
                const xArr = xAxis === 'wall' ? series.wallTimes : xAxis === 'rel' ? series.relTimes : series.steps;
                for (let i = 0; i < series.count; i++) {
                    if (xAxis === 'step' && xArr[i] < 0) continue;
                    if (xArr[i] < min) min = xArr[i];
                    if (xArr[i] > max) max = xArr[i];
                }
            }
        }
        return min === Infinity ? [0, 100] : [min, max];
    }, []);

    const deleteAllRuns = useCallback(async (): Promise<{ ok: boolean; reason?: string }> => {
        try {
            const resp = await fetch(`${apiBase}/api/history`, { method: 'DELETE' });
            if (!resp.ok) {
                if (resp.status === 404 || resp.status === 405) {
                    return { ok: false, reason: 'server-restart-required' };
                }

                let message = `Request failed with status ${resp.status}`;
                try {
                    const body = await resp.json() as { error?: string };
                    if (body.error) message = body.error;
                } catch {
                    // ignore parse failures
                }
                return { ok: false, reason: message };
            }
            clearStore(storeRef, startTimes, summaryRef, imageStoreRef);
            setRuns([]);
            setTick((t) => t + 1);
            setReloadToken((t) => t + 1);
            return { ok: true };
        } catch (err) {
            console.error('[Aeroboard] Failed to delete runs:', err);
            return { ok: false, reason: err instanceof Error ? err.message : 'network-error' };
        }
    }, [apiBase]);

    return {
        connected,
        getChartData,
        getMetricsForRun,
        getGlobalXBounds,
        getRunSummaries,
        getHyperparameterKeys,
        getMetricSummaryKeys,
        getImageSeriesForRun,
        getImageSeries,
        runs,
        tick,
        deleteAllRuns,
        loadHistory,
    };
}