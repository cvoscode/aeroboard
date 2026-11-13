import { useCallback,useLayoutEffect,useMemo,useRef,type ReactNode } from "react";
import { ChartContext,type ChartSettings } from "./chartSettingsContext";
import { chartBaseName } from "../lib/dashboardDefaults";
/** Keep keyed actions stable while always invoking the latest settings callbacks. */
function useAction<Args extends unknown[]>(callback: (...args: Args) => void) {
  const current = useRef(callback);
  useLayoutEffect(() => { current.current = callback; }, [callback]);
  return useCallback((...args: Args) => current.current(...args), []);
}

function useActions(settings: ChartSettings) {
  const scale = useAction(settings.onScaleChange);
  const trend = useAction(settings.onToggleTrend);
  const hide = useAction(settings.onHide);
  const reorder = useAction(settings.onReorder);
  const favorite = useAction((key: string) => {
    const enabled = !settings.favorites[key];
    settings.setFavorites(previous => ({ ...previous, [chartBaseName(key)]: enabled }));
    if (enabled) settings.onPin(key);
  });
  const collapse = useAction((key: string) => {
    const collapsed = settings.collapsed[chartBaseName(key)] ?? !settings.favorites[key];
    settings.setCollapsed(previous => ({ ...previous, [chartBaseName(key)]: !collapsed }));
  });
  const target = useAction((key: string, value: number | null) => {
    settings.setOptimalValues(previous => ({ ...previous, [chartBaseName(key)]: value }));
  });
  const events=useMemo(()=>new Map(settings.events.map(event=>[event.id,event])),[settings.events]);
  const event = useAction((id: string) => {
    const selected = events.get(id);
    if (selected) settings.onEventSelect(selected);
  });
  return useMemo(() => ({ scale, trend, hide, reorder, favorite, collapse, target, event }),
    [scale, trend, hide, reorder, favorite, collapse, target, event]);
}


export function ChartSettingsProvider({settings,children}:{settings:ChartSettings;children:ReactNode}) {
  const actions=useActions(settings);
  const markers=useMemo(()=>{
    const origins=settings.xAxisType==="rel"?Array.from(new Set(settings.seriesOrigins)):[0];
    return new Map(origins.map(origin=>[origin,settings.events.flatMap(event=>{
      const x=settings.xAxisType==="step"?event.data.step:event.data.timestamp_ms/1000-origin;
      return x===null?[]:[{id:event.id,x,label:event.data.label}];
    })]));
  },[settings.seriesOrigins,settings.events,settings.xAxisType]);
  const value=useMemo(()=>({settings,actions,markers}),[settings,actions,markers]);
  return <ChartContext.Provider value={value}>{children}</ChartContext.Provider>;
}
