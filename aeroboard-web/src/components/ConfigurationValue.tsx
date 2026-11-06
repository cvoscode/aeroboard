import { formatConfiguration } from "../lib/configuration";

/** Large configurations expand only when requested, keeping comparison rows compact. */
export function ConfigurationValue({ value, label }: { value: unknown; label: string }) {
  const text = formatConfiguration(value);
  if (text.length <= 70 && !text.includes("\n")) return <span className="configuration-short-value" title={text}>{text}</span>;
  return <details className="configuration-value">
    <summary aria-label={`Inspect ${label}`} title={text}><span>{text.replace(/\s+/g, " ")}</span></summary>
    <pre>{text}</pre>
  </details>;
}
