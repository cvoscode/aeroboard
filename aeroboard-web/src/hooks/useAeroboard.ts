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

type Store = Record<string, Record<string, TimeSeriesData>>;

function makeEmptySeries(): TimeSeriesData {
    return {
        wallTimes: new Float64Array(200000),
        relTimes: new Float64Array(200000),
        steps: new Float64Array(200000).fill(-1),
        plotX: new Float64Array(200000),
        values: new Float64Array(200000),
        count: 0,
    };
}

function ingestPoint(
    store: Store,
    startTimes: Record<string, number>,
    runId: string,
    name: string,
    tsMs: number,
    val: number,
    step: number | null
): boolean {
    const wallTime = tsMs / 1000;

    if (!startTimes[runId]) startTimes[runId] = wallTime;
    const relTime = wallTime - startTimes[runId];

    if (!store[runId]) store[runId] = {};
    const runStore = store[runId];

    if (!runStore[name]) runStore[name] = makeEmptySeries();
    const series = runStore[name];

    if (series.count >= series.values.length) return false; // buffer full

    const i = series.count;
    series.wallTimes[i] = wallTime;
    series.relTimes[i] = relTime;
    series.plotX[i] = step ?? relTime;
    series.values[i] = val;
    if (step !== null) series.steps[i] = step;
    series.count = i + 1;
    return true;
}

function clearStore(store: React.MutableRefObject<Store>, startTimes: React.MutableRefObject<Record<string, number>>) {
    store.current = {};
    startTimes.current = {};
}

export function useAeroboard(wsUrl: string = 'ws://localhost:3000/ws/live') {
    const [connected, setConnected] = useState(false);
    const storeRef = useRef<Store>({});
    const startTimes = useRef<Record<string, number>>({});
    const [runs, setRuns] = useState<string[]>([]);
    const [tick, setTick] = useState(0);
    const [reloadToken, setReloadToken] = useState(0);

    const refreshRuns = useCallback(() => {
        setRuns(Object.keys(storeRef.current).sort());
        setTick(t => t + 1);
    }, []);

    // ── Load historical data from GreptimeDB via REST on mount ────────────────
    useEffect(() => {
        const loadHistory = async () => {
            try {
                const resp = await fetch('http://localhost:3000/api/history');
                if (!resp.ok) return;
                const json = await resp.json() as { rows: { ts_ms: number; run_id: string; name: string; value: number }[] };

                if (!json.rows?.length) return;

                // Sort by ts_ms so step/relTime is computed correctly
                json.rows.sort((a, b) => a.ts_ms - b.ts_ms);

                // Group step by run: for each ts_ms bucket, gather the 'step' value first
                // Build a ts->step map per run for alignment
                const stepMap: Record<string, Map<number, number>> = {};
                for (const r of json.rows) {
                    if (r.name === 'step' || r.name.endsWith('/step')) {
                        if (!stepMap[r.run_id]) stepMap[r.run_id] = new Map();
                        stepMap[r.run_id].set(r.ts_ms, r.value);
                    }
                }

                let newData = false;
                for (const row of json.rows) {
                    if (row.name === 'step' || row.name.endsWith('/step')) continue;
                    const step = stepMap[row.run_id]?.get(row.ts_ms) ?? null;
                    const added = ingestPoint(storeRef.current, startTimes.current, row.run_id, row.name, row.ts_ms, row.value, step);
                    if (added) newData = true;
                }

                if (newData) {
                    refreshRuns();
                }
            } catch (e) {
                console.warn('[Aeroboard] Failed to load history (GreptimeDB may be unavailable):', e);
            }
        };
        loadHistory();
    }, [refreshRuns, reloadToken]);

    // ── WebSocket for live data ───────────────────────────────────────────────
    useEffect(() => {
        const ws = new WebSocket(wsUrl);
        ws.binaryType = 'arraybuffer';

        ws.onopen = () => setConnected(true);
        ws.onclose = () => setConnected(false);

        ws.onmessage = async (event) => {
            try {
                if (!(event.data instanceof ArrayBuffer)) return;
                const table = arrow.tableFromIPC(event.data);

                let shouldUpdate = false;
                let foundNewRun = false;

                for (const batch of table.batches) {
                    const timeCol = batch.getChild('timestamp');
                    const runIdCol = batch.getChild('run_id');
                    const nameCol = batch.getChild('name');
                    const valCol = batch.getChild('value');

                    if (!timeCol || !runIdCol || !nameCol || !valCol) continue;

                    // Find the step value for this batch (if any)
                    let batchStep: number | null = null;
                    for (let i = 0; i < batch.numRows; i++) {
                        const n = String(nameCol.get(i));
                        if (n === 'step' || n.endsWith('/step')) {
                            batchStep = Number(valCol.get(i));
                            break;
                        }
                    }

                    const prevRunCount = Object.keys(storeRef.current).length;

                    for (let i = 0; i < batch.numRows; i++) {
                        const name = String(nameCol.get(i));
                        if (name === 'step' || name.endsWith('/step')) continue;

                        const tsMs = Number(timeCol.get(i));
                        const runId = String(runIdCol.get(i));
                        const val = Number(valCol.get(i));

                        const added = ingestPoint(storeRef.current, startTimes.current, runId, name, tsMs, val, batchStep);
                        if (added) shouldUpdate = true;
                    }

                    if (Object.keys(storeRef.current).length > prevRunCount) foundNewRun = true;
                }

                if (foundNewRun) setRuns(Object.keys(storeRef.current).sort());
                if (shouldUpdate) setTick(t => t + 1);
            } catch (err) {
                console.error('Failed to parse Arrow batch from WS:', err);
            }
        };

        return () => ws.close();
    }, [wsUrl]);

    const getChartData = useCallback((runId: string, name: string): TimeSeriesData | null => {
        if (!runId) return null;
        const series = storeRef.current[runId]?.[name];
        return series && series.count > 0 ? series : null;
    }, []);

    const getMetricsForRun = useCallback((runId: string): string[] => {
        if (!runId) return [];
        return Object.keys(storeRef.current[runId] ?? {});
    }, []);

    const getGlobalXBounds = useCallback((targetRuns: string[], xAxis: 'wall' | 'rel' | 'step'): [number, number] => {
        let min = Infinity, max = -Infinity;
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
            const resp = await fetch('http://localhost:3000/api/history', { method: 'DELETE' });
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
            clearStore(storeRef, startTimes);
            setRuns([]);
            setTick(t => t + 1);
            setReloadToken(t => t + 1);
            return { ok: true };
        } catch (err) {
            console.error('Failed to delete runs:', err);
            return { ok: false, reason: err instanceof Error ? err.message : 'network-error' };
        }
    }, []);

    return { connected, getChartData, getMetricsForRun, getGlobalXBounds, runs, tick, deleteAllRuns };
}
