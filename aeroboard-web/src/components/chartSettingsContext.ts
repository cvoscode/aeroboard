import { createContext, useContext, type Dispatch, type SetStateAction } from "react";
import type { TimeSeriesData } from "../hooks/useAeroboard";
import type { RunEvent } from "../hooks/useWorkspace";
import type { WidthMode } from "../hooks/useDashboardPreferences";
import type { YAxisScale } from "../lib/chartScale";
import type { ChartMarker, XAxisType } from "./LineChart";
type BooleanSettings = Record<string, boolean>;
export interface ChartSettings {
  chartScales: Record<string, YAxisScale>;
  trendCharts: BooleanSettings;
  events: RunEvent[];
  seriesOrigins: number[];
  currentMode: "single" | "compare";
  selectedRuns: string[];
  getChartData: (runId: string, name: string) => TimeSeriesData | null;
  runColors: Record<string, string>;
  theme: "light" | "dark";
  xAxisType: XAxisType;
  smoothing: number;
  removeOutliers: boolean;
  xRange: [number, number] | null;
  widthMode: WidthMode;
  favorites: BooleanSettings;
  collapsed: BooleanSettings;
  optimalValues: Record<string, number | null>;
  setFavorites: Dispatch<SetStateAction<BooleanSettings>>;
  setCollapsed: Dispatch<SetStateAction<BooleanSettings>>;
  setOptimalValues: Dispatch<SetStateAction<Record<string, number | null>>>;
  onScaleChange: (key: string, scale: YAxisScale) => void;
  onToggleTrend: (key: string) => void;
  onHide: (key: string) => void;
  onPin: (key: string) => void;
  onReorder: (active: string, over: string) => void;
  onEventSelect: (event: RunEvent) => void;
}


export interface ChartActions {
  scale:(key:string,scale:YAxisScale)=>void;
  trend:(key:string)=>void;
  hide:(key:string)=>void;
  reorder:(active:string,over:string)=>void;
  favorite:(key:string)=>void;
  collapse:(key:string)=>void;
  target:(key:string,value:number|null)=>void;
  event:(id:string)=>void;
}
export const ChartContext=createContext<{settings:ChartSettings;actions:ChartActions;markers:Map<number,ChartMarker[]>}|null>(null);
export function useChartSettings() {
  const value=useContext(ChartContext);
  if(!value)throw new Error("ChartSettingsProvider is required");
  return value;
}
