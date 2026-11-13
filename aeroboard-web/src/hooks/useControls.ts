import { useMemo, type SetStateAction } from 'react';
import { usePersistentState } from './usePersistentState';
import { asObject,parseControls } from '../lib/controls';
export function useControls(scope:string,viewControls:Record<string,unknown>) {
  const [overrides,setOverrides]=usePersistentState<Record<string,unknown>>(`aeroboard-controls:${scope}`,{},asObject);
  const controls=useMemo(()=>parseControls({...viewControls,...overrides}),[viewControls,overrides]);
  const updateControl=<T,>(key:string,update:SetStateAction<T>,fallback:T)=>{
    setOverrides(previous=>{
      const value=(key in previous?previous[key]:viewControls[key]??fallback) as T;
      return {...previous,[key]:typeof update==='function'?(update as (previous:T)=>T)(value):update};
    });
  };
  return {controls,controlOverrides:overrides,setControlOverrides:setOverrides,updateControl};
}
