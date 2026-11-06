export interface RunEntry {
  fullRunId: string;
  experimentName: string;
  versionLabel: string;
  versionNumber: number;
  /** When the run started, or 0 if unknown. */
  startedAtMs: number;
}

export interface ExperimentGroup {
  experimentName: string;
  runs: RunEntry[];
  latestRunId: string;
  latestStartedAtMs: number;
  /** Versions in the group, including any a filter hides. */
  totalRuns: number;
}

export function parseRun(runId: string, startedAtMs = 0): RunEntry {
  const versionMatch = runId.match(/^(.*?)(?:-v(\d+))?$/);
  const baseWithoutVersion = versionMatch?.[1] ?? runId;
  const versionNumber = versionMatch?.[2] ? Number(versionMatch[2]) : 1;
  const lightningMatch = baseWithoutVersion.match(/^(.*?)(-pl.+)$/);
  const experimentName = (lightningMatch?.[1] ?? baseWithoutVersion).trim() || runId;
  const versionLabel = versionMatch?.[2] ? `v${versionMatch[2]}` : "v1";

  return {
    fullRunId: runId,
    experimentName,
    versionLabel,
    versionNumber,
    startedAtMs,
  };
}

/** Name for a run shown under its group heading, which already gives the experiment name. */
export function runLabelInGroup(run: RunEntry): string {
  const variant = run.fullRunId.slice(run.experimentName.length).replace(/^-/, "").replace(/-?v\d+$/, "");
  return variant ? `${variant} ${run.versionLabel}` : run.versionLabel;
}

function newestFirst(a: RunEntry, b: RunEntry): number {
  if (b.startedAtMs !== a.startedAtMs) return b.startedAtMs - a.startedAtMs;
  if (b.versionNumber !== a.versionNumber) return b.versionNumber - a.versionNumber;
  return b.fullRunId.localeCompare(a.fullRunId);
}

/** Run IDs newest first; runs with no start time keep their relative order at the end. */
export function sortRunsNewestFirst(runs: string[], startedAtMs: Record<string, number> = {}): string[] {
  return [...runs].sort((a, b) => (startedAtMs[b] ?? 0) - (startedAtMs[a] ?? 0));
}

/** Groups and the runs inside them, newest first; groups with no start times sort by name. */
export function buildExperimentGroups(runs: string[], startedAtMs: Record<string, number> = {}): ExperimentGroup[] {
  const grouped = new Map<string, RunEntry[]>();

  for (const runId of runs) {
    const entry = parseRun(runId, startedAtMs[runId]);
    const bucket = grouped.get(entry.experimentName) ?? [];
    bucket.push(entry);
    grouped.set(entry.experimentName, bucket);
  }

  return Array.from(grouped.entries())
    .map(([experimentName, entries]) => {
      const sortedRuns = [...entries].sort(newestFirst);

      return {
        experimentName,
        runs: sortedRuns,
        latestRunId: sortedRuns[0]?.fullRunId ?? experimentName,
        latestStartedAtMs: sortedRuns[0]?.startedAtMs ?? 0,
        totalRuns: sortedRuns.length,
      };
    })
    .sort((a, b) => b.latestStartedAtMs - a.latestStartedAtMs || a.experimentName.localeCompare(b.experimentName));
}

export function filterExperimentGroups(groups: ExperimentGroup[], rawFilter: string): ExperimentGroup[] {
  const query = rawFilter.trim().toLowerCase();
  if (!query) return groups;

  return groups
    .map((group) => {
      const experimentMatches = group.experimentName.toLowerCase().includes(query);
      const matchingRuns = experimentMatches
        ? group.runs
        : group.runs.filter((run) => `${run.fullRunId} ${run.versionLabel}`.toLowerCase().includes(query));

      if (matchingRuns.length === 0) return null;

      return {
        ...group,
        runs: matchingRuns,
        latestRunId: matchingRuns[0]?.fullRunId ?? group.latestRunId,
      };
    })
    .filter((group): group is ExperimentGroup => group !== null);
}
