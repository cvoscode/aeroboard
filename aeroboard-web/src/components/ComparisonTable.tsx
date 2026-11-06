import { useCallback, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import { AgGridReact } from "ag-grid-react";
import {
  ClientSideRowModelModule, ClientSideRowModelApiModule, ColumnApiModule, CsvExportModule,
  NumberFilterModule, QuickFilterModule, RenderApiModule, RowApiModule, TextFilterModule, TooltipModule, ValidationModule, themeQuartz,
  type ColDef, type GetRowIdParams, type GridSizeChangedEvent, type ICellRendererParams, type ModelUpdatedEvent, type RowDataUpdatedEvent,
} from "ag-grid-community";
import type { RunSummary } from "../hooks/useAeroboard";
import type { Score, RunRecord } from "../hooks/useWorkspace";
import type { Reducer } from "../lib/comparison";
import { compareGridValues, comparisonRows, isNumericColumn, type ComparisonGridRow } from "../lib/comparisonGrid";
import { formatConfiguration, parseConfiguration } from "../lib/configuration";

interface ComparisonTableProps {
  runs: RunSummary[];
  selectedHyperparameters: string[];
  selectedMetrics: string[];
  scores: Score[];
  reducer: Reducer;
  onReducerChange: (reducer: Reducer) => void;
  baseline: string;
  onBaselineChange: (run: string) => void;
  comparisonStep: number | null;
  onStepChange: (step: number | null) => void;
  records: RunRecord[];
  comparisonSegment: string;
  onSegmentChange: (value: string) => void;
}

const modules = [ClientSideRowModelModule, ClientSideRowModelApiModule, ColumnApiModule, CsvExportModule,
  NumberFilterModule, QuickFilterModule, RenderApiModule, RowApiModule, TextFilterModule, TooltipModule, ...(import.meta.env.DEV ? [ValidationModule] : [])];
const gridTheme = themeQuartz.withParams({
  accentColor: "var(--accent-primary)", backgroundColor: "var(--bg-card)", foregroundColor: "var(--text-primary)",
  borderColor: "var(--panel-border)", headerBackgroundColor: "var(--surface-subtle)", headerTextColor: "var(--text-secondary)",
  oddRowBackgroundColor: "var(--surface-subtle)", fontFamily: "inherit", fontSize: 13, spacing: 6,
  wrapperBorderRadius: 4, browserColorScheme: "inherit",
});
const getRowId = ({ data }: GetRowIdParams<ComparisonGridRow>) => data.runId;
interface GridContext { inspect: (label: string, raw: string) => void }

function RunCell({ data }: ICellRendererParams<ComparisonGridRow>) {
  if (!data) return null;
  return <div className="comparison-grid-run"><span className="comparison-run-name" title={data.runId}>{data.runId}</span>
    {data.baseline ? <small>Baseline</small> : null}</div>;
}

function StatusCell({ value }: ICellRendererParams<ComparisonGridRow, string>) {
  return <span className="run-status">{value}</span>;
}

function HyperparameterCell({ data, colDef, context }: ICellRendererParams<ComparisonGridRow, string | number, GridContext>) {
  const key = colDef?.colId?.slice(3) ?? "";
  const raw = data?.hyperparameters.get(key);
  if (raw === undefined) return <span className="comparison-missing-value">—</span>;
  const label = `${key} for ${data!.runId}`;
  if (raw.length > 70 || raw.includes("\n") || /^\s*[[{]/.test(raw)) return (
    <button type="button" className="comparison-inspect-value" aria-label={`Inspect ${label}`} title={raw}
      onClick={() => context.inspect(label, raw)}><span aria-hidden="true">↗</span><span>{raw.replace(/\s+/g, " ")}</span></button>
  );
  return <span className="configuration-short-value" title={raw}>{raw || '""'}</span>;
}

function MetricCell({ value, data, colDef }: ICellRendererParams<ComparisonGridRow, number>) {
  const key = colDef?.colId?.slice(7) ?? "";
  const delta = data?.improvements.get(key);
  return <div className="comparison-grid-metric"><span>{value?.toPrecision(5) ?? "—"}</span>
    {delta ? <small className={delta.absolute >= 0 ? "metric-improvement" : "metric-regression"}>
      {delta.absolute >= 0 ? "+" : ""}{delta.absolute.toPrecision(3)} improvement{delta.percent === null ? "" : ` (${delta.percent.toFixed(1)}%)`}
    </small> : null}</div>;
}

const defaultColDef: ColDef<ComparisonGridRow> = {
  sortable: true, resizable: true, filter: "agTextColumnFilter", floatingFilter: true, minWidth: 120,
  initialWidth: 180, cellDataType: false, wrapHeaderText: true,
  comparator: (left, right, _leftNode, _rightNode, descending) => compareGridValues(left, right, descending),
  getQuickFilterText: () => "",
};

export function ComparisonTable({ runs, selectedHyperparameters, selectedMetrics, scores, reducer, onReducerChange, baseline, onBaselineChange, comparisonStep, onStepChange, records, comparisonSegment, onSegmentChange }: ComparisonTableProps) {
  const grid = useRef<AgGridReact<ComparisonGridRow>>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const runColumnLayout = useRef({ compact: false, desktopWidth: 220 });
  const [filterText, setFilterText] = useState("");
  const [displayedCount, setDisplayedCount] = useState(runs.length);
  const [inspection, setInspection] = useState({ label: "Configuration value", text: "" });
  const rowData = useMemo(() => comparisonRows(runs, selectedMetrics, scores, reducer, baseline, records),
    [runs, selectedMetrics, scores, reducer, baseline, records]);
  const context = useMemo<GridContext>(() => ({ inspect: (label, raw) => {
    setInspection({ label, text: formatConfiguration(parseConfiguration(raw)) });
    dialog.current?.showModal();
  } }), []);
  const columnDefs = useMemo<ColDef<ComparisonGridRow>[]>(() => [
    { colId: "run", headerName: "Run", field: "runId", pinned: "left", lockPinned: true, lockPosition: "left",
      initialWidth: 220, minWidth: 140, initialSort: "asc", cellRenderer: RunCell,
      getQuickFilterText: params => params.data?.searchText ?? "" },
    { colId: "status", headerName: "Status", field: "status", initialWidth: 120, cellRenderer: StatusCell },
    ...selectedHyperparameters.map(key => {
      const numeric = isNumericColumn(rowData, key);
      return { colId: `hp:${key}`, headerName: key, headerTooltip: key,
        filter: numeric ? "agNumberColumnFilter" : "agTextColumnFilter",
        valueGetter: (params) => numeric ? params.data?.numericHyperparameters.get(key) ?? null : params.data?.hyperparameters.get(key) ?? null,
        cellRenderer: HyperparameterCell,
      } satisfies ColDef<ComparisonGridRow>;
    }),
    ...selectedMetrics.map(key => ({ colId: `metric:${key}`,
      headerName: `${key} (${reducer === "at_step" ? `step ${comparisonStep ?? "?"}` : reducer})`, headerTooltip: key,
      filter: "agNumberColumnFilter", valueGetter: params => params.data?.metrics.get(key) ?? null, cellRenderer: MetricCell,
    } satisfies ColDef<ComparisonGridRow>)),
  ], [selectedHyperparameters, selectedMetrics, rowData, reducer, comparisonStep]);
  const updateCount = useCallback((event: ModelUpdatedEvent<ComparisonGridRow>) => setDisplayedCount(event.api.getDisplayedRowCount()), []);
  // Baseline badges and deltas can change while a cell's primary value stays the same.
  const refreshContext = useCallback((event: RowDataUpdatedEvent<ComparisonGridRow>) => {
    event.api.refreshCells({ columns: ["run", ...selectedMetrics.map(key => `metric:${key}`)], force: true });
  }, [selectedMetrics]);
  const resizePinnedColumn = useCallback((event: GridSizeChangedEvent<ComparisonGridRow>) => {
    if (!event.clientWidth) return;
    const compact = event.clientWidth < 600;
    const layout = runColumnLayout.current;
    if (compact === layout.compact) return;
    if (compact) layout.desktopWidth = event.api.getColumn("run")?.getActualWidth() ?? 220;
    layout.compact = compact;
    event.api.applyColumnState({ state: [{ colId: "run", width: compact ? 140 : layout.desktopWidth }] });
  }, []);

  const exportCsv = () => grid.current?.api.exportDataAsCsv({ fileName: "comparison.csv",
    processCellCallback: params => {
      const key = params.column.getColId();
      if (key.startsWith("hp:")) return params.node?.data?.hyperparameters.get(key.slice(3)) ?? "";
      return params.value == null ? "" : String(params.value);
    },
  });

  if (runs.length === 0 || (selectedHyperparameters.length === 0 && selectedMetrics.length === 0)) return (
    <div className="comparison-card empty-state-card"><h3>Comparison table</h3>
      <p>Select runs plus at least one hyperparameter or metric to compare.</p></div>
  );

  return <div className="comparison-card comparison-table-card">
    <div className="comparison-card-header">
      <div><h3>Comparison table</h3><p aria-live="polite">{displayedCount} / {runs.length} run{runs.length === 1 ? "" : "s"} visible</p></div>
      <div className="comparison-table-controls">
        <select aria-label="Metric reducer" value={reducer} onChange={event => onReducerChange(event.target.value as Reducer)}>
          <option value="last">Last observation</option><option value="best">Best (defined goal)</option><option value="at_step">At training step</option>
        </select>
        {reducer === "at_step" ? <input type="number" step="any" aria-label="Comparison step" value={comparisonStep ?? ""} onChange={event => onStepChange(event.target.value === "" ? null : Number(event.target.value))} /> : null}
        {reducer === "at_step" ? <label>Segment<input aria-label="Comparison segment" value={comparisonSegment} onChange={event => onSegmentChange(event.target.value)} placeholder="Name, or __global__" /><small>Multiple segments need an explicit name.</small></label> : null}
        <select aria-label="Baseline run" value={baseline} onChange={event => onBaselineChange(event.target.value)}>
          <option value="">No baseline</option>{runs.map(run => <option key={run.run_id} value={run.run_id}>{run.run_id}</option>)}
        </select>
        <button type="button" onClick={exportCsv}>Export table CSV</button>
        <input className="comparison-filter-input" type="search" aria-label="Filter comparison runs" placeholder="Filter runs, hparams, metrics"
          value={filterText} onChange={event => setFilterText(event.target.value)} />
        <button type="button" onClick={() => {setFilterText("");grid.current?.api.setFilterModel(null);}}>Clear filters</button>
      </div>
    </div>
    <p className="comparison-scroll-hint">Click headers to sort; Shift-click to sort by multiple columns. Drag headers to reorder, and their edges to resize.</p>
    <div className="comparison-grid" role="region" aria-label="Run comparison table" style={{ height: Math.min(560, Math.max(240, rowData.length * 52 + 108)) }}>
      <AgGridReact<ComparisonGridRow> ref={grid} modules={modules} theme={gridTheme} context={context}
        rowData={rowData} columnDefs={columnDefs} defaultColDef={defaultColDef} getRowId={getRowId}
        rowHeight={52} headerHeight={52} floatingFiltersHeight={36} rowBuffer={8}
        quickFilterText={filterText} cacheQuickFilter onModelUpdated={updateCount} onRowDataUpdated={refreshContext}
        onGridSizeChanged={resizePinnedColumn} maintainColumnOrder
        suppressDragLeaveHidesColumns enableCellTextSelection ensureDomOrder animateRows={false} />
    </div>
    <dialog ref={dialog} className="workspace-settings-dialog comparison-value-dialog" aria-labelledby="comparison-value-title"
      onClick={event => {if (event.target === event.currentTarget) event.currentTarget.close();}}>
      <section className="workspace-panel">
        <div className="workspace-settings-heading"><h2 id="comparison-value-title">{inspection.label}</h2>
          <button type="button" aria-label="Close configuration value" onClick={() => dialog.current?.close()}><X size={18} /></button>
        </div><pre>{inspection.text}</pre>
      </section>
    </dialog>
  </div>;
}
