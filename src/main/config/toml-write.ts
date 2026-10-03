/** Serialises JavaScript values as TOML tokens, and small helpers on parsed TOML values. */
import type { ConfigPath } from '@shared/config';

export type PlainObject = Record<string, unknown>;

export function isPlainObject(value: unknown): value is PlainObject {
  if (value === null || typeof value !== 'object') return false;
  if (Array.isArray(value) || value instanceof Date) return false;
  const proto = Object.getPrototypeOf(value) as unknown;
  return proto === Object.prototype || proto === null;
}

/** Structural equality over parsed TOML values. */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (a instanceof Date && b instanceof Date) {
    return a.getTime() === b.getTime() && a.toISOString() === b.toISOString();
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => deepEqual(item, b[i]));
  }
  if (isPlainObject(a) && isPlainObject(b)) {
    const keys = Object.keys(a);
    return (
      keys.length === Object.keys(b).length &&
      keys.every((key) => Object.hasOwn(b, key) && deepEqual(a[key], b[key]))
    );
  }
  return false;
}

/** Copies tables and arrays; scalars (including dates) are shared. */
export function cloneValue<T>(value: T): T {
  if (Array.isArray(value)) return value.map((item) => cloneValue(item)) as T;
  if (isPlainObject(value)) {
    const out: PlainObject = {};
    for (const [key, child] of Object.entries(value)) out[key] = cloneValue(child);
    return out as T;
  }
  return value;
}

/**
 * Drops what TOML cannot hold: `undefined` and `null` members of tables and
 * arrays. A top-level `null` becomes `undefined`, which means "remove".
 */
export function normaliseValue(value: unknown): unknown {
  if (value === null || value === undefined) return undefined;
  if (Array.isArray(value)) {
    return value.map(normaliseValue).filter((item) => item !== undefined);
  }
  if (isPlainObject(value)) {
    const out: PlainObject = {};
    for (const [key, child] of Object.entries(value)) {
      const normalised = normaliseValue(child);
      if (normalised !== undefined) out[key] = normalised;
    }
    return out;
  }
  return value;
}

export function getAt(root: unknown, path: ConfigPath): unknown {
  let node = root;
  for (const part of path) {
    if (Array.isArray(node)) {
      if (typeof part !== 'number') return undefined;
      node = node[part];
    } else if (isPlainObject(node)) {
      if (!Object.hasOwn(node, part)) return undefined;
      node = node[part];
    } else {
      return undefined;
    }
  }
  return node;
}

/** Sets `value` at `path`, replacing anything in the way with tables. Returns the new root. */
export function setAt(root: unknown, path: ConfigPath, value: unknown): unknown {
  if (path.length === 0) return value;
  const [head, ...rest] = path as [string | number, ...ConfigPath];
  if (typeof head === 'number') {
    const list = Array.isArray(root) ? root : [];
    list[head] = setAt(list[head], rest, value);
    return list;
  }
  const table: PlainObject = isPlainObject(root) ? root : {};
  table[head] = setAt(Object.hasOwn(table, head) ? table[head] : undefined, rest, value);
  return table;
}

/** Removes the key or list item at `path`, if present. */
export function removeAt(root: unknown, path: ConfigPath): void {
  if (path.length === 0) return;
  const parent = getAt(root, path.slice(0, -1));
  const last = path[path.length - 1]!;
  if (Array.isArray(parent) && typeof last === 'number') {
    if (last < parent.length) parent.splice(last, 1);
  } else if (isPlainObject(parent)) {
    delete parent[last];
  }
}

const BARE_KEY = /^[A-Za-z0-9_-]+$/;

/** A single key segment, quoted only when it has to be. */
export function formatKey(key: string): string {
  return BARE_KEY.test(key) ? key : basicString(key);
}

/** A dotted key: `['cleanup', 'llm_enabled']` → `cleanup.llm_enabled`. */
export function formatKeyPath(keys: string[]): string {
  return keys.map(formatKey).join('.');
}

const SHORT_ESCAPES: Record<string, string> = {
  '\b': '\\b',
  '\t': '\\t',
  '\n': '\\n',
  '\f': '\\f',
  '\r': '\\r',
  '"': '\\"',
  '\\': '\\\\',
};

function escapeChar(char: string): string {
  return SHORT_ESCAPES[char] ?? `\\u${char.charCodeAt(0).toString(16).padStart(4, '0')}`;
}

// Control characters, quotes and backslashes cannot appear raw in a basic string.
const NEEDS_ESCAPE = /[\x00-\x1f\x7f"\\]/g;

/** A single-line TOML basic string. */
export function basicString(text: string): string {
  return `"${text.replace(NEEDS_ESCAPE, escapeChar)}"`;
}

/**
 * A multi-line TOML basic string, with real line breaks so long prompt and
 * snippet text stays readable in an editor.
 */
function multilineString(text: string): string {
  let out = '';
  let lastWasRawQuote = false;
  for (let i = 0; i < text.length; i++) {
    const char = text[i]!;
    let piece: string;
    if (char === '\n') {
      piece = '\n';
    } else if (char === '"') {
      // Never leave two raw quotes together or one at the end: either could
      // run into the closing delimiter.
      piece = lastWasRawQuote || i === text.length - 1 ? '\\"' : '"';
    } else {
      piece = char.replace(NEEDS_ESCAPE, escapeChar);
    }
    lastWasRawQuote = piece === '"';
    out += piece;
  }
  // The newline right after the opening delimiter is not part of the value.
  return '"""\n' + out + '"""';
}

function numberToken(value: number): string {
  if (Number.isNaN(value)) return 'nan';
  if (value === Infinity) return 'inf';
  if (value === -Infinity) return '-inf';
  if (Number.isSafeInteger(value)) return String(value);
  const text = String(value);
  // A float needs a fraction or an exponent to be read back as one.
  return /[.e]/i.test(text) ? text : `${text}.0`;
}

export interface WriteOptions {
  /**
   * Allow `"""` strings for text containing line breaks. Only safe for a
   * `key = value` line in a document with `\n` line endings: in a CRLF
   * document the line breaks would be read back as `\r\n`.
   */
  multiline?: boolean;
}

/** Serialises a value as a TOML token; tables become inline tables. */
export function formatValue(value: unknown, options: WriteOptions = {}): string {
  switch (typeof value) {
    case 'string':
      return options.multiline && value.includes('\n')
        ? multilineString(value)
        : basicString(value);
    case 'number':
      return numberToken(value);
    case 'bigint':
      return String(value);
    case 'boolean':
      return value ? 'true' : 'false';
    case 'object':
      break;
    default:
      throw new TypeError(`Cannot write a ${typeof value} to TOML`);
  }
  // smol-toml's TomlDate overrides toISOString to keep local dates and times as written.
  if (value instanceof Date) return value.toISOString();
  if (Array.isArray(value)) return `[${value.map((item) => formatValue(item)).join(', ')}]`;
  if (isPlainObject(value)) {
    const entries = Object.entries(value).filter(([, child]) => child !== undefined);
    if (entries.length === 0) return '{}';
    const body = entries.map(([key, child]) => `${formatKey(key)} = ${formatValue(child)}`);
    return `{ ${body.join(', ')} }`;
  }
  throw new TypeError('Cannot write this value to TOML');
}

/**
 * The `key = value` lines of one table body. Nested tables are flattened to
 * dotted keys (`cleanup.llm_enabled = true`), as in the spec's profile example.
 */
export function formatTableLines(
  table: PlainObject,
  options: WriteOptions,
  prefix: string[] = [],
): string[] {
  const lines: string[] = [];
  for (const [key, value] of Object.entries(table)) {
    if (value === undefined) continue;
    const keys = [...prefix, key];
    if (isPlainObject(value) && Object.keys(value).length > 0) {
      lines.push(...formatTableLines(value, options, keys));
    } else {
      lines.push(`${formatKeyPath(keys)} = ${formatValue(value, options)}`);
    }
  }
  return lines;
}
