import { useLiveSocket } from "./useLiveSocket";
import { useState, useEffect, useRef, useCallback } from 'react';
import { tableFromIPC } from 'apache-arrow';
import { fetchImageHistory, fetchMediaHistory, fetchRuns, fetchRunSummaries, fetchScalarHistory, fetchServerDashboardDefaults, imageContentUrl, mediaContentUrl } from "./aeroboard/api";
import { DASHBOARD_CONFIG_KEY, extractRunDashboardConfig, parseRunDashboardConfigValue, type PartialDashboardDefaults } from "../lib/dashboardDefaults";
import { getMediaSeriesForRun as readMediaSeriesForRun, MEDIA_KINDS, type MediaStore, revokeMediaStore, upsertMediaClip } from "./aeroboard/mediaStore";
import { mediaMimeType } from "../lib/media";
import { getImageSeries as readImageSeries, getImageSeriesForRun as readImageSeriesForRun, type ImageStore, revokeImageStore, upsertImageFrame } from "./aeroboard/imageStore";
import {
    ensureRunSummary,
    setSummaryHyperparameter,
    getChartData as readChartData,
    getGlobalXBounds as readGlobalXBounds,
    getMetricsForRun as readMetricsForRun,
    mergeScalarColumns,
    type RunSummaryStore,
    type ScalarStore,
} from "./aeroboard/scalarStore";
import type { ImageFrame, ImageSeries, LoadStatus, MediaClip, MediaKind, MediaSeries, RunSummary, TimeSeriesData } from "./aeroboard/types";

export type { HyperparameterEntry, ImageFrame, ImageSeries, LoadStatus, MediaClip, MediaKind, MediaSeries, MetricSummary, RunSummary, TimeSeriesData } from "./aeroboard/types";

type DashboardConfigStore = Record<string, PartialDashboardDefaults | null>;

function setHyperparameter(
    summaryStore: RunSummaryStore,
    dashboardConfigs: DashboardConfigStore,
    runId: string,
    key: string,
    value: string,
    valueType: string,
) {
    // Per-run dashboard settings share the hyperparameter channel but aren't hyperparameters.
    if (key === DASHBOARD_CONFIG_KEY) {
        dashboardConfigs[runId] = parseRunDashboardConfigValue(value);
        return;
    }
    const summary = ensureRunSummary(summaryStore, runId);
    setSummaryHyperparameter(summary,key,value,valueType);
}

/**
 * Combine a summary fetched from the server with what this tab accumulated
 * from live data, so a late-arriving fetch can't erase newer live values.
 */
function mergeSummaries(server: RunSummary, local: RunSummary | undefined): RunSummary {
    if (!local) return server;
    const hyperparameters = new Map(server.hyperparameters.map((entry) => [entry.key, entry]));
    for (const entry of local.hyperparameters) hyperparameters.set(entry.key, entry);

    const metrics = new Map(server.metric_summaries.map((entry) => [entry.name, { ...entry }]));
    for (const entry of local.metric_summaries) {
        const existing = metrics.get(entry.name);
        if (!existing) {
            metrics.set(entry.name, entry);
            continue;
        }
        // Keep the live entry object (it carries its timestamp for later updates).
        metrics.set(entry.name, Object.assign(entry, {
            min: Math.min(existing.min, entry.min),
            max: Math.max(existing.max, entry.max),
        }));
    }
    return {
        run_id: server.run_id,
        hyperparameters: Array.from(hyperparameters.values()),
        metric_summaries: Array.from(metrics.values()),
    };
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

function sameList(a: string[], b: string[]): boolean {
    return a.length === b.length && a.every((value, index) => value === b[index]);
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
    /** `<img src>` for a frame: its live Blob URL, or the persisted-content URL. */
    getImageSource: (frame: ImageFrame, attempt?: number) => string;
    getMediaSeriesForRun: (runId: string) => MediaSeries[];
    getMediaSource: (clip: MediaClip, attempt?: number) => string;
    runs: string[];
    tick: number;
    subscribeRuns: (runs:string[])=>void;
    loadStatus: {
        runs: LoadStatus;
        summaries: LoadStatus;
        history: LoadStatus;
        images: LoadStatus;
        media: LoadStatus;
    };
    deleteAllRuns: () => Promise<{ ok: boolean; reason?: string }>;
    /** Remove successfully deleted runs from local caches, including late responses. */
    forgetRuns: (runIds: string[]) => void;
    loadRunSummaries: (runIds?: string[]) => Promise<void>;
    loadHistoryForRuns: (runIds: string[]) => Promise<void>;
    loadImagesForRun: (runId: string) => Promise<void>;
    loadMediaForRun: (runId: string) => Promise<void>;
    /** Dashboard settings the training script sent for this run, if any. */
    getRunDashboardConfig: (runId: string) => PartialDashboardDefaults | null;
    /** Server-wide defaults from aeroboard.toml (null until loaded). */
    serverDashboardDefaults: PartialDashboardDefaults | null;
    /** Problem reading aeroboard.toml, surfaced so a typo isn't silently ignored. */
    dashboardConfigError: string | null;
}

export function useAeroboard(wsUrl: string = getDefaultWsUrl(), apiBase: string = getDefaultApiBase()): UseAeroboardResult {
    const selectedRunsRef=useRef<string[]>([]);
    const socketRef=useRef<WebSocket|null>(null);
    const subscribeRuns=useCallback((runs:string[])=>{
        const previous=new Set(selectedRunsRef.current);
        selectedRunsRef.current=runs;
        for(const run of runs)if(!previous.has(run))loadedHistoryRunsRef.current.delete(run);
        // Keep a small navigation cache; selected comparisons are never evicted.
        const inactive=Object.keys(storeRef.current).filter(run=>!runs.includes(run));
        for(const run of inactive.slice(0,Math.max(0,inactive.length-8))){
            delete storeRef.current[run];delete startTimes.current[run];loadedHistoryRunsRef.current.delete(run);
            if(imageStoreRef.current[run]){revokeImageStore({[run]:imageStoreRef.current[run]});delete imageStoreRef.current[run];loadedImageRunsRef.current.delete(run);}
            if(mediaStoreRef.current[run]){revokeMediaStore({[run]:mediaStoreRef.current[run]});delete mediaStoreRef.current[run];loadedMediaRunsRef.current.delete(run);}
        }
        if(socketRef.current?.readyState===WebSocket.OPEN)socketRef.current.send(JSON.stringify({type:'subscribe',runs}));
    },[]);
    const [connected, setConnected] = useState(false);
    const storeRef = useRef<ScalarStore>({});
    const summaryRef = useRef<RunSummaryStore>({});
    const imageStoreRef = useRef<ImageStore>({});
    const mediaStoreRef = useRef<MediaStore>({});
    const dashboardConfigsRef = useRef<DashboardConfigStore>({});
    const seenBatchesRef = useRef(new Set<string>());
    const [serverDashboardDefaults, setServerDashboardDefaults] = useState<PartialDashboardDefaults | null>(null);
    const [dashboardConfigError, setDashboardConfigError] = useState<string | null>(null);
    const startTimes = useRef<Record<string, number>>({});
    const [runs, setRuns] = useState<string[]>([]);
    const runsRef = useRef<string[]>([]);
    /** Runs the server reported via /api/runs (the sidebar also shows runs seen live). */
    const fetchedRunsRef = useRef<string[]>([]);
    const deletedRunsRef = useRef(new Set<string>());
    const [tick, setTick] = useState(0);
    const refreshScheduledRef = useRef(false);
    const loadedSummaryRunsRef = useRef<Set<string>>(new Set());
    const pendingSummaryRunsRef = useRef<Set<string>>(new Set());
    const loadedHistoryRunsRef = useRef<Set<string>>(new Set());
    const pendingHistoryRunsRef = useRef<Set<string>>(new Set());
    const loadedImageRunsRef = useRef<Set<string>>(new Set());
    const loadedMediaRunsRef = useRef<Set<string>>(new Set());
    const [loadStatus, setLoadStatus] = useState<UseAeroboardResult["loadStatus"]>({
        runs: "idle",
        summaries: "idle",
        history: "idle",
        images: "idle",
        media: "idle",
    });

    /**
     * Re-render at most once per animation frame, however many batches arrive.
     * The run list is the union of server-listed runs and runs seen locally.
     */
    const refreshRuns = useCallback(() => {
        if (refreshScheduledRef.current) return;
        refreshScheduledRef.current = true;
        const schedule = typeof requestAnimationFrame === 'function'
            ? requestAnimationFrame
            : (callback: () => void) => setTimeout(callback, 16);
        schedule(() => {
            refreshScheduledRef.current = false;
            const allRuns = Array.from(new Set<string>([
                ...fetchedRunsRef.current,
                ...Object.keys(summaryRef.current),
                ...Object.keys(storeRef.current),
                ...Object.keys(imageStoreRef.current),
                ...Object.keys(mediaStoreRef.current),
            ])).filter(runId => !deletedRunsRef.current.has(runId)).sort();
            // Only publish a new array when the list changed, so effects keyed on
            // `runs` don't re-run on every live batch.
            if (!sameList(allRuns, runsRef.current)) {
                runsRef.current = allRuns;
                setRuns(allRuns);
            }
            setTick((t) => t + 1);
        });
    }, []);

    const loadRuns = useCallback(async () => {
        setLoadStatus((prev) => ({ ...prev, runs: "loading" }));
        try {
            fetchedRunsRef.current = await fetchRuns(apiBase);
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, runs: "ready" }));
        } catch (error) {
            console.warn("[Aeroboard] Failed to load runs:", error);
            setLoadStatus((prev) => ({ ...prev, runs: "error" }));
        }
    }, [apiBase, refreshRuns]);

    const loadRunSummaries = useCallback(async (runIds?: string[]) => {
        const targetRuns = runIds && runIds.length > 0 ? runIds : runsRef.current;
        // Skip runs already loaded *or* currently being fetched: a run the server has
        // no summary for (e.g. media only, or live-only mode) must not be re-requested
        // on every live batch.
        const missingRuns = targetRuns.filter(
            (runId) => !deletedRunsRef.current.has(runId) && !loadedSummaryRunsRef.current.has(runId) && !pendingSummaryRunsRef.current.has(runId),
        );
        if (missingRuns.length === 0) return;
        missingRuns.forEach((runId) => pendingSummaryRunsRef.current.add(runId));

        setLoadStatus((prev) => ({ ...prev, summaries: "loading" }));
        try {
            const summaries = await fetchRunSummaries(apiBase, missingRuns);
            for (const rawSummary of summaries) {
                if (deletedRunsRef.current.has(rawSummary.run_id)) continue;
                const { summary, config } = extractRunDashboardConfig(rawSummary);
                summaryRef.current[summary.run_id] = mergeSummaries(summary, summaryRef.current[summary.run_id]);
                if (config) dashboardConfigsRef.current[summary.run_id] ??= config;
            }
            missingRuns.forEach((runId) => loadedSummaryRunsRef.current.add(runId));
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, summaries: "ready" }));
        } catch (error) {
            console.warn("[Aeroboard] Failed to load run summaries:", error);
            setLoadStatus((prev) => ({ ...prev, summaries: "error" }));
        } finally {
            missingRuns.forEach((runId) => pendingSummaryRunsRef.current.delete(runId));
        }
    }, [apiBase, refreshRuns]);

    /** Fetch persisted history and merge it with live points already received. */
    const fetchHistory = useCallback(async (runIds: string[], force: boolean) => {
        const targetRuns = runIds.filter(
            (runId) => !deletedRunsRef.current.has(runId) && (force || !loadedHistoryRunsRef.current.has(runId)) && !pendingHistoryRunsRef.current.has(runId),
        );
        if (targetRuns.length === 0) return;
        targetRuns.forEach((runId) => pendingHistoryRunsRef.current.add(runId));

        setLoadStatus((prev) => ({ ...prev, history: "loading" }));
        try {
            const lastTimes = targetRuns.map(run => Math.max(0,...Object.values(storeRef.current[run] ?? {}).map(series => series.count ? series.wallTimes[series.count-1]*1000 : 0)));
            const startMs = force && lastTimes.every(time => time>0) ? Math.min(...lastTimes)-1000 : undefined;
            const table = await fetchScalarHistory(apiBase, targetRuns,startMs);
            mergeScalarColumns(storeRef.current, summaryRef.current, startTimes.current,table.batches,
                new Set(targetRuns.filter(runId => !deletedRunsRef.current.has(runId))));
            targetRuns.forEach((runId) => loadedHistoryRunsRef.current.add(runId));
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, history: "ready" }));
        } catch (error) {
            console.warn("[Aeroboard] Failed to load history:", error);
            setLoadStatus((prev) => ({ ...prev, history: "error" }));
        } finally {
            targetRuns.forEach((runId) => pendingHistoryRunsRef.current.delete(runId));
        }
    }, [apiBase, refreshRuns]);

    const loadHistoryForRuns = useCallback((runIds: string[]) => fetchHistory(runIds, false), [fetchHistory]);

    const loadImagesForRun = useCallback(async (runId: string) => {
        if (!runId || deletedRunsRef.current.has(runId) || loadedImageRunsRef.current.has(runId)) return;
        loadedImageRunsRef.current.add(runId);

        setLoadStatus((prev) => ({ ...prev, images: "loading" }));
        try {
            const frames = await fetchImageHistory(apiBase, runId);
            for (const frame of frames) {
                if (deletedRunsRef.current.has(frame.run_id)) continue;
                upsertImageFrame(imageStoreRef.current, frame);
            }
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, images: "ready" }));
        } catch (error) {
            loadedImageRunsRef.current.delete(runId);
            console.warn("[Aeroboard] Failed to load image history:", error);
            setLoadStatus((prev) => ({ ...prev, images: "error" }));
        }
    }, [apiBase, refreshRuns]);

    const loadMediaForRun = useCallback(async (runId: string) => {
        if (!runId || deletedRunsRef.current.has(runId) || loadedMediaRunsRef.current.has(runId)) return;
        loadedMediaRunsRef.current.add(runId);

        setLoadStatus((prev) => ({ ...prev, media: "loading" }));
        try {
            // Metadata only: clip bytes are streamed on demand by the <audio>/<video> elements.
            const perKind = await Promise.all(MEDIA_KINDS.map((kind) => fetchMediaHistory(apiBase, kind, runId)));
            for (const clips of perKind) {
                for (const clip of clips) {
                    if (!deletedRunsRef.current.has(clip.run_id)) upsertMediaClip(mediaStoreRef.current, clip);
                }
            }
            refreshRuns();
            setLoadStatus((prev) => ({ ...prev, media: "ready" }));
        } catch (error) {
            loadedMediaRunsRef.current.delete(runId);
            console.warn("[Aeroboard] Failed to load media history:", error);
            setLoadStatus((prev) => ({ ...prev, media: "error" }));
        }
    }, [apiBase, refreshRuns]);

    /**
     * Catch up after the live stream had a gap (reconnect, or the server reported
     * that this tab lagged): re-fetch everything already shown. Merging dedupes.
     */
    const resync = useCallback(() => {
        void loadRuns();
        const historyRuns = Array.from(loadedHistoryRunsRef.current);
        if (historyRuns.length > 0) void fetchHistory(historyRuns, true);
        for (const runId of Array.from(loadedImageRunsRef.current)) {
            loadedImageRunsRef.current.delete(runId);
            void loadImagesForRun(runId);
        }
        for (const runId of Array.from(loadedMediaRunsRef.current)) {
            loadedMediaRunsRef.current.delete(runId);
            void loadMediaForRun(runId);
        }
    }, [fetchHistory, loadImagesForRun, loadMediaForRun, loadRuns]);

    useEffect(() => {
        void loadRuns();
    }, [loadRuns]);

    useEffect(() => {
        let cancelled = false;
        fetchServerDashboardDefaults(apiBase)
            .then(({ defaults, error }) => {
                if (cancelled) return;
                setServerDashboardDefaults(defaults);
                setDashboardConfigError(error);
            })
            .catch((error) => {
                // Older servers have no /api/config: fall back to no defaults.
                console.warn("[Aeroboard] Dashboard defaults unavailable:", error);
                if (!cancelled) setServerDashboardDefaults({});
            });
        return () => {
            cancelled = true;
        };
    }, [apiBase]);

    useEffect(() => {
        if (runs.length === 0) return;
        void loadRunSummaries(runs);
    }, [loadRunSummaries, runs]);

    const handleArrowMessage = useCallback((data: ArrayBuffer) => {
        const table = tableFromIPC(new Uint8Array(data));
        const envelope = table.schema.metadata.get('aeroboard.envelope');
        if (envelope) {
            const id = (JSON.parse(envelope) as {batch_id?: string}).batch_id;
            if (id) {
                const seen = seenBatchesRef.current;
                if (seen.has(id)) return;
                seen.add(id);
                if (seen.size > 8192) seen.delete(seen.values().next().value!);
            }
        }
        let shouldUpdate = false;

        for (const batch of table.batches) {
            const timeCol = batch.getChild('timestamp');
            const runIdCol = batch.getChild('run_id');
            if (!timeCol || !runIdCol) continue;

            const nameCol = batch.getChild('name');
            const valueCol = batch.getChild('value');
            const stepCol = batch.getChild('step');
            const assetIdCol = batch.getChild('asset_id');
            const imageCol = batch.getChild('image_data');
            const audioCol = batch.getChild('audio_data');
            const videoCol = batch.getChild('video_data');
            const formatCol = batch.getChild('format');
            const keyCol = batch.getChild('key');
            const valueTypeCol = batch.getChild('value_type');

            if (nameCol && valueCol && !keyCol) {
                const selected=new Set(runsRef.current.filter(run=>!deletedRunsRef.current.has(run)));
                if (mergeScalarColumns(storeRef.current,summaryRef.current,startTimes.current,[batch],selected)) shouldUpdate=true;
            } else if (nameCol && imageCol && formatCol) {
                for (let i = 0; i < batch.numRows; i++) {
                    if (deletedRunsRef.current.has(String(runIdCol.get(i)))) continue;
                    const payload = imageCol.get(i) as Uint8Array | null;
                    const format = String(formatCol.get(i));
                    upsertImageFrame(imageStoreRef.current, {
                        asset_id: assetIdCol?.get(i) == null ? undefined : String(assetIdCol.get(i)),
                        step: stepCol?.get(i) == null ? null : Number(stepCol.get(i)),
                        ts_ms: Number(timeCol.get(i)),
                        run_id: String(runIdCol.get(i)),
                        name: String(nameCol.get(i)),
                        format,
                        objectUrl: payload && payload.byteLength > 0
                            ? URL.createObjectURL(new Blob([payload as Uint8Array<ArrayBuffer>], { type: `image/${format}` }))
                            : undefined,
                    });
                    shouldUpdate = true;
                }
            } else if (nameCol && formatCol && (audioCol || videoCol)) {
                const kind: MediaKind = audioCol ? 'audio' : 'video';
                const dataCol = (audioCol ?? videoCol)!;
                for (let i = 0; i < batch.numRows; i++) {
                    if (deletedRunsRef.current.has(String(runIdCol.get(i)))) continue;
                    const format = String(formatCol.get(i));
                    const payload = dataCol.get(i) as Uint8Array | null;
                    // The server strips payloads when clips are persisted; those are
                    // fetched lazily. Live-only mode delivers bytes, kept as a Blob URL.
                    const objectUrl = payload && payload.byteLength > 0
                        ? URL.createObjectURL(new Blob([payload as Uint8Array<ArrayBuffer>], { type: mediaMimeType(kind, format) }))
                        : undefined;
                    upsertMediaClip(mediaStoreRef.current, {
                        asset_id: assetIdCol?.get(i) == null ? undefined : String(assetIdCol.get(i)),
                        step: stepCol?.get(i) == null ? null : Number(stepCol.get(i)),
                        kind,
                        ts_ms: Number(timeCol.get(i)),
                        run_id: String(runIdCol.get(i)),
                        name: String(nameCol.get(i)),
                        format,
                        objectUrl,
                    });
                    shouldUpdate = true;
                }
            } else if (keyCol && valueCol && valueTypeCol) {
                for (let i = 0; i < batch.numRows; i++) {
                    if (deletedRunsRef.current.has(String(runIdCol.get(i)))) continue;
                    setHyperparameter(
                        summaryRef.current,
                        dashboardConfigsRef.current,
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
    }, [refreshRuns]);

    useLiveSocket(wsUrl,socketRef,selectedRunsRef,{
        binary:handleArrowMessage,connected:setConnected,reconnect:resync,
        control:message=>{
                        if (message.type === 'lagged') {
                            console.warn(`[Aeroboard] Live stream skipped ${message.skipped ?? '?'} batches; re-fetching history`);
                            resync();
                            window.dispatchEvent(new Event("aeroboard:catalog"));
                         } else if (message.type === 'data_changed') {
                            let changed=false;
                            for(const run of message.runs??[])if(!fetchedRunsRef.current.includes(run)){fetchedRunsRef.current.push(run);changed=true;}
                            if(changed)refreshRuns();
                            window.dispatchEvent(new CustomEvent("aeroboard:data",{detail:message}));
                        } else if (message.type?.endsWith('_changed')) {
                            if(message.kind==='run')void loadRuns();
                            window.dispatchEvent(new CustomEvent("aeroboard:catalog",{detail:message}));
                        }
        },
    });

    useEffect(() => {
        const media = mediaStoreRef;
        const images = imageStoreRef;
        return () => {
            revokeMediaStore(media.current);
            revokeImageStore(images.current);
        };
    }, []);

    const getChartData = useCallback((runId: string, name: string): TimeSeriesData | null => {
        const series=readChartData(storeRef.current,runId,name);
        if(series&&!series.loadWindow){
            const windows=new Set<string>();
            series.loadWindow=async(startMs,endMs)=>{
                const key=`${startMs}:${endMs}`;if(windows.has(key))return;windows.add(key);
                try{
                    const table=await fetchScalarHistory(apiBase,[runId],undefined,{metric:name,start_ms:startMs,end_ms:endMs});
                    if(!deletedRunsRef.current.has(runId))mergeScalarColumns(storeRef.current,summaryRef.current,startTimes.current,table.batches,new Set([runId]));
                }catch(error){windows.delete(key);console.warn('[Aeroboard] Failed to refine chart window',error);}
            };
        }
        return series;
    }, [apiBase]);

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

    const getImageSource = useCallback((frame: ImageFrame, attempt = 0): string => {
        return frame.objectUrl ?? imageContentUrl(apiBase, frame, attempt);
    }, [apiBase]);

    const getMediaSeriesForRun = useCallback((runId: string): MediaSeries[] => {
        return readMediaSeriesForRun(mediaStoreRef.current, runId);
    }, []);

    const getRunDashboardConfig = useCallback((runId: string): PartialDashboardDefaults | null => {
        return dashboardConfigsRef.current[runId] ?? null;
    }, []);

    const getMediaSource = useCallback((clip: MediaClip, attempt = 0): string => {
        return clip.objectUrl ?? mediaContentUrl(apiBase, clip, attempt);
    }, [apiBase]);

    const getGlobalXBounds = useCallback((targetRuns: string[], xAxis: 'wall' | 'rel' | 'step'): [number, number] => {
        return readGlobalXBounds(storeRef.current, targetRuns, xAxis);
    }, []);

    const forgetRuns = useCallback((runIds: string[]) => {
        for (const runId of runIds) {
            deletedRunsRef.current.add(runId);
            delete storeRef.current[runId];
            delete summaryRef.current[runId];
            delete startTimes.current[runId];
            delete dashboardConfigsRef.current[runId];
            if (imageStoreRef.current[runId]) revokeImageStore({ [runId]: imageStoreRef.current[runId] });
            delete imageStoreRef.current[runId];
            if (mediaStoreRef.current[runId]) revokeMediaStore({ [runId]: mediaStoreRef.current[runId] });
            delete mediaStoreRef.current[runId];
            for (const cache of [loadedSummaryRunsRef, loadedHistoryRunsRef, loadedImageRunsRef, loadedMediaRunsRef]) {
                cache.current.delete(runId);
            }
        }
        fetchedRunsRef.current = fetchedRunsRef.current.filter(runId => !deletedRunsRef.current.has(runId));
        refreshRuns();
    }, [refreshRuns]);

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
            storeRef.current = {};
            startTimes.current = {};
            summaryRef.current = {};
            revokeImageStore(imageStoreRef.current);
            imageStoreRef.current = {};
            revokeMediaStore(mediaStoreRef.current);
            mediaStoreRef.current = {};
            dashboardConfigsRef.current = {};
            fetchedRunsRef.current = [];
            for (const loaded of [loadedSummaryRunsRef, loadedHistoryRunsRef, loadedImageRunsRef, loadedMediaRunsRef]) {
                loaded.current.clear();
            }
            runsRef.current = [];
            setRuns([]);
            setTick((t) => t + 1);
            return { ok: true };
        } catch (err) {
            console.error('[Aeroboard] Failed to delete runs:', err);
            return { ok: false, reason: err instanceof Error ? err.message : 'network-error' };
        }
    }, [apiBase]);

    return {
        subscribeRuns,
        connected,
        getChartData,
        getMetricsForRun,
        getGlobalXBounds,
        getRunSummaries,
        getHyperparameterKeys,
        getMetricSummaryKeys,
        getImageSeriesForRun,
        getImageSeries,
        getImageSource,
        getMediaSeriesForRun,
        getMediaSource,
        runs,
        tick,
        loadStatus,
        deleteAllRuns,
        forgetRuns,
        loadRunSummaries,
        loadHistoryForRuns,
        loadImagesForRun,
        loadMediaForRun,
        getRunDashboardConfig,
        serverDashboardDefaults,
        dashboardConfigError,
    };
}
