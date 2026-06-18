export interface TimeSeriesData {
  wallTimes: Float64Array;
  relTimes: Float64Array;
  steps: Float64Array;
  plotX: Float64Array;
  values: Float64Array;
  count: number;
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
  ts_ms: number;
  run_id: string;
  name: string;
  format: string;
  image_base64?: string;
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
};

export type LoadStatus = "idle" | "loading" | "ready" | "error";
