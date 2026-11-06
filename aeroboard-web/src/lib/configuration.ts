type ConfigValue = string | number | boolean | null | ConfigValue[] | { [key: string]: ConfigValue };

/** Read JSON and the literal dictionaries emitted by Python loggers, without evaluating code. */
export function parseConfiguration(value: string): unknown {
  try {return JSON.parse(value);} catch { /* Older loggers use Python repr. */ }
  let position = 0;
  const whitespace = () => {while (/\s/.test(value[position] ?? "") && position < value.length) position++;};
  const take = (character: string) => {
    whitespace();
    if (value[position] !== character) throw new Error("Unexpected literal");
    position++;
  };
  const read = (depth: number): ConfigValue => {
    if (depth > 50) throw new Error("Nested literal");
    whitespace();
    const character = value[position];
    if (character === "'" || character === '"') {
      position++;
      let text = "";
      while (position < value.length) {
        const next = value[position++];
        if (next === character) return text;
        if (next !== "\\") {text += next;continue;}
        const escaped = value[position++];
        const escapes: Record<string, string> = { n: "\n", r: "\r", t: "\t", b: "\b", f: "\f", "\\": "\\", "'": "'", '"': '"' };
        if (escaped in escapes) text += escapes[escaped];
        else if (escaped === "u" || escaped === "x") {
          const length = escaped === "u" ? 4 : 2;
          const digits = value.slice(position, position + length);
          if (!new RegExp(`^[0-9a-fA-F]{${length}}$`).test(digits)) throw new Error("Invalid escape");
          text += String.fromCharCode(parseInt(digits, 16));position += length;
        } else throw new Error("Unsupported escape");
      }
      throw new Error("Unclosed string");
    }
    if (character === "{") {
      take("{");
      const entries: [string, ConfigValue][] = [];
      whitespace();
      while (value[position] !== "}") {
        const key = read(depth + 1);
        if (typeof key !== "string") throw new Error("Non-string key");
        take(":");entries.push([key, read(depth + 1)]);whitespace();
        if (value[position] !== ",") break;
        take(",");whitespace();
      }
      take("}");return Object.fromEntries(entries);
    }
    if (character === "[" || character === "(") {
      const end = character === "[" ? "]" : ")";
      take(character);
      const entries: ConfigValue[] = [];
      whitespace();
      while (value[position] !== end) {
        entries.push(read(depth + 1));whitespace();
        if (value[position] !== ",") break;
        take(",");whitespace();
      }
      take(end);return entries;
    }
    for (const [literal, result] of [["True", true], ["False", false], ["None", null]] as const) {
      if (value.startsWith(literal, position)) {position += literal.length;return result;}
    }
    const numeric = value.slice(position).match(/^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/);
    if (numeric && Number.isFinite(Number(numeric[0]))) {position += numeric[0].length;return Number(numeric[0]);}
    throw new Error("Not a literal");
  };
  try {
    const parsed = read(0);
    whitespace();
    return position === value.length ? parsed : value;
  } catch {return value;}
}

export function formatConfiguration(value: unknown): string {
  if (value === undefined) return "Missing";
  if (typeof value === "string") return value === "" ? '""' : value;
  return JSON.stringify(value, null, 2) ?? String(value);
}

export interface ConfigurationChange { key: string; before: unknown; after: unknown }

const isRecord = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const ownValue = (record: Record<string, unknown>, key: string): unknown => Object.hasOwn(record, key) ? record[key] : undefined;
const stable = (value: unknown): unknown => Array.isArray(value) ? value.map(stable) : isRecord(value)
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, stable(value[key])])) : value;

/** Compare individual fields rather than repeating entire nested configurations. */
export function configurationChanges(before: Record<string, unknown>, after: Record<string, unknown>): ConfigurationChange[] {
  const changes: ConfigurationChange[] = [];
  const compare = (key: string, previous: unknown, next: unknown) => {
    if ((isRecord(previous) && (isRecord(next) || next === undefined)) || (isRecord(next) && previous === undefined)) {
      const left = isRecord(previous) ? previous : {};
      const right = isRecord(next) ? next : {};
      const keys = [...new Set([...Object.keys(left), ...Object.keys(right)])].sort();
      if (keys.length) {keys.forEach(child => compare(`${key}.${child}`, ownValue(left, child), ownValue(right, child)));return;}
    }
    if (JSON.stringify(stable(previous)) !== JSON.stringify(stable(next))) changes.push({ key, before: previous, after: next });
  };
  [...new Set([...Object.keys(before), ...Object.keys(after)])].sort().forEach(key => compare(key, ownValue(before, key), ownValue(after, key)));
  return changes;
}
