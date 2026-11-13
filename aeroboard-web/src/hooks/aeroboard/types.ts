export interface TimeSeriesData {
  wallTimes: Float64Array;
  relTimes: Float64Array;
  steps: Float64Array;
  plotX: Float64Array;
  values: Float64Array;
  count: number;
  startTime?: number;
  loadWindow?: (startMs:number,endMs:number)=>Promise<void>;
  minStep?: number;
  maxStep?: number;
}

export interface HyperparameterEntry {
  key: string;
  value: string;
  value_type: string;
}

export interface MetricSummary {
  name: string;
  last: number;
  min: number;
  max: number;
}

export interface RunSummary {
  run_id: string;
  hyperparameters: HyperparameterEntry[];
  metric_summaries: MetricSummary[];
}

export interface ImageFrame {
  asset_id?: string;
  step?: number | null;
  ts_ms: number;
  run_id: string;
  name: string;
  format: string;
  /** Blob URL for frames pushed live with their bytes; persisted frames load by URL. */
  objectUrl?: string;
}

export interface ImageSeries {
  run_id: string;
  name: string;
  frames: ImageFrame[];
}

export type ScalarRow = {
  ts_ms: number;
  run_id: string;
  name: string;
  value: number;
  /** Training step sent with the value (null/absent for data from older clients). */
  step?: number | null;
};

export type LoadStatus = "idle" | "loading" | "ready" | "error";

export type MediaKind = "audio" | "video";

export interface MediaClip {
  asset_id?: string;
  step?: number | null;
  kind: MediaKind;
  ts_ms: number;
  run_id: string;
  name: string;
  format: string;
  /** Blob URL for clips pushed with their bytes (live-only mode). */
  objectUrl?: string;
}

export interface MediaSeries {
  kind: MediaKind;
  run_id: string;
  name: string;
  clips: MediaClip[];
}
