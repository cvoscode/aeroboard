export interface RunEntry {
  fullRunId: string;
  experimentName: string;
  versionLabel: string;
  versionNumber: number;
}

export interface ExperimentGroup {
  experimentName: string;
  runs: RunEntry[];
  latestRunId: string;
}

export function parseRun(runId: string): RunEntry {
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
  };
}

export function buildExperimentGroups(runs: string[]): ExperimentGroup[] {
  const grouped = new Map<string, RunEntry[]>();

  for (const runId of runs) {
    const entry = parseRun(runId);
    const bucket = grouped.get(entry.experimentName) ?? [];
    bucket.push(entry);
    grouped.set(entry.experimentName, bucket);
  }

  return Array.from(grouped.entries())
    .map(([experimentName, entries]) => {
      const sortedRuns = [...entries].sort((a, b) => {
        if (b.versionNumber !== a.versionNumber) return b.versionNumber - a.versionNumber;
        return b.fullRunId.localeCompare(a.fullRunId);
      });

      return {
        experimentName,
        runs: sortedRuns,
        latestRunId: sortedRuns[0]?.fullRunId ?? experimentName,
      };
    })
    .sort((a, b) => a.experimentName.localeCompare(b.experimentName));
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
