import { useCallback, useEffect, useState, type SetStateAction } from "react";

export function usePersistentState<T>(
  key: string,
  initialValue: T,
  parse?: (raw: unknown) => T,
) {
  const read = () => {
    if (typeof window === "undefined") return initialValue;

    const stored = window.localStorage.getItem(key);
    if (stored == null) return initialValue;

    try {
      const parsed = JSON.parse(stored);
      return parse ? parse(parsed) : (parsed as T);
    } catch {
      return initialValue;
    }
  };
  const [entry, setEntry] = useState<{key: string; value: T}>(() => ({key, value: read()}));
  let current = entry;
  if (entry.key !== key) {
    current = {key, value: read()};
    setEntry(current);
  }
  const setValue = useCallback((update: SetStateAction<T>) => {
    setEntry(previous => ({key: previous.key, value: typeof update === "function" ? (update as (value: T) => T)(previous.value) : update}));
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") return;
    if (entry.key === key) window.localStorage.setItem(key, JSON.stringify(entry.value));
  }, [key, entry]);

  return [current.value, setValue] as const;
}
