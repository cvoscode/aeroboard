import React, { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { Activity } from "lucide-react";

import "./App.css";
import "./workspace.css";
import { DashboardHeader } from "./components/DashboardHeader";
import { DashboardSidebar } from "./components/DashboardSidebar";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { MetricChartGrid } from "./components/MetricChartGrid";
import { useAeroboard } from "./hooks/useAeroboard";
import { useDashboardPreferences, type WidthMode } from "./hooks/useDashboardPreferences";
import { usePersistentState } from "./hooks/usePersistentState";
import { useWorkspace, useScores, type RunEvent, type SavedView } from "./hooks/useWorkspace";
import { WorkspacePanel } from "./components/WorkspacePanel";
import type { Reducer } from "./lib/comparison";
import { buildExperimentGroups, filterExperimentGroups } from "./lib/runGroups";
import type { XAxisType } from "./components/LineChart";

const ComparePanels = lazy(() => import("./components/ComparePanels"));
const ImagePanel = lazy(() => import("./components/ImagePanel"));
import {
  defaultOrderRank,
  isStarredByDefault,
  resolveDashboardDefaults,
  parseDashboardDefaults,
  matchesChart,
} from "./lib/dashboardDefaults";
const MediaPanel = lazy(() => import("./components/MediaPanel"));

type DashboardTab = "scalars" | "images" | "media" | "analysis";
type ThemeMode = "light" | "dark";

const THEME_STORAGE_KEY = "aeroboard-theme";
const DEFAULT_COLORS = ["#00d2ff", "#ff4c6b", "#00e676", "#ffeb3b", "#e040fb", "#ff9800", "#9c27b0"];

interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

interface ErrorBoundaryProps {
  children: React.ReactNode;
  fallback?: React.ReactNode;
}

class ErrorBoundary extends React.Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: React.ErrorInfo) {
    console.error("[Aeroboard] Uncaught error:", error, errorInfo);
  }

  render() {
    if (!this.state.hasError) return this.props.children;

    if (this.props.fallback) return this.props.fallback;

    return (
      <div className="fatal-shell">
        <h1>Something went wrong</h1>
        <p>{this.state.error?.message || "An unexpected error occurred"}</p>
        <button type="button" onClick={() => window.location.reload()}>
          Reload Dashboard
        </button>
      </div>
    );
  }
}

function getInitialTheme(): ThemeMode {
  if (typeof window === "undefined") return "dark";

  const savedTheme = window.localStorage.getItem(THEME_STORAGE_KEY);
  if (savedTheme === "light" || savedTheme === "dark") {
    return savedTheme;
  }

  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/**
 * Charts read their colors from CSS variables while rendering, and child effects run
 * before this component's effects, so the theme attribute must be on <html> before the
 * render that uses it, not in a useEffect after it.
 */
function applyThemeToDocument(theme: ThemeMode) {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

function asObject(raw: unknown): Record<string, unknown> {
  return raw && typeof raw === "object" && !Array.isArray(raw) ? raw as Record<string, unknown> : {};
}
function names(raw: unknown): string[] { return Array.isArray(raw) ? raw.filter((value): value is string => typeof value === "string") : []; }
function flags(raw: unknown): Record<string, boolean> {return Object.fromEntries(Object.entries(asObject(raw)).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"));}

function App() {
  const {
    connected,
    getChartData,
    loadStatus,
    loadRunSummaries,
    loadHistoryForRuns,
    loadImagesForRun,
    getImageSource,
    getMetricsForRun,
    getGlobalXBounds,
    getRunSummaries,
    getHyperparameterKeys,
    getMetricSummaryKeys,
    getImageSeriesForRun,
    getImageSeries,
    getMediaSeriesForRun,
    getMediaSource,
    loadMediaForRun,
    runs,
    tick,
    deleteAllRuns,
    getRunDashboardConfig,
    serverDashboardDefaults,
    dashboardConfigError,
  } = useAeroboard();

  const [projectId, setProjectId] = useState(() => new URLSearchParams(window.location.search).get("project") || "default");
  const [viewChoice, setViewChoice] = useState(() => new URLSearchParams(window.location.search).get("view") ?? "__default__");
  const workspace = useWorkspace(projectId);
  const defaultView = workspace.projects.find(project => project.id === projectId)?.data.default_view;
  const activeView = workspace.views.find(view => view.id === (viewChoice === "__default__" ? defaultView : viewChoice));
  const viewSettings = activeView?.data.settings ?? {};
  const viewControls = asObject(viewSettings.controls);
  const scope = workspace.capabilities ? `${workspace.capabilities.server_id}:${projectId}:${activeView?.id ?? "defaults"}` : "legacy";
  const [controlOverrides, setControlOverrides] = usePersistentState<Record<string, unknown>>(`aeroboard-controls:${scope}`, {}, asObject);
  const controls = {...viewControls, ...controlOverrides};
  const updateControl = <T,>(key: string, update: React.SetStateAction<T>, fallback: T) => {
    setControlOverrides(previous => {
      const value = (key in previous ? previous[key] : viewControls[key] ?? fallback) as T;
      return {...previous, [key]: typeof update === "function" ? (update as (previous: T) => T)(value) : update};
    });
  };

  const {
    favoriteCharts,
    setFavoriteCharts,
    collapsedCharts,
    setCollapsedCharts,
    orderedMetrics,
    setOrderedMetrics,
    collapsedExperiments,
    setCollapsedExperiments,
    runColors,
    setRunColors,
    optimalValues,
    setOptimalValues,
    resetChartView,
    hasChartOverrides,
  } = useDashboardPreferences(scope);
  const [configErrorDismissed, setConfigErrorDismissed] = useState(false);

  const mode = controls.mode === "compare" ? "compare" : "single";
  const setMode = (value: "single" | "compare") => updateControl("mode",value,"single");
  const [selectedRuns, setSelectedRuns] = useState<string[]>(() => new URLSearchParams(window.location.search).get("runs")?.split(",").filter(Boolean) ?? []);
  const xAxisType: XAxisType = controls.xAxisType === "wall" || controls.xAxisType === "rel" ? controls.xAxisType : "step";
  const setXAxisType = (value: XAxisType) => updateControl("xAxisType",value,"step");
  const smoothing = typeof controls.smoothing === "number" ? Math.min(1,Math.max(0,controls.smoothing)) : 0;
  const setSmoothing = (value: number) => updateControl("smoothing",value,0);
  const removeOutliers = controls.removeOutliers === true;
  const setRemoveOutliers = (value: boolean) => updateControl("removeOutliers",value,false);
  const widthMode: WidthMode = controls.widthMode === "compact" || controls.widthMode === "full" ? controls.widthMode : "wide";
  const setWidthMode = (value: WidthMode) => updateControl("widthMode",value,"wide");
  const globalXRange: [number,number] | null = Array.isArray(controls.xRange) && controls.xRange.length === 2 && controls.xRange.every(value => typeof value === "number" && Number.isFinite(value)) ? controls.xRange as [number,number] : null;
  const setGlobalXRange = (value: [number,number] | null) => updateControl("xRange",value,null);
  const metricSearch = typeof controls.metricSearch === "string" ? controls.metricSearch : "";
  const comparisonReducer: Reducer = controls.reducer === "best" || controls.reducer === "at_step" ? controls.reducer : "last";
  const baseline = typeof controls.baseline === "string" ? controls.baseline : "";
  const comparisonStep = typeof controls.comparisonStep === "number" && Number.isFinite(controls.comparisonStep) ? controls.comparisonStep : null;
  const comparisonSegment = typeof controls.comparisonSegment === "string" ? controls.comparisonSegment : "";
  const logCharts = flags(controls.logCharts);
  const hiddenCharts = names(controls.hiddenCharts);
  const [runEvents, setRunEvents] = useState<RunEvent[]>([]);
  const [focusedEvent, setFocusedEvent] = useState<RunEvent | null>(null);
  const [isDeletingRuns, setIsDeletingRuns] = useState(false);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const selectedCompareHyperparameters = names(controls.compareHyperparameters);
  const setSelectedCompareHyperparameters = (value: React.SetStateAction<string[]>) => updateControl("compareHyperparameters",value,[]);
  const selectedCompareMetrics = names(controls.compareMetrics);
  const setSelectedCompareMetrics = (value: React.SetStateAction<string[]>) => updateControl("compareMetrics",value,[]);
  const [selectedImageName, setSelectedImageName] = useState("");
  const [runFilter, setRunFilter] = useState("");
  const [activeTab, setActiveTab] = useState<DashboardTab>("scalars");
  const [theme, setTheme] = useState<ThemeMode>(() => {
    const initial = getInitialTheme();
    applyThemeToDocument(initial);
    return initial;
  });
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  useEffect(() => {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  }, [theme]);

  const mergedRunColors = useMemo(() => {
    const next = { ...runColors };
    runs.forEach((runId, index) => {
      if (!next[runId]) next[runId] = DEFAULT_COLORS[index % DEFAULT_COLORS.length];
    });
    return next;
  }, [runColors, runs]);

  const projectRunIds = useMemo(() => workspace.capabilities ? workspace.records.filter(run => run.parent === projectId && !run.data.archived).map(run => run.data.run_id) : runs, [workspace.capabilities, workspace.records, projectId, runs]);
  const effectiveSelectedRuns = useMemo(() => {
    const viewRunIds = names(viewSettings.selectedRuns);
    const fallback = viewRunIds.length > 0 ? viewRunIds : projectRunIds.length > 0 ? [projectRunIds[0]] : [];
    const source = selectedRuns.length > 0 ? selectedRuns : fallback;
    return source.filter((runId) => projectRunIds.includes(runId) || workspace.records.some(run => run.data.run_id === runId && run.parent === projectId));
  }, [projectRunIds, viewSettings.selectedRuns, selectedRuns, workspace.records, projectId]);
  const {scores, error: scoreError} = useScores(workspace.capabilities ? effectiveSelectedRuns : [], comparisonReducer === "at_step" ? comparisonStep : null, workspace.updated, comparisonReducer === "at_step" ? comparisonSegment : "");

  useEffect(() => {
    if (effectiveSelectedRuns.length === 0) return;
    void loadRunSummaries(effectiveSelectedRuns);
    void loadHistoryForRuns(effectiveSelectedRuns);
  }, [effectiveSelectedRuns, loadHistoryForRuns, loadRunSummaries]);

  useEffect(() => {
    if (activeTab !== "images") return;
    const selectedRun = effectiveSelectedRuns[0];
    if (!selectedRun) return;
    void loadImagesForRun(selectedRun);
  }, [activeTab, effectiveSelectedRuns, loadImagesForRun]);

  useEffect(() => {
    if (activeTab !== "media") return;
    const selectedRun = effectiveSelectedRuns[0];
    if (!selectedRun) return;
    void loadMediaForRun(selectedRun);
  }, [activeTab, effectiveSelectedRuns, loadMediaForRun]);

  const experimentGroups = useMemo(() => buildExperimentGroups(projectRunIds), [projectRunIds]);
  const filteredExperimentGroups = useMemo(() => filterExperimentGroups(experimentGroups, runFilter), [experimentGroups, runFilter]);

  const bounds = useMemo(
    () => getGlobalXBounds(effectiveSelectedRuns, xAxisType),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
    [effectiveSelectedRuns, getGlobalXBounds, xAxisType, tick],
  );

  const runSummaries = useMemo(
    () => getRunSummaries(effectiveSelectedRuns),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
    [effectiveSelectedRuns, getRunSummaries, tick],
  );

  const availableHyperparameters = useMemo(
    () => getHyperparameterKeys(effectiveSelectedRuns),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
    [effectiveSelectedRuns, getHyperparameterKeys, tick],
  );

  const availableMetricSummaries = useMemo(
    () => getMetricSummaryKeys(effectiveSelectedRuns),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
    [effectiveSelectedRuns, getMetricSummaryKeys, tick],
  );

  const imageSeriesOptions = useMemo(() => {
    const selectedRun = effectiveSelectedRuns[0];
    return selectedRun ? getImageSeriesForRun(selectedRun) : [];
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
  }, [effectiveSelectedRuns, getImageSeriesForRun, tick]);

  // The stores live in refs, so memos that read them depend on `tick`, which
  // useAeroboard bumps (at most once per frame) whenever new data arrives.
  const mediaSeries = useMemo(() => {
    const selectedRun = effectiveSelectedRuns[0];
    return selectedRun ? getMediaSeriesForRun(selectedRun) : [];
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
  }, [effectiveSelectedRuns, getMediaSeriesForRun, tick]);

  const hasAnyImages = useMemo(
    () => runs.some((runId) => getImageSeriesForRun(runId).length > 0),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
    [getImageSeriesForRun, runs, tick],
  );

  const selectedRunsHaveImages = useMemo(
    () => effectiveSelectedRuns.some((runId) => getImageSeriesForRun(runId).length > 0),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
    [effectiveSelectedRuns, getImageSeriesForRun, tick],
  );

  const effectiveActiveTab = activeTab;

  const resolvedImageName = useMemo(() => {
    if (imageSeriesOptions.length === 0) return "";
    return imageSeriesOptions.some((series) => series.name === selectedImageName)
      ? selectedImageName
      : imageSeriesOptions[0].name;
  }, [imageSeriesOptions, selectedImageName]);

  const activeImageSeries = useMemo(() => {
    const selectedRun = effectiveSelectedRuns[0];
    if (!selectedRun || !resolvedImageName) return null;
    return getImageSeries(selectedRun, resolvedImageName);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
  }, [effectiveSelectedRuns, getImageSeries, resolvedImageName, tick]);

  const currentMode: "single" | "compare" = effectiveActiveTab === "analysis" ? "compare" : mode;

  const groupedMetrics = useMemo(() => {
    if (effectiveSelectedRuns.length === 0) return {};

    const groups: Record<string, string[]> = {};
    if (currentMode === "single") {
      const run = effectiveSelectedRuns[0];
      getMetricsForRun(run).forEach((metric) => {
        const parts = metric.split("/");
        const base = parts.length > 1 ? parts[1] : parts[0];
        if (base === "step") return;
        if (!groups[base]) groups[base] = [];
        groups[base].push(metric);
      });
    } else {
      effectiveSelectedRuns.forEach((runId) => {
        getMetricsForRun(runId).forEach((metric) => {
          if (metric === "step" || metric.endsWith("/step")) return;
          if (!groups[metric]) groups[metric] = [];
          if (!groups[metric].includes(metric)) groups[metric].push(metric);
        });
      });
    }

    return groups;
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `tick`: the store is a ref
  }, [currentMode, effectiveSelectedRuns, getMetricsForRun, tick]);

  const groupKeys = useMemo(() => Object.keys(groupedMetrics).sort(), [groupedMetrics]);

  // Initial view: aeroboard.toml defaults, overridden by what the selected runs' training
  // scripts configured. `tick` covers per-run settings arriving over the live socket.
  const dashboardDefaults = useMemo(() => {
    return resolveDashboardDefaults(
      serverDashboardDefaults,
      effectiveSelectedRuns.map((runId) => getRunDashboardConfig(runId)),
      groupKeys,
      parseDashboardDefaults(viewSettings.dashboard),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [effectiveSelectedRuns, getRunDashboardConfig, groupKeys, serverDashboardDefaults, tick, viewSettings.dashboard]);

  // UI choices (localStorage) win; otherwise fall back to the configured defaults.
  const effectiveFavoriteCharts = useMemo(() => {
    const result: Record<string, boolean> = {};
    for (const key of groupKeys) result[key] = favoriteCharts[key] ?? isStarredByDefault(dashboardDefaults, key);
    return result;
  }, [dashboardDefaults, favoriteCharts, groupKeys]);

  const effectiveOptimalValues = useMemo(() => {
    const result: Record<string, number | null> = {};
    for (const key of groupKeys) result[key] = key in optimalValues ? optimalValues[key] : names(viewSettings.removedTargets).some(name => matchesChart(name,key)) ? null : dashboardDefaults.targets[key] ?? null;
    return result;
  }, [dashboardDefaults, groupKeys, optimalValues, viewSettings.removedTargets]);

  const orderedMetricKeys = useMemo(() => {
    const keys = new Set(groupKeys);
    const userOrdered = orderedMetrics.filter((metric) => keys.has(metric));
    const rest = groupKeys
      .filter((metric) => !orderedMetrics.includes(metric))
      .sort((a, b) => defaultOrderRank(dashboardDefaults, a) - defaultOrderRank(dashboardDefaults, b) || a.localeCompare(b));
    return [...userOrdered, ...rest];
  }, [dashboardDefaults, groupKeys, orderedMetrics]);

  const visibleMetricKeys = useMemo(() => {
    const favorites = orderedMetricKeys.filter((key) => effectiveFavoriteCharts[key]);
    const nonFavorites = orderedMetricKeys.filter((key) => !effectiveFavoriteCharts[key] && groupedMetrics[key]);
    return [...favorites, ...nonFavorites].filter(key => !hiddenCharts.includes(key) && (!metricSearch || key.toLowerCase().includes(metricSearch.toLowerCase()) || groupedMetrics[key]?.some(name => name.toLowerCase().includes(metricSearch.toLowerCase()))));
  }, [effectiveFavoriteCharts, groupedMetrics, orderedMetricKeys, hiddenCharts, metricSearch]);

  const singleRunRevision =
    effectiveActiveTab === "analysis" ? "compare" : mode === "single" ? (effectiveSelectedRuns[0] ?? "none") : "compare";

  const visibleTabs: { id: DashboardTab; label: string }[] = [
    { id: "scalars", label: "Scalar Values" },
    { id: "images", label: "Images / Plots" },
    { id: "media", label: "Audio / Video" },
    { id: "analysis", label: "Hyperparameter Analysis" },
  ];

  const changeMode = (nextMode: "single" | "compare") => {
    setMode(nextMode);
    setSelectedRuns((prev) => (nextMode === "single" ? prev.slice(0, 1) : prev));
  };

  const toggleRun = (runId: string) => {
    setDeleteError(null);
    setSidebarOpen(false);

    if (currentMode === "single") {
      setSelectedRuns([runId]);
      return;
    }

    setSelectedRuns((prev) => prev.includes(runId) ? prev.filter((value) => value !== runId) : [...prev, runId]);
  };

  const toggleExperimentCollapsed = (experimentName: string) => {
    setCollapsedExperiments((prev) => ({ ...prev, [experimentName]: !prev[experimentName] }));
  };

  const toggleSelection = (value: string, type: "hyper" | "metric") => {
    const setSelected = type === "hyper" ? setSelectedCompareHyperparameters : setSelectedCompareMetrics;
    setSelected((prev) => prev.includes(value) ? prev.filter((entry) => entry !== value) : [...prev, value]);
  };

  const toggleAllHyperparameters = () => {
    setSelectedCompareHyperparameters((prev) => prev.length === availableHyperparameters.length ? [] : availableHyperparameters);
  };

  const toggleAllMetrics = () => {
    setSelectedCompareMetrics((prev) => prev.length === availableMetricSummaries.length ? [] : availableMetricSummaries);
  };

  const handleXAxisTypeChange = (nextType: XAxisType) => {
    setXAxisType(nextType);
    setGlobalXRange(null);
  };

  const handleDeleteAllRuns = () => {
    if (runs.length === 0 || isDeletingRuns) return;
    setDeleteError(null);
    setShowDeleteConfirm(true);
  };

  const handleConfirmDeleteAll = async () => {
    setShowDeleteConfirm(false);
    setIsDeletingRuns(true);
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
    setGlobalXRange(null);
    setDeleteError(null);
  };

  const renderActiveTab = () => {
    if (effectiveSelectedRuns.length === 0) {
      const hasAnyRuns = runs.length > 0;
      return (
        <div className="placeholder-hero">
          <Activity size={56} className="floating-icon" />
          <h2>{hasAnyRuns ? "Select a Run" : "Awaiting Telemetry..."}</h2>
          <p>{hasAnyRuns ? "Choose a run from the sidebar to view its metrics." : "Start a training run to see metrics here."}</p>
          {!hasAnyRuns ? <span className="placeholder-cta">Run a training script to begin</span> : null}
        </div>
      );
    }

    if (effectiveActiveTab === "analysis") {
      return (
        <Suspense fallback={<div className="comparison-card empty-state-card"><h3>Loading analysis view…</h3></div>}>
          <ComparePanels
            runs={runSummaries}
            selectedHyperparameters={selectedCompareHyperparameters}
            selectedMetrics={selectedCompareMetrics}
            runColors={mergedRunColors}
            scores={scores} reducer={comparisonReducer} onReducerChange={value => updateControl("reducer",value,"last")}
            baseline={baseline} onBaselineChange={value => updateControl("baseline",value,"")}
            comparisonStep={comparisonStep} onStepChange={value => updateControl("comparisonStep",value,null)} records={workspace.records}
            comparisonSegment={comparisonSegment} onSegmentChange={value => updateControl("comparisonSegment",value,"")}
          />
        </Suspense>
      );
    }

    if (effectiveActiveTab === "images") {
      if (loadStatus.images === "loading" && !activeImageSeries) {
        return (
          <div className="comparison-card empty-state-card">
            <h3>Images / Plots</h3>
            <p>Loading image history for the selected run…</p>
          </div>
        );
      }

      if (!selectedRunsHaveImages || !activeImageSeries) {
        return (
          <div className="comparison-card empty-state-card">
            <h3>Images / Plots</h3>
            <p>Select a run with logged images to view visual outputs. Image history is loaded on demand.</p>
          </div>
        );
      }

      return (
        <Suspense fallback={<div className="comparison-card empty-state-card"><h3>Loading image view…</h3></div>}>
          <ImagePanel series={activeImageSeries} getImageSource={getImageSource} onPositionSelect={selectMediaPosition} />
        </Suspense>
      );
    }

    if (effectiveActiveTab === "media") {
      if (mediaSeries.length === 0) {
        return (
          <div className="comparison-card empty-state-card">
            <h3>Audio / Video</h3>
            <p>
              {loadStatus.media === "loading"
                ? "Loading media history for the selected run…"
                : "No audio or video logged for this run yet. Use client.log_audio(...) or client.log_video(...)."}
            </p>
          </div>
        );
      }

      return (
        <Suspense fallback={<div className="comparison-card empty-state-card"><h3>Loading media view…</h3></div>}>
          <MediaPanel series={mediaSeries} getMediaSource={getMediaSource} onPositionSelect={selectMediaPosition} />
        </Suspense>
      );
    }

    if (orderedMetricKeys.length === 0) {
      return (
        <div className="placeholder-hero">
          <h2>Ingesting Metrics...</h2>
        </div>
      );
    }

    return (
      <MetricChartGrid
        visibleMetricKeys={visibleMetricKeys}
        groupedMetrics={groupedMetrics}
        currentMode={currentMode}
        effectiveSelectedRuns={effectiveSelectedRuns}
        getChartData={getChartData}
        mergedRunColors={mergedRunColors}
        theme={theme}
        xAxisType={xAxisType}
        smoothing={smoothing}
        removeOutliers={removeOutliers}
        xRange={globalXRange}
        widthMode={widthMode}
        favoriteCharts={effectiveFavoriteCharts}
        collapsedCharts={{...flags(viewSettings.collapsedCharts), ...collapsedCharts}}
        singleRunRevision={singleRunRevision}
        setFavoriteCharts={setFavoriteCharts}
        setCollapsedCharts={setCollapsedCharts}
        setOrderedMetrics={setOrderedMetrics}
        optimalValues={effectiveOptimalValues}
        setOptimalValues={setOptimalValues}
        logCharts={logCharts} onToggleLog={key => updateControl<Record<string,boolean>>("logCharts",previous => ({...previous,[key]:!previous[key]}),{})}
        onHideChart={key => updateControl<string[]>("hiddenCharts",previous => [...previous,key],[])}
        events={[...runEvents, ...(focusedEvent && !runEvents.some(event => event.id === focusedEvent.id) ? [focusedEvent] : [])].filter(event => workspace.records.some(run => run.id === event.data.run_uid && effectiveSelectedRuns.includes(run.data.run_id)))}
        onEventSelect={selectEvent}
      />
    );
  };

  const nextTheme = theme === "dark" ? "light" : "dark";

  const chooseView = (view: SavedView | null) => {
    setViewChoice(view?.id ?? "__none__");
    setSelectedRuns(names(view?.data.settings.selectedRuns));
    const url = new URL(window.location.href); url.searchParams.set("project",projectId); url.searchParams.set("view",view?.id ?? "__none__");
    window.history.replaceState(null,"",url);
  };
  const viewSnapshot: Record<string, unknown> = {
    dashboard: {starred: groupKeys.filter(key => effectiveFavoriteCharts[key]), order: orderedMetricKeys,
      targets: Object.fromEntries(Object.entries(effectiveOptimalValues).filter((entry): entry is [string, number] => entry[1] !== null))},
    removedTargets: Object.keys(effectiveOptimalValues).filter(key => effectiveOptimalValues[key] === null),
    collapsedCharts: {...flags(viewSettings.collapsedCharts), ...collapsedCharts},
    controls: {...controls, mode, widthMode, xAxisType, smoothing, removeOutliers, xRange: globalXRange,
      reducer: comparisonReducer, baseline, comparisonStep, compareMetrics: selectedCompareMetrics, compareHyperparameters: selectedCompareHyperparameters},
    selectedRuns: effectiveSelectedRuns,
  };
  const selectEvent = (event: RunEvent) => {
    setFocusedEvent(event);
    if (event.data.step !== null) {
      setXAxisType("step");setGlobalXRange([event.data.step-10,event.data.step+10]);setActiveTab("scalars");
    }
  };
  const selectMediaPosition = (media: {run_id: string; name: string; ts_ms: number; step?: number | null; asset_id?: string; format: string}) => {
    const run = workspace.records.find(run => run.data.run_id === media.run_id);
    if (!run || media.step == null) return;
    selectEvent({id: `media:${media.asset_id ?? media.ts_ms}`, parent: run.id, revision: 0,
      data: {run_uid: run.id, label: media.name, kind: "media", timestamp_ms: media.ts_ms,
        step: media.step, details: {format: media.format}, media_ids: media.asset_id ? [media.asset_id] : []}});
  };

  return (
    <div className="layout-root">
      <DashboardSidebar
        sidebarOpen={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        theme={theme}
        onThemeToggle={() => {
          applyThemeToDocument(nextTheme);
          setTheme(nextTheme);
        }}
        nextTheme={nextTheme}
        effectiveActiveTab={effectiveActiveTab}
        mode={mode}
        onChangeMode={changeMode}
        runs={runs}
        isDeletingRuns={isDeletingRuns}
        onDeleteAllRuns={handleDeleteAllRuns}
        runFilter={runFilter}
        onRunFilterChange={setRunFilter}
        filteredExperimentGroups={filteredExperimentGroups}
        effectiveSelectedRuns={effectiveSelectedRuns}
        collapsedExperiments={collapsedExperiments}
        onToggleExperimentCollapsed={toggleExperimentCollapsed}
        onToggleRun={toggleRun}
        mergedRunColors={mergedRunColors}
        onRunColorChange={(runId, color) => setRunColors((prev) => ({ ...prev, [runId]: color }))}
        xAxisType={xAxisType}
        onXAxisTypeChange={handleXAxisTypeChange}
        smoothing={smoothing}
        onSmoothingChange={setSmoothing}
        removeOutliers={removeOutliers}
        onRemoveOutliersChange={setRemoveOutliers}
        widthMode={widthMode}
        onWidthModeChange={setWidthMode}
        canResetChartView={hasChartOverrides || Object.keys(controlOverrides).length > 0}
        onResetChartView={() => {resetChartView();setControlOverrides({});}}
        runStatuses={Object.fromEntries(workspace.records.map(run => [run.data.run_id,run.data.display_status]))}
        bounds={bounds}
        globalXRange={globalXRange}
        onGlobalXRangeChange={setGlobalXRange}
        availableHyperparameters={availableHyperparameters}
        selectedCompareHyperparameters={selectedCompareHyperparameters}
        onToggleSelection={toggleSelection}
        onToggleAllHyperparameters={toggleAllHyperparameters}
        availableMetricSummaries={availableMetricSummaries}
        selectedCompareMetrics={selectedCompareMetrics}
        onToggleAllMetrics={toggleAllMetrics}
        hasAnyImages={hasAnyImages}
        imageSeriesOptions={imageSeriesOptions}
        selectedImageName={resolvedImageName}
        onSelectedImageNameChange={setSelectedImageName}
      />

      <div className="main-viewport">
        <DashboardHeader
          connected={connected}
          effectiveActiveTab={effectiveActiveTab}
          effectiveSelectedRuns={effectiveSelectedRuns}
          visibleTabs={visibleTabs}
          activeTab={effectiveActiveTab}
          onTabChange={(tab) => {
            setActiveTab(tab);
            setSidebarOpen(false);
          }}
          sidebarOpen={sidebarOpen}
          onToggleSidebar={() => setSidebarOpen((prev) => !prev)}
          deleteError={deleteError}
          onDismissDeleteError={() => setDeleteError(null)}
        >
          <WorkspacePanel workspace={workspace} projectId={projectId} viewId={activeView?.id ?? ""}
            selectedRuns={effectiveSelectedRuns} metrics={availableMetricSummaries} settings={viewSnapshot}
            onProjectChange={id => {setProjectId(id);setViewChoice("__default__");setSelectedRuns([]);
              const url = new URL(window.location.href);url.searchParams.set("project",id);url.searchParams.delete("view");url.searchParams.delete("runs");window.history.replaceState(null,"",url);}}
            onViewChange={chooseView} onSelectRuns={ids => {setMode(ids.length > 1 ? "compare" : "single");setSelectedRuns(ids);}}
            onEventsChange={setRunEvents} onEventSelect={selectEvent}
            runBrowser={asObject(controls.runBrowser)} onRunBrowserChange={value => updateControl("runBrowser",value,{})}
            onReloadPreferences={() => {resetChartView();setControlOverrides({});}}
            onImportPreferences={() => {
              try {
                setFavoriteCharts(flags(JSON.parse(localStorage.getItem("aeroboard-favorite-charts") ?? "{}")));
                setCollapsedCharts(flags(JSON.parse(localStorage.getItem("aeroboard-collapsed-charts") ?? "{}")));
                setOrderedMetrics(names(JSON.parse(localStorage.getItem("aeroboard-ordered-metrics") ?? "[]")));
                const targets = asObject(JSON.parse(localStorage.getItem("aeroboard-optimal-values") ?? "{}"));
                setOptimalValues(Object.fromEntries(Object.entries(targets).filter((entry): entry is [string, number | null] => entry[1] === null || typeof entry[1] === "number" && Number.isFinite(entry[1]))));
              } catch {workspace.setError("Previous browser preferences could not be imported.");}
            }} />
        </DashboardHeader>

        {dashboardConfigError && !configErrorDismissed ? (
          <div className="status-banner status-banner--error config-error-banner" role="status">
            <p>Dashboard defaults not applied: {dashboardConfigError}</p>
            <button type="button" onClick={() => setConfigErrorDismissed(true)}>
              Dismiss
            </button>
          </div>
        ) : null}

        <section className="dashboard-content">
          {workspace.capabilities && workspace.projects.length > 0 && !workspace.projects.some(project => project.id === projectId) ? <p role="status">Project unavailable. <a href="?project=default">Open the Default project</a>.</p> : null}
          {workspace.capabilities && viewChoice !== "__default__" && viewChoice !== "__none__" && !activeView ? <p role="status">Saved view unavailable. Configured defaults are active.</p> : null}
          {effectiveActiveTab === "scalars" ? <div className="workspace-toolbar chart-filter-toolbar">
            <input type="search" aria-label="Search metrics" placeholder="Find a metric or namespace…" value={metricSearch} onChange={event => updateControl("metricSearch",event.target.value,"")} />
            {hiddenCharts.length ? <button type="button" onClick={() => updateControl("hiddenCharts",[],[])}>Show hidden charts ({hiddenCharts.length})</button> : null}
            <a href={`/api/export/scalars.csv?${new URLSearchParams({run_id: effectiveSelectedRuns.join(",")})}`} download="metrics-raw.csv">Export raw CSV</a>
          </div> : null}
          {scoreError && workspace.capabilities ? <p role="status">Scorecards unavailable: {scoreError}</p> : null}
          {effectiveActiveTab === "scalars" && scores.some(score => score.goal) ? <div className="scorecard-grid">{scores.filter(score => score.goal).slice(0,12).map(score => <div className="comparison-card metric-scorecard" key={`${score.run_id}:${score.name}`}>
            <small>{score.run_id} · {score.name} · {score.goal}</small><strong>{score.best?.toPrecision(5) ?? "—"}{score.unit ? ` ${score.unit}` : ""}</strong>
            <span>Best at step {score.best_step ?? "unknown"} · last {score.last?.toPrecision(5) ?? "non-finite / missing"}</span>
            {score.target !== null ? <span>Target {score.target} · {score.time_to_target_ms === null ? "not reached" : `reached after ${(score.time_to_target_ms/1000).toFixed(1)}s`}</span> : null}
          </div>)}</div> : null}
          {renderActiveTab()}
          {focusedEvent && workspace.records.some(run => run.id === focusedEvent.data.run_uid && effectiveSelectedRuns.includes(run.data.run_id)) ? <div className="comparison-card"><button type="button" onClick={() => setFocusedEvent(null)}>Close event</button><h3>{focusedEvent.data.label}</h3>
            <p>{focusedEvent.data.kind} · {focusedEvent.data.step === null ? new Date(focusedEvent.data.timestamp_ms).toLocaleString() : `Step ${focusedEvent.data.step}`}</p>
            <pre>{JSON.stringify(focusedEvent.data.details,null,2)}</pre>
            {focusedEvent.data.media_ids.map(id => <a key={id} href={`/api/assets/${encodeURIComponent(id)}/content`} target="_blank" rel="noreferrer">Open associated media</a>)}
          </div> : null}
        </section>
      </div>

      <ConfirmDialog
        open={showDeleteConfirm}
        title="Delete All Runs"
        message="Delete all runs and clear persisted history? This cannot be undone."
        confirmLabel="Delete All"
        cancelLabel="Cancel"
        onConfirm={handleConfirmDeleteAll}
        onCancel={() => setShowDeleteConfirm(false)}
        dangerous
      />
    </div>
  );
}

export default function AppWithBoundary() {
  return (
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  );
}
