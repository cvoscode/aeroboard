import { parseChartScales } from './chartScale';
import { parseMetricGroups } from './metricGroups';
export function asObject(raw:unknown):Record<string,unknown>{return raw&&typeof raw==='object'&&!Array.isArray(raw)?raw as Record<string,unknown>:{};}
export function names(raw:unknown):string[]{return Array.isArray(raw)?raw.filter((v):v is string=>typeof v==='string'):[];}
export function flags(raw:unknown):Record<string,boolean>{return Object.fromEntries(Object.entries(asObject(raw)).filter((entry):entry is [string,boolean]=>typeof entry[1]==='boolean'));}
export function parseControls(raw:unknown) {
  const data=asObject(raw),range=data.xRange;
  return {...data,
    mode:data.mode==='compare'?'compare' as const:'single' as const,
    xAxisType:(data.xAxisType==='wall'||data.xAxisType==='rel'?data.xAxisType:'step') as 'wall'|'rel'|'step',
    smoothing:typeof data.smoothing==='number'&&Number.isFinite(data.smoothing)?Math.min(1,Math.max(0,data.smoothing)):0,
    removeOutliers:data.removeOutliers===true,
    widthMode:(data.widthMode==='compact'||data.widthMode==='full'?data.widthMode:'wide') as 'compact'|'full'|'wide',
    xRange:Array.isArray(range)&&range.length===2&&range.every(v=>typeof v==='number'&&Number.isFinite(v))&&range[0]<range[1]?range as [number,number]:null,
    metricSearch:typeof data.metricSearch==='string'?data.metricSearch:'',
    reducer:(data.reducer==='best'||data.reducer==='at_step'?data.reducer:'last') as 'best'|'at_step'|'last',
    baseline:typeof data.baseline==='string'?data.baseline:'',
    comparisonStep:typeof data.comparisonStep==='number'&&Number.isFinite(data.comparisonStep)?data.comparisonStep:null,
    comparisonSegment:typeof data.comparisonSegment==='string'?data.comparisonSegment:'',
    logCharts:flags(data.logCharts),trendCharts:flags(data.trendCharts),chartScales:parseChartScales(data.chartScales),
    hiddenCharts:names(data.hiddenCharts),compareHyperparameters:names(data.compareHyperparameters),compareMetrics:names(data.compareMetrics),
    metricGroups:parseMetricGroups(data.metricGroups),runBrowser:asObject(data.runBrowser),objectivesOpen:data.objectivesOpen===true,
  };
}
export type Controls=ReturnType<typeof parseControls>;
