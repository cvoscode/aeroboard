import { tableFromIPC } from "apache-arrow";
import type { ImageFrame, MediaClip, MediaKind, RunSummary, ScalarRow } from "./types";
import { parseDashboardDefaults, type PartialDashboardDefaults } from "../../lib/dashboardDefaults";

function buildUrl(base: string, path: string, params?: Record<string, string | number | boolean | undefined>) {
  const url = new URL(path, base);
  if (params) {
    for (const [key, value] of Object.entries(params)) {
      if (value === undefined) continue;
      url.searchParams.set(key, String(value));
    }
  }
  return url.toString();
}

async function fetchJson<T>(url: string): Promise<T> {
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(`Request failed with status ${resp.status}`);
  }
  return resp.json() as Promise<T>;
}

async function fetchPagedRows<T>(url: string): Promise<T[]> {
  const firstPage = await fetchJson<{ rows?: T[]; next_cursor?: number | null }>(
    `${url}${url.includes("?") ? "&" : "?"}limit=10000&cursor=0`,
  );
  const rows = [...(firstPage.rows ?? [])];
  let nextCursor = firstPage.next_cursor ?? null;

  while (nextCursor !== null) {
    const page = await fetchJson<{ rows?: T[]; next_cursor?: number | null }>(
      `${url}${url.includes("?") ? "&" : "?"}limit=10000&cursor=${nextCursor}`,
    );
    rows.push(...(page.rows ?? []));
    nextCursor = page.next_cursor ?? null;
  }

  return rows;
}

export async function fetchRuns(apiBase: string): Promise<string[]> {
  const data = await fetchJson<{ runs?: string[] }>(buildUrl(apiBase, "/api/runs"));
  return (data.runs ?? []).sort();
}

export async function fetchRunSummaries(apiBase: string, runIds?: string[]): Promise<RunSummary[]> {
  const data = await fetchJson<{ run_summaries?: RunSummary[] }>(
    buildUrl(apiBase, "/api/runs/summary", {
      run_id: runIds && runIds.length > 0 ? runIds.join(",") : undefined,
    }),
  );
  return data.run_summaries ?? [];
}

/**
 * Scalar history for `runIds`, sorted by time. Fetched as a single Arrow IPC
 * stream: values stay binary and run/metric names are dictionary-encoded, which
 * is several times smaller than the JSON rows and avoids paging.
 */
export async function fetchScalarHistory(apiBase: string, runIds: string[]): Promise<ScalarRow[]> {
  const resp = await fetch(buildUrl(apiBase, "/api/history", { run_id: runIds.join(","), format: "arrow" }));
  if (!resp.ok) {
    throw new Error(`Request failed with status ${resp.status}`);
  }
  const table = tableFromIPC(new Uint8Array(await resp.arrayBuffer()));
  const rows: ScalarRow[] = [];
  for (const batch of table.batches) {
    const timestamps = batch.getChild("timestamp");
    const runs = batch.getChild("run_id");
    const names = batch.getChild("name");
    const values = batch.getChild("value");
    const steps = batch.getChild("step");
    if (!timestamps || !runs || !names || !values) continue;
    for (let i = 0; i < batch.numRows; i++) {
      const step = steps?.get(i);
      rows.push({
        ts_ms: Number(timestamps.get(i)),
        run_id: String(runs.get(i)),
        name: String(names.get(i)),
        value: Number(values.get(i)),
        step: step === null || step === undefined ? null : Number(step),
      });
    }
  }
  rows.sort((a, b) => a.ts_ms - b.ts_ms);
  return rows;
}

/** Image metadata for a run; frame bytes load lazily via `imageContentUrl`. */
export async function fetchImageHistory(apiBase: string, runId: string): Promise<ImageFrame[]> {
  return fetchPagedRows<ImageFrame>(
    buildUrl(apiBase, "/api/history/images", {
      run_id: runId,
      include_data: false,
    }),
  );
}

/** URL of a persisted image frame's raw bytes (usable directly as `<img src>`). */
export function imageContentUrl(apiBase: string, frame: ImageFrame, attempt = 0): string {
  if (frame.asset_id) return buildUrl(apiBase,`/api/assets/${encodeURIComponent(frame.asset_id)}/content`,{attempt: attempt || undefined});
  return buildUrl(apiBase, "/api/history/media/content", {
    kind: "image",
    run_id: frame.run_id,
    name: frame.name,
    ts_ms: frame.ts_ms,
    attempt: attempt > 0 ? attempt : undefined,
  });
}

export async function fetchMediaHistory(apiBase: string, kind: MediaKind, runId: string): Promise<MediaClip[]> {
  const rows = await fetchPagedRows<Omit<MediaClip, "kind">>(
    buildUrl(apiBase, "/api/history/media", { kind, run_id: runId }),
  );
  return rows.map((row) => ({ ...row, kind }));
}

/** URL of the raw clip bytes; the server streams it with Range support. */
export function mediaContentUrl(apiBase: string, clip: MediaClip, attempt = 0): string {
  if (clip.asset_id) return buildUrl(apiBase,`/api/assets/${encodeURIComponent(clip.asset_id)}/content`,{attempt: attempt || undefined});
  return buildUrl(apiBase, "/api/history/media/content", {
    kind: clip.kind,
    run_id: clip.run_id,
    name: clip.name,
    ts_ms: clip.ts_ms,
    attempt: attempt > 0 ? attempt : undefined,
  });
}

/** Server-wide dashboard defaults from `aeroboard.toml`; `error` describes a broken config file. */
export async function fetchServerDashboardDefaults(apiBase: string): Promise<{ defaults: PartialDashboardDefaults; error: string | null }> {
  const body = await fetchJson<{ dashboard?: unknown; error?: string }>(buildUrl(apiBase, "/api/config"));
  return { defaults: parseDashboardDefaults(body.dashboard), error: body.error ?? null };
}
