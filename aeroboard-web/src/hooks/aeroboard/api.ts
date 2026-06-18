import type { ImageFrame, RunSummary, ScalarRow } from "./types";

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

export async function fetchScalarHistory(apiBase: string, runIds: string[]): Promise<ScalarRow[]> {
  const rows = await fetchPagedRows<ScalarRow>(
    buildUrl(apiBase, "/api/history", {
      run_id: runIds.join(","),
      include_summaries: false,
    }),
  );
  rows.sort((a, b) => a.ts_ms - b.ts_ms);
  return rows;
}

export async function fetchImageHistory(apiBase: string, runId: string): Promise<ImageFrame[]> {
  return fetchPagedRows<ImageFrame>(
    buildUrl(apiBase, "/api/history/images", {
      run_id: runId,
      include_data: false,
    }),
  );
}

export async function fetchImageFrame(
  apiBase: string,
  runId: string,
  name: string,
  tsMs: number,
): Promise<ImageFrame | null> {
  const data = await fetchJson<{
    run_id: string;
    name: string;
    ts_ms: number;
    format: string;
    image_base64: string;
    error?: string | null;
  }>(
    buildUrl(apiBase, "/api/history/images/frame", {
      run_id: runId,
      name,
      ts_ms: tsMs,
    }),
  );

  if (data.error) return null;

  return {
    run_id: data.run_id,
    name: data.name,
    ts_ms: data.ts_ms,
    format: data.format,
    image_base64: data.image_base64,
  };
}
