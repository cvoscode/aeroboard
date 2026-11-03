import { useCallback, useEffect, useState } from "react";

export interface Entity<T = Record<string, unknown>> { id: string; parent: string; revision: number; data: T }
export interface RunData {
  run_id: string; display_name: string; project_id: string; status: string; display_status: string;
  tags: string[]; notes: string; archived: boolean; provenance: Record<string, unknown>;
  started_at_ms: number | null; ended_at_ms: number | null; last_heartbeat_ms: number | null;
  active_session: string | null; attempt: number; error?: string | null;
  metadata_revision: number;
  delivery?: {status: string; committed_batches: number; final_sequence: number | null; dropped_rows: number};
}
export type RunRecord = Entity<RunData>;
export type Project = Entity<{ name: string; default_view: string | null }>;
export type SavedView = Entity<{name: string; settings: Record<string, unknown>; schema_version: number}>;
export type MetricDefinition = Entity<{name: string; goal: "minimize" | "maximize" | null; target: number | null; unit: string | null}>;
export type RunEvent = Entity<{label: string; kind: string; step: number | null; timestamp_ms: number; details: Record<string, unknown>; media_ids: string[]; run_uid: string}>;
export interface Score {
  run_id: string; name: string; goal: "minimize" | "maximize" | null; target: number | null; unit: string | null;
  best: number | null; best_step: number | null; best_at_ms: number | null; last: number | null;
  last_step: number | null; last_is_finite: boolean; finite_count: number; at_step: number | null;
  time_to_target_ms: number | null; target_at_ms: number | null;
}
export interface Capabilities { server_id: string; persistent: boolean; features: string[]; durable_replay: boolean }

export async function workspaceRequest<T>(path: string, method = "GET", body?: unknown, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`/api${path}`, {method, signal, headers: body === undefined ? {} : {"Content-Type": "application/json"}, body: body === undefined ? undefined : JSON.stringify(body)});
  if (!response.ok) {
    const error = await response.json().catch(() => ({error: response.statusText}));
    throw new Error(`${response.status}: ${error.error ?? response.statusText}`);
  }
  return response.json() as Promise<T>;
}

async function allRecords(signal: AbortSignal): Promise<RunRecord[]> {
  const rows: RunRecord[] = [];
  let offset: number | null = 0;
  while (offset !== null) {
    const page: {rows: RunRecord[]; next_offset: number | null} = await workspaceRequest(`/run-records?archived=true&limit=1000&offset=${offset}`, "GET", undefined, signal);
    rows.push(...page.rows);
    offset = page.next_offset;
  }
  return rows;
}

export function useWorkspace(project: string) {
  const [capabilities, setCapabilities] = useState<Capabilities | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [records, setRecords] = useState<RunRecord[]>([]);
  const [views, setViews] = useState<SavedView[]>([]);
  const [definitions, setDefinitions] = useState<MetricDefinition[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [updated, setUpdated] = useState(0);
  const refresh = useCallback(() => setRevision(v => v + 1), []);
  useEffect(() => {
    const abort = new AbortController();
    const signal = abort.signal;
    let pending: ReturnType<typeof setTimeout> | undefined;
    let loading = false;
    let reload = false;
    const load = async () => {
      if (loading) {reload = true; return;}
      loading = true;
      try {
        const capabilities = await workspaceRequest<Capabilities>("/capabilities", "GET", undefined, signal);
        const [projects, records, views, definitions] = await Promise.all([
          workspaceRequest<Project[]>("/projects", "GET", undefined, signal), allRecords(signal),
          workspaceRequest<SavedView[]>(`/projects/${encodeURIComponent(project)}/views`, "GET", undefined, signal),
          workspaceRequest<MetricDefinition[]>("/metric-definitions", "GET", undefined, signal),
        ]);
        if (signal.aborted) return;
        setCapabilities(capabilities); setProjects(projects); setRecords(records); setViews(views); setDefinitions(definitions); setError(null);
        setUpdated(value => value + 1);
      } catch (error) {
        if (!signal.aborted && !(error instanceof Error && error.message.startsWith("404:"))) setError(String(error));
      } finally {
        loading = false;
        if (reload && !signal.aborted) {reload = false; void load();}
      }
    };
    const schedule = () => { clearTimeout(pending); pending = setTimeout(() => void load(), 150); };
    void load();
    const timer = setInterval(schedule, 15_000);
    window.addEventListener("aeroboard:catalog", schedule);
    return () => {abort.abort(); clearInterval(timer); clearTimeout(pending); window.removeEventListener("aeroboard:catalog", schedule);};
  }, [project, revision]);
  const mutate = useCallback(async <T,>(path: string, method: string, body?: unknown) => {
    try { const value = await workspaceRequest<T>(path, method, body); refresh(); return value; }
    catch (error) {setError(String(error)); throw error;}
  }, [refresh]);
  return {capabilities, projects, records, views, definitions, error, setError, refresh, mutate, updated};
}

export function useScores(runIds: string[], step: number | null, revision: number, segment = "") {
  const [result, setResult] = useState<{key: string; rows: Score[]}>({key: "", rows: []});
  const [error, setError] = useState<string | null>(null);
  const key = runIds.join(",");
  const requestKey = JSON.stringify([key,step,segment]);
  useEffect(() => {
    if (!key) return;
    const abort = new AbortController();
    const load = () => {
      const params = new URLSearchParams({run_id: key});
      if (step !== null) params.set("step", String(step));
      if (segment) params.set("segment",segment);
      workspaceRequest<{rows: Score[]}>(`/scorecards?${params}`, "GET", undefined, abort.signal)
        .then(value => {if (!abort.signal.aborted) {setResult({key: requestKey,rows: value.rows}); setError(null);}})
        .catch(error => {if (!abort.signal.aborted) setError(String(error));});
    };
    load();
    const timer = setInterval(load, 3000);
    return () => {abort.abort(); clearInterval(timer);};
  }, [key, step, revision, segment, requestKey]);
  return {scores: key && result.key === requestKey ? result.rows : [], error};
}
