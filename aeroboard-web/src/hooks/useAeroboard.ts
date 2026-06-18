import { useState, useEffect, useRef, useCallback } from 'react';
import * as arrow from 'apache-arrow';
import { fetchImageFrame, fetchImageHistory, fetchRuns, fetchRunSummaries, fetchScalarHistory } from "./aeroboard/api";
import { getImageSeries as readImageSeries, getImageSeriesForRun as readImageSeriesForRun, type ImageStore, upsertImageFrame } from "./aeroboard/imageStore";
import {
    ensureRunSummary,
    getChartData as readChartData,
    getGlobalXBounds as readGlobalXBounds,
    getMetricsForRun as readMetricsForRun,
    ingestPoint,
    ingestScalarRows,
    type RunSummaryStore,
    type ScalarStore,
} from "./aeroboard/scalarStore";
import type { ImageSeries, LoadStatus, RunSummary, TimeSeriesData } from "./aeroboard/types";

export type { HyperparameterEntry, ImageFrame, ImageSeries, LoadStatus, MetricSummary, RunSummary, TimeSeriesData } from "./aeroboard/types";

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

function clearStore(
    store: React.MutableRefObject<ScalarStore>,
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
    loadStatus: {
        runs: LoadStatus;
        summaries: LoadStatus;
        history: LoadStatus;
        images: LoadStatus;
    };
    deleteAllRuns: () => Promise<{ ok: boolean; reason?: string }>;
    loadRunSummaries: (runIds?: string[]) => Promise<void>;
    loadHistoryForRuns: (runIds: string[]) => Promise<void>;
    loadImagesForRun: (runId: string) => Promise<void>;
    ensureImageFrame: (runId: string, name: string, tsMs: number) => Promise<void>;
}

export function useAeroboard(wsUrl: string = getDefaultWsUrl(), apiBase: string = getDefaultApiBase()) {
    const [connected, setConnected] = useState(false);
    const storeRef = useRef<ScalarStore>({});
    const summaryRef = useRef<RunSummaryStore>({});
    const imageStoreRef = useRef<ImageStore>({});
    const startTimes = useRef<Record<string, number>>({});
    const [runs, setRuns] = useState<string[]>([]);
    const [tick, setTick] = useState(0);
    const loadedSummaryRunsRef = useRef<Set<string>>(new Set());
    const loadedHistoryRunsRef = useRef<Set<string>>(new Set());
    const loadedImageRunsRef = useRef<Set<string>>(new Set());
    const frameRequestsRef = useRef<Set<string>>(new Set());
    const [loadStatus, setLoadStatus] = useState<UseAeroboardResult["loadStatus"]>({
        runs: "idle",
        summaries: "idle",
        history: "idle",
        images: "idle",
    });

    const refreshRuns = useCallback(() => {
        const allRuns = new Set<string>([
            ...Object.keys(summaryRef.current),
            ...Object.keys(storeRef.current),
            ...Object.keys(imageStoreRef.current),
        ]);
        setRuns(Array.from(allRuns).sort());
        setTick((t) => t + 1);
    }, []);

    const loadRuns = useCallback(async () => {
        setLoadStatus((prev) => ({ ...prev, runs: "loading" }));
        try {
            const fetchedRuns = await fetchRuns(apiBase);
            setRuns(fetchedRuns);
            setTick((t) => t + 1);
            setLoadStatus((prev) => ({ ...prev, runs: "ready" }));
        } catch (error) {
            console.warn("[Aeroboard] Failed to load runs:", error);
            setLoadStatus((prev) => ({ ...prev, runs: "error" }));
        }
    }, [apiBase]);

    const loadRunSummaries = useCallback(async (runIds?: string[]) => {
        const targetRuns = runIds && runIds.length > 0 ? runIds : runs;
        const missingRuns = targetRuns.filter((runId) => !loadedSummaryRunsRef.current.has(runId));
        if (missingRuns.length === 0) return;

        setLoadStatus((prev) => ({ ...prev, summaries: "loading" }));
        try {
            const summaries = await fetchRunSummaries(apiBase, missingRuns);
            for (const summary of summaries) {
                summaryRef.current[summary.run_id] = summary;
                loadedSummaryRunsRef.current.add(summary.run_id);
            }
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, summaries: "ready" }));
        } catch (error) {
            console.warn("[Aeroboard] Failed to load run summaries:", error);
            setLoadStatus((prev) => ({ ...prev, summaries: "error" }));
        }
    }, [apiBase, refreshRuns, runs]);

    const loadHistoryForRuns = useCallback(async (runIds: string[]) => {
        const missingRuns = runIds.filter((runId) => !loadedHistoryRunsRef.current.has(runId));
        if (missingRuns.length === 0) return;

        setLoadStatus((prev) => ({ ...prev, history: "loading" }));
        try {
            const rows = await fetchScalarHistory(apiBase, missingRuns);
            ingestScalarRows(storeRef.current, summaryRef.current, startTimes.current, rows);
            missingRuns.forEach((runId) => loadedHistoryRunsRef.current.add(runId));
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, history: "ready" }));
        } catch (error) {
            console.warn("[Aeroboard] Failed to load history:", error);
            setLoadStatus((prev) => ({ ...prev, history: "error" }));
        }
    }, [apiBase, refreshRuns]);

    const loadImagesForRun = useCallback(async (runId: string) => {
        if (!runId || loadedImageRunsRef.current.has(runId)) return;

        setLoadStatus((prev) => ({ ...prev, images: "loading" }));
        try {
            const frames = await fetchImageHistory(apiBase, runId);
            for (const frame of frames) {
                upsertImageFrame(imageStoreRef.current, frame);
            }
            loadedImageRunsRef.current.add(runId);
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, images: "ready" }));
        } catch (error) {
            console.warn("[Aeroboard] Failed to load image history:", error);
            setLoadStatus((prev) => ({ ...prev, images: "error" }));
        }
    }, [apiBase, refreshRuns]);

    const ensureImageFrame = useCallback(async (runId: string, name: string, tsMs: number) => {
        const existing = imageStoreRef.current[runId]?.[name]?.frames.find((frame) => frame.ts_ms === tsMs);
        if (existing?.image_base64) return;

        const requestKey = `${runId}:${name}:${tsMs}`;
        if (frameRequestsRef.current.has(requestKey)) return;
        frameRequestsRef.current.add(requestKey);

        try {
            const payload = await fetchImageFrame(apiBase, runId, name, tsMs);
            if (payload) {
                upsertImageFrame(imageStoreRef.current, payload);
                refreshRuns();
            }
        } catch (error) {
            console.warn("[Aeroboard] Failed to load image frame:", error);
        } finally {
            frameRequestsRef.current.delete(requestKey);
        }
    }, [apiBase, refreshRuns]);

    useEffect(() => {
        void loadRuns();
    }, [loadRuns]);

    useEffect(() => {
        if (runs.length === 0) return;
        void loadRunSummaries(runs);
    }, [loadRunSummaries, runs]);

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
                            const row = {
                                ts_ms: Number(timeCol.get(i)),
                                run_id: String(runIdCol.get(i)),
                                name: metricName,
                                value: Number(valueCol.get(i)),
                            };
                            if (ingestPoint(storeRef.current, summaryRef.current, startTimes.current, row, batchStep)) {
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
        return readChartData(storeRef.current, runId, name);
    }, []);

    const getMetricsForRun = useCallback((runId: string): string[] => {
        return readMetricsForRun(storeRef.current, runId);
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
        return readImageSeriesForRun(imageStoreRef.current, runId);
    }, []);

    const getImageSeries = useCallback((runId: string, name: string): ImageSeries | null => {
        return readImageSeries(imageStoreRef.current, runId, name);
    }, []);

    const getGlobalXBounds = useCallback((targetRuns: string[], xAxis: 'wall' | 'rel' | 'step'): [number, number] => {
        return readGlobalXBounds(storeRef.current, targetRuns, xAxis);
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
            loadedSummaryRunsRef.current.clear();
            loadedHistoryRunsRef.current.clear();
            loadedImageRunsRef.current.clear();
            frameRequestsRef.current.clear();
            setRuns([]);
            setTick((t) => t + 1);
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
        loadStatus,
        deleteAllRuns,
        loadRunSummaries,
        loadHistoryForRuns,
        loadImagesForRun,
        ensureImageFrame,
    };
}
