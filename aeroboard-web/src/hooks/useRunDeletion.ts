import {useState,type Dispatch,type SetStateAction} from 'react';
import type {RunRecord} from './useWorkspace';
interface Options {
  runs:string[];isPersistent:boolean;records:RunRecord[];projectId:string;
  filteredExperimentGroups:{experimentName:string;runs:unknown[]}[];
  mutate:(path:string,method:string)=>Promise<unknown>;
  forgetRuns:(runs:string[])=>void;
  deleteAllRuns:()=>Promise<{ok:boolean;reason?:string}>;
  setSelectedRuns:Dispatch<SetStateAction<string[]>>;
  resetRange:()=>void;
}
export function useRunDeletion({runs,isPersistent,records:runRecords,projectId,filteredExperimentGroups,mutate,forgetRuns,deleteAllRuns,setSelectedRuns,resetRange}:Options) {
  const [isDeletingRuns, setIsDeletingRuns] = useState(false);
  const [deleteRequest, setDeleteRequest] = useState<{
    title: string; message: string; confirmLabel: string; records: RunRecord[] | null;
  } | null>(null);
  const [deleteError,setDeleteError]=useState<string|null>(null);
  const handleDeleteAllRuns = () => {
    if (runs.length === 0 || isDeletingRuns) return;
    setDeleteError(null);
    setDeleteRequest({title: "Delete all runs", message: "This deletes every run on this server, in all projects, along with its persisted history. This can’t be undone.", confirmLabel: "Delete all", records: null});
  };

  const handleDeleteRuns = (runIds: string[], experimentName?: string) => {
    const hiddenByFilter = experimentName !== undefined
      && (filteredExperimentGroups.find(group => group.experimentName === experimentName)?.runs.length ?? 0) < runIds.length;
    if (isDeletingRuns || !isPersistent) return;
    const records = runIds.map(runId => runRecords.find(run => run.data.run_id === runId && run.parent === projectId));
    if (!records.length || records.some(run => !run)) {
      setDeleteError("Run information changed. Refresh the dashboard and try again.");
      return;
    }
    setDeleteError(null);
    const running = (records as RunRecord[]).filter(run => run.data.display_status === "running").length;
    const runningNote = running === 0 ? ""
      : experimentName ? ` ${running} ${running === 1 ? "is" : "are"} still running: training continues, but new data will be rejected.`
      : " It’s still running: training continues, but new data will be rejected.";
    setDeleteRequest({
      title: experimentName ? "Delete run group" : "Delete run",
      message: experimentName
        ? `Delete all ${records.length} run version${records.length === 1 ? "" : "s"} in “${experimentName}” and their persisted data?${hiddenByFilter ? " This includes versions hidden by the filter." : ""}${runningNote} This can’t be undone.`
        : `Delete “${runIds[0]}” and its persisted data?${runningNote} This can’t be undone.`,
      confirmLabel: experimentName ? "Delete group" : "Delete run",
      records: records as RunRecord[],
    });
  };

  const handleConfirmDelete = async () => {
    if (!deleteRequest || isDeletingRuns) return;
    setDeleteRequest(null);
    setIsDeletingRuns(true);

    if (deleteRequest.records) {
      const failures: string[] = [];
      for (const run of deleteRequest.records) {
        try {
          await mutate(`/run-records/${encodeURIComponent(run.id)}?revision=${run.data.metadata_revision}`, "DELETE");
          forgetRuns([run.data.run_id]);
          setSelectedRuns(previous => previous.filter(id => id !== run.data.run_id));
        } catch (error) {
          failures.push(`${run.data.run_id}: ${error instanceof Error ? error.message : String(error)}`);
        }
      }
      setIsDeletingRuns(false);
      setDeleteError(failures.length ? `Could not delete ${failures.length} run${failures.length === 1 ? "" : "s"}. ${failures.join("; ")}` : null);
      return;
    }

    const result = await deleteAllRuns();
    setIsDeletingRuns(false);

    if (!result.ok) {
      if (result.reason === "server-restart-required") {
        setDeleteError("Delete All Runs needs the updated server route. Restart `uv run aeroboard start` and try again.");
      } else {
        setDeleteError(`Failed to delete all runs. ${result.reason ?? "Make sure the Aeroboard server and GreptimeDB are running."}`);
      }
      return;
    }

    setSelectedRuns([]);
    resetRange();
    setDeleteError(null);
  };

  return {isDeletingRuns,deleteRequest,setDeleteRequest,deleteError,setDeleteError,handleDeleteAllRuns,handleDeleteRuns,handleConfirmDelete};
}
