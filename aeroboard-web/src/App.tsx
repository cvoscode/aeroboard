import React, { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { Activity } from "lucide-react";

import "./App.css";
import { DashboardHeader } from "./components/DashboardHeader";
import { DashboardSidebar } from "./components/DashboardSidebar";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { MetricChartGrid } from "./components/MetricChartGrid";
import { useAeroboard } from "./hooks/useAeroboard";
import { useDashboardPreferences } from "./hooks/useDashboardPreferences";
import { buildExperimentGroups, filterExperimentGroups } from "./lib/runGroups";
import type { XAxisType } from "./components/LineChart";

const ComparePanels = lazy(() => import("./components/ComparePanels"));
const ImagePanel = lazy(() => import("./components/ImagePanel"));

type DashboardTab = "scalars" | "images" | "analysis";
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

function App() {
  const {
    connected,
    getChartData,
    loadStatus,
    loadRunSummaries,
    loadHistoryForRuns,
    loadImagesForRun,
    ensureImageFrame,
    getMetricsForRun,
    getGlobalXBounds,
    getRunSummaries,
    getHyperparameterKeys,
    getMetricSummaryKeys,
    getImageSeriesForRun,
    getImageSeries,
    runs,
    deleteAllRuns,
  } = useAeroboard();

  const {
    widthMode,
    setWidthMode,
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
  } = useDashboardPreferences();

  const [mode, setMode] = useState<"single" | "compare">("single");
  const [selectedRuns, setSelectedRuns] = useState<string[]>([]);
  const [xAxisType, setXAxisType] = useState<XAxisType>("step");
  const [smoothing, setSmoothing] = useState(0);
  const [removeOutliers, setRemoveOutliers] = useState(false);
  const [globalXRange, setGlobalXRange] = useState<[number, number] | null>(null);
  const [isDeletingRuns, setIsDeletingRuns] = useState(false);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [selectedCompareHyperparameters, setSelectedCompareHyperparameters] = useState<string[]>([]);
  const [selectedCompareMetrics, setSelectedCompareMetrics] = useState<string[]>([]);
  const [selectedImageName, setSelectedImageName] = useState("");
  const [runFilter, setRunFilter] = useState("");
  const [activeTab, setActiveTab] = useState<DashboardTab>("scalars");
  const [theme, setTheme] = useState<ThemeMode>(() => getInitialTheme());
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  }, [theme]);

  const mergedRunColors = useMemo(() => {
    const next = { ...runColors };
    runs.forEach((runId, index) => {
      if (!next[runId]) next[runId] = DEFAULT_COLORS[index % DEFAULT_COLORS.length];
    });
    return next;
  }, [runColors, runs]);

  const selectedRunFallback = useMemo(() => (selectedRuns.length === 0 && runs.length > 0 ? [runs[0]] : []), [runs, selectedRuns.length]);

  const effectiveSelectedRuns = useMemo(() => {
    const source = selectedRuns.length > 0 ? selectedRuns : selectedRunFallback;
    return source.filter((runId) => runs.includes(runId));
  }, [runs, selectedRunFallback, selectedRuns]);

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

  const experimentGroups = useMemo(() => buildExperimentGroups(runs), [runs]);
  const filteredExperimentGroups = useMemo(() => filterExperimentGroups(experimentGroups, runFilter), [experimentGroups, runFilter]);

  const bounds = useMemo(
    () => getGlobalXBounds(effectiveSelectedRuns, xAxisType),
    [effectiveSelectedRuns, getGlobalXBounds, xAxisType],
  );

  const runSummaries = useMemo(
    () => getRunSummaries(effectiveSelectedRuns),
    [effectiveSelectedRuns, getRunSummaries],
  );

  const availableHyperparameters = useMemo(
    () => getHyperparameterKeys(effectiveSelectedRuns),
    [effectiveSelectedRuns, getHyperparameterKeys],
  );

  const availableMetricSummaries = useMemo(
    () => getMetricSummaryKeys(effectiveSelectedRuns),
    [effectiveSelectedRuns, getMetricSummaryKeys],
  );

  const imageSeriesOptions = useMemo(() => {
    const selectedRun = effectiveSelectedRuns[0];
    return selectedRun ? getImageSeriesForRun(selectedRun) : [];
  }, [effectiveSelectedRuns, getImageSeriesForRun]);

  const hasAnyImages = useMemo(
    () => runs.some((runId) => getImageSeriesForRun(runId).length > 0),
    [getImageSeriesForRun, runs],
  );

  const selectedRunsHaveImages = useMemo(
    () => effectiveSelectedRuns.some((runId) => getImageSeriesForRun(runId).length > 0),
    [effectiveSelectedRuns, getImageSeriesForRun],
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
  }, [effectiveSelectedRuns, getImageSeries, resolvedImageName]);

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
  }, [currentMode, effectiveSelectedRuns, getMetricsForRun]);

  const groupKeys = useMemo(() => Object.keys(groupedMetrics).sort(), [groupedMetrics]);
  const orderedMetricKeys = useMemo(() => {
    const keys = new Set(groupKeys);
    return [
      ...orderedMetrics.filter((metric) => keys.has(metric)),
      ...groupKeys.filter((metric) => !orderedMetrics.includes(metric)),
    ];
  }, [groupKeys, orderedMetrics]);

  const visibleMetricKeys = useMemo(() => {
    const favorites = orderedMetricKeys.filter((key) => favoriteCharts[key]);
    const nonFavorites = orderedMetricKeys.filter((key) => !favoriteCharts[key] && groupedMetrics[key]);
    return [...favorites, ...nonFavorites];
  }, [favoriteCharts, groupedMetrics, orderedMetricKeys]);

  const singleRunRevision =
    effectiveActiveTab === "analysis" ? "compare" : mode === "single" ? (effectiveSelectedRuns[0] ?? "none") : "compare";

  const visibleTabs: { id: DashboardTab; label: string }[] = [
    { id: "scalars", label: "Scalar Values" },
    { id: "images", label: "Images / Plots" },
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
          <ImagePanel series={activeImageSeries} onRequestFrame={ensureImageFrame} />
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
        favoriteCharts={favoriteCharts}
        collapsedCharts={collapsedCharts}
        singleRunRevision={singleRunRevision}
        setFavoriteCharts={setFavoriteCharts}
        setCollapsedCharts={setCollapsedCharts}
        setOrderedMetrics={setOrderedMetrics}
      />
    );
  };

  const nextTheme = theme === "dark" ? "light" : "dark";

  return (
    <div className="layout-root">
      <DashboardSidebar
        sidebarOpen={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        theme={theme}
        onThemeToggle={() => setTheme(nextTheme)}
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
        />

        <section className="dashboard-content">
          {renderActiveTab()}
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
