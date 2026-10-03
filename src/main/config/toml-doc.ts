/**
 * A position-preserving scan of a TOML document. `smol-toml` gives us the
 * values; this gives us where each of them sits in the source text, which is
 * what in-place patching and "which line is wrong" both need.
 *
 * The scanner assumes the text already parses (callers run `smol-toml` first),
 * so it is tolerant rather than strict.
 */
import { parse } from 'smol-toml';
import type { ConfigPath } from '@shared/config';

export interface ValueNode {
  kind: 'string' | 'bare' | 'array' | 'inline';
  /** Offset of the first character of the value token. */
  start: number;
  /** Offset just past the last character of the value token. */
  end: number;
  /** Elements, for `array`. */
  items?: ValueNode[];
  /** Key/value pairs, for `inline` tables. */
  entries?: KeyValueNode[];
}

export interface KeyValueNode {
  /** Decoded key segments: `insert.paste_shortcut` → `['insert', 'paste_shortcut']`. */
  key: string[];
  keyStart: number;
  value: ValueNode;
}

export interface SectionNode {
  /** Absolute path, with list indexes: the second `[[profiles]]` is `['profiles', 1]`. */
  path: ConfigPath;
  /** `null` for the root section (everything before the first header). */
  header: { keys: string[]; array: boolean } | null;
  /** Offset of the start of the header line; 0 for the root section. */
  start: number;
  /** Offset just past the header line; 0 for the root section. */
  bodyStart: number;
  /** Offset of the start of the next header line, or the end of the text. */
  end: number;
  entries: KeyValueNode[];
}

/** Where a value lives, which decides how it is replaced or removed. */
export type Owner =
  | { type: 'section'; section: SectionNode; entry: KeyValueNode }
  | { type: 'inline'; table: ValueNode; entry: KeyValueNode }
  | { type: 'array'; array: ValueNode; index: number };

export interface Located {
  path: ConfigPath;
  node: ValueNode;
  owner: Owner;
}

export interface TomlDoc {
  source: string;
  /** Line ending used by the document: `\r\n` if any is present, else `\n`. */
  eol: string;
  /** The root section first, then one per `[header]` in file order. */
  sections: SectionNode[];
  /** Every value in the document with its absolute path, in file order. */
  values: Located[];
}

const BARE_VALUE_END = new Set([',', ']', '}', '#', '\n', '\r']);

class Scanner {
  pos = 0;
  constructor(private readonly s: string) {}

  scan(): SectionNode[] {
    const s = this.s;
    const root: SectionNode = {
      path: [],
      header: null,
      start: 0,
      bodyStart: 0,
      end: 0,
      entries: [],
    };
    const sections = [root];
    // Number of items seen so far for each array of tables, keyed by its path.
    const counts = new Map<string, number>();
    let current = root;

    while (this.pos < s.length) {
      this.skipBlank();
      const c = s[this.pos];
      if (c === undefined) break;
      if (c === '\n' || c === '\r') {
        this.pos++;
      } else if (c === '#') {
        this.pos = this.lineEnd(this.pos);
      } else if (c === '[') {
        const lineStart = s.lastIndexOf('\n', this.pos - 1) + 1;
        const array = s[this.pos + 1] === '[';
        this.pos += array ? 2 : 1;
        const keys = this.key();
        const lineEnd = this.lineEnd(this.pos);
        this.pos = lineEnd < s.length ? lineEnd + 1 : lineEnd;
        current.end = lineStart;
        current = {
          path: resolveHeader(keys, array, counts),
          header: { keys, array },
          start: lineStart,
          bodyStart: this.pos,
          end: s.length,
          entries: [],
        };
        sections.push(current);
      } else {
        const entry = this.keyValue();
        if (entry) current.entries.push(entry);
        else this.pos = this.lineEnd(this.pos);
      }
    }
    current.end = s.length;
    return sections;
  }

  private skipBlank(): void {
    while (this.s[this.pos] === ' ' || this.s[this.pos] === '\t') this.pos++;
  }

  /** Skips whitespace, newlines and comments (inside arrays and inline tables). */
  private skipTrivia(): void {
    const s = this.s;
    for (;;) {
      const c = s[this.pos];
      if (c === ' ' || c === '\t' || c === '\n' || c === '\r') this.pos++;
      else if (c === '#') this.pos = this.lineEnd(this.pos);
      else return;
    }
  }

  /** Offset of the newline that ends the line containing `from`, or the text length. */
  private lineEnd(from: number): number {
    const i = this.s.indexOf('\n', from);
    return i === -1 ? this.s.length : i;
  }

  private keyValue(): KeyValueNode | null {
    const keyStart = this.pos;
    const key = this.key();
    this.skipBlank();
    if (key.length === 0 || this.s[this.pos] !== '=') return null;
    this.pos++;
    this.skipBlank();
    return { key, keyStart, value: this.value() };
  }

  /** Reads a possibly dotted, possibly quoted key and leaves `pos` after it. */
  private key(): string[] {
    const s = this.s;
    const parts: string[] = [];
    for (;;) {
      this.skipBlank();
      const c = s[this.pos];
      const start = this.pos;
      if (c === '"' || c === "'") {
        this.string();
        parts.push(decodeString(s.slice(start, this.pos)));
      } else {
        while (this.pos < s.length && !' \t.=]\r\n#'.includes(s[this.pos]!)) this.pos++;
        if (this.pos === start) break;
        parts.push(s.slice(start, this.pos));
      }
      this.skipBlank();
      if (s[this.pos] !== '.') break;
      this.pos++;
    }
    return parts;
  }

  private value(): ValueNode {
    const s = this.s;
    const start = this.pos;
    const c = s[this.pos];
    if (c === '"' || c === "'") {
      this.string();
      return { kind: 'string', start, end: this.pos };
    }
    if (c === '[') {
      this.pos++;
      const items: ValueNode[] = [];
      for (;;) {
        this.skipTrivia();
        if (this.pos >= s.length) break;
        if (s[this.pos] === ']') {
          this.pos++;
          break;
        }
        if (s[this.pos] === ',') {
          this.pos++;
          continue;
        }
        const before = this.pos;
        items.push(this.value());
        if (this.pos === before) this.pos++; // malformed input: always make progress
      }
      return { kind: 'array', start, end: this.pos, items };
    }
    if (c === '{') {
      this.pos++;
      const entries: KeyValueNode[] = [];
      for (;;) {
        this.skipTrivia();
        if (this.pos >= s.length) break;
        if (s[this.pos] === '}') {
          this.pos++;
          break;
        }
        if (s[this.pos] === ',') {
          this.pos++;
          continue;
        }
        const before = this.pos;
        const entry = this.keyValue();
        if (entry) entries.push(entry);
        if (this.pos === before) this.pos++;
      }
      return { kind: 'inline', start, end: this.pos, entries };
    }
    while (this.pos < s.length && !BARE_VALUE_END.has(s[this.pos]!)) this.pos++;
    // Dates may contain a space, so trim instead of stopping at whitespace.
    let end = this.pos;
    while (end > start && (s[end - 1] === ' ' || s[end - 1] === '\t')) end--;
    return { kind: 'bare', start, end };
  }

  /** Moves `pos` past the string that starts at `pos`. */
  private string(): void {
    const s = this.s;
    const quote = s[this.pos]!;
    const basic = quote === '"';
    const triple = quote.repeat(3);
    if (s.startsWith(triple, this.pos)) {
      this.pos += 3;
      while (this.pos < s.length) {
        if (basic && s[this.pos] === '\\') {
          this.pos += 2;
        } else if (s.startsWith(triple, this.pos)) {
          this.pos += 3;
          // Up to two quotes may sit directly before the closing delimiter.
          for (let extra = 0; extra < 2 && s[this.pos] === quote; extra++) this.pos++;
          return;
        } else {
          this.pos++;
        }
      }
      return;
    }
    this.pos++;
    while (this.pos < s.length) {
      const c = s[this.pos];
      if (basic && c === '\\') {
        this.pos += 2;
      } else if (c === quote) {
        this.pos++;
        return;
      } else if (c === '\n') {
        return;
      } else {
        this.pos++;
      }
    }
  }
}

/** Decodes a quoted key by letting the real parser handle the escapes. */
function decodeString(raw: string): string {
  try {
    const value = parse(`k = ${raw}`).k;
    if (typeof value === 'string') return value;
  } catch {
    // fall through
  }
  return raw.slice(1, -1);
}

/**
 * Turns header keys into an absolute path. A header that passes through an
 * array of tables refers to its most recent item, so `[profiles.cleanup]`
 * after the second `[[profiles]]` is `['profiles', 1, 'cleanup']`.
 */
function resolveHeader(keys: string[], array: boolean, counts: Map<string, number>): ConfigPath {
  const path: ConfigPath = [];
  keys.forEach((key, i) => {
    path.push(key);
    const id = JSON.stringify(path);
    const last = i === keys.length - 1;
    if (last && array) {
      const index = counts.get(id) ?? 0;
      counts.set(id, index + 1);
      path.push(index);
    } else if (!last) {
      const seen = counts.get(id);
      if (seen !== undefined) path.push(seen - 1);
    }
  });
  return path;
}

function collectValue(path: ConfigPath, node: ValueNode, owner: Owner, out: Located[]): void {
  out.push({ path, node, owner });
  if (node.kind === 'inline') {
    for (const entry of node.entries ?? []) {
      collectValue(
        [...path, ...entry.key],
        entry.value,
        { type: 'inline', table: node, entry },
        out,
      );
    }
  } else if (node.kind === 'array') {
    (node.items ?? []).forEach((item, index) => {
      collectValue([...path, index], item, { type: 'array', array: node, index }, out);
    });
  }
}

/** Scans TOML source into sections and located values. */
export function scanToml(source: string): TomlDoc {
  const sections = new Scanner(source).scan();
  const values: Located[] = [];
  for (const section of sections) {
    for (const entry of section.entries) {
      collectValue(
        [...section.path, ...entry.key],
        entry.value,
        { type: 'section', section, entry },
        values,
      );
    }
  }
  return { source, eol: source.includes('\r\n') ? '\r\n' : '\n', sections, values };
}

export function pathEquals(a: ConfigPath, b: ConfigPath): boolean {
  return a.length === b.length && a.every((part, i) => part === b[i]);
}

/** True when `prefix` is the start of `path` (or equal to it). */
export function isPathPrefix(prefix: ConfigPath, path: ConfigPath): boolean {
  return prefix.length <= path.length && prefix.every((part, i) => part === path[i]);
}

/** The value defined at exactly `path`, if it is written as a single token. */
export function findValue(doc: TomlDoc, path: ConfigPath): Located | undefined {
  return doc.values.find((located) => pathEquals(located.path, path));
}

/** Offset of the start of the line containing `offset`. */
export function lineStartOf(source: string, offset: number): number {
  return source.lastIndexOf('\n', offset - 1) + 1;
}

/** Offset just past the newline that ends the line containing `offset`. */
export function lineEndAfter(source: string, offset: number): number {
  const i = source.indexOf('\n', offset);
  return i === -1 ? source.length : i + 1;
}

function lineNumberAt(source: string, offset: number): number {
  let line = 1;
  for (let i = source.indexOf('\n'); i !== -1 && i < offset; i = source.indexOf('\n', i + 1))
    line++;
  return line;
}

/**
 * Returns a function that maps a config path to its 1-based line in `source`.
 * A path that is not written out (a missing required key, say) resolves to the
 * nearest enclosing key or table header.
 */
export function createLineLocator(source: string): (path: ConfigPath) => number | undefined {
  let doc: TomlDoc;
  try {
    doc = scanToml(source);
  } catch {
    return () => undefined;
  }
  return (path) => {
    for (let length = path.length; length >= 1; length--) {
      const prefix = path.slice(0, length);
      const exact = findValue(doc, prefix);
      if (exact) {
        const offset = exact.owner.type === 'array' ? exact.node.start : exact.owner.entry.keyStart;
        return lineNumberAt(source, offset);
      }
      const section = doc.sections.find((s) => s.header !== null && pathEquals(s.path, prefix));
      if (section) return lineNumberAt(source, section.start);
      // A table spelled with dotted keys has no line of its own: use its first key.
      const child = doc.values.find((located) => isPathPrefix(prefix, located.path));
      if (child) {
        const offset = child.owner.type === 'array' ? child.node.start : child.owner.entry.keyStart;
        return lineNumberAt(source, offset);
      }
      const nested = doc.sections.find((s) => s.header !== null && isPathPrefix(prefix, s.path));
      if (nested) return lineNumberAt(source, nested.start);
    }
    return undefined;
  };
}

/** The 1-based line of `path` in `source`, when it can be located. */
export function locateLine(source: string, path: ConfigPath): number | undefined {
  return createLineLocator(source)(path);
}
