/**
 * In-place edits of `config.toml`. The settings window changes values through
 * here so that hand-written comments, ordering and formatting survive: only the
 * tokens that have to change are touched.
 */
import { parse, stringify } from 'smol-toml';
import type { ConfigPatch, ConfigPath } from '@shared/config';
import {
  findValue,
  isPathPrefix,
  lineEndAfter,
  lineStartOf,
  scanToml,
  type Located,
  type SectionNode,
  type TomlDoc,
} from './toml-doc';
import {
  cloneValue,
  deepEqual,
  formatKey,
  formatKeyPath,
  formatTableLines,
  formatValue,
  getAt,
  isPlainObject,
  normaliseValue,
  removeAt,
  setAt,
  type PlainObject,
} from './toml-write';

/** Top-level lists written as `[[name]]` blocks. An empty one is simply absent from the file. */
const LIST_KEYS = new Set(['dictionary', 'snippets', 'profiles', 'local_models']);

/** A patch that cannot be applied, such as one addressing a list item that does not exist. */
export class TomlPatchError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'TomlPatchError';
  }
}

interface Range {
  start: number;
  end: number;
}

const isString = (part: string | number): part is string => typeof part === 'string';

function splice(source: string, start: number, end: number, text: string): string {
  return source.slice(0, start) + text + source.slice(end);
}

function isBlank(text: string): boolean {
  return text.trim() === '';
}

/** A non-empty array of tables, the shape of `[[dictionary]]`, `[[profiles]]` and friends. */
function isTableList(value: unknown): value is PlainObject[] {
  return Array.isArray(value) && value.length > 0 && value.every(isPlainObject);
}

/**
 * Removes ranges from `source`. A range that reaches the end of the file also
 * takes the blank lines before it (back to `floor` at most), so removals never
 * leave a ragged tail. `map` translates an offset in the old text to the new text.
 */
function removeRanges(
  source: string,
  ranges: Range[],
  floor = 0,
): { text: string; map: (offset: number) => number } {
  const merged: Range[] = [];
  for (const range of [...ranges].sort((a, b) => a.start - b.start)) {
    const last = merged[merged.length - 1];
    if (last && range.start <= last.end) last.end = Math.max(last.end, range.end);
    else merged.push({ ...range });
  }
  const tail = merged[merged.length - 1];
  if (tail && tail.end >= source.length) {
    while (tail.start > floor) {
      const previous = lineStartOf(source, tail.start - 1);
      if (!isBlank(source.slice(previous, tail.start))) break;
      tail.start = previous;
    }
    // The blank lines may have run back into the range before this one.
    while (merged.length > 1 && merged[merged.length - 2]!.end >= tail.start) {
      tail.start = Math.min(tail.start, merged[merged.length - 2]!.start);
      merged.splice(merged.length - 2, 1);
    }
  }
  let text = source;
  for (let i = merged.length - 1; i >= 0; i--)
    text = splice(text, merged[i]!.start, merged[i]!.end, '');
  const map = (offset: number): number => {
    let out = offset;
    for (const range of merged) {
      if (offset >= range.end) out -= range.end - range.start;
      else if (offset > range.start) out -= offset - range.start;
    }
    return out;
  };
  return { text, map };
}

/** The lines of one `key = value` entry, including a value that spans several lines. */
function entryRange(source: string, located: Located): Range {
  const keyStart =
    located.owner.type === 'array' ? located.node.start : located.owner.entry.keyStart;
  return { start: lineStartOf(source, keyStart), end: lineEndAfter(source, located.node.end) };
}

/** Offset just past the last `key = value` line of a section (or past its header). */
function bodyEnd(source: string, section: SectionNode): number {
  const last = section.entries[section.entries.length - 1];
  return last ? lineEndAfter(source, last.value.end) : section.bodyStart;
}

/**
 * A section from its header to its last key, plus the blank lines that follow.
 * Comments after the last key are left alone: they usually introduce whatever
 * comes next.
 */
function sectionRange(source: string, section: SectionNode): Range {
  let end = bodyEnd(source, section);
  while (end < source.length) {
    const next = lineEndAfter(source, end);
    if (!isBlank(source.slice(end, next))) break;
    end = next;
  }
  return { start: section.start, end };
}

/** Inserts a block of whole lines at `offset`, separated from its neighbours by a blank line. */
function insertBlock(
  source: string,
  offset: number,
  block: string,
  eol: string,
  spaceBefore = true,
): string {
  let before = source.slice(0, offset);
  const after = source.slice(offset);
  if (before !== '' && !before.endsWith('\n')) before += eol;
  const spaced = isBlank(before) || /(^|\n)[ \t]*\r?\n$/.test(before);
  const lead = spaced || !spaceBefore ? '' : eol;
  const trail = isBlank(after) || /^[ \t]*\r?\n/.test(after) ? '' : eol;
  return before + lead + block + trail + after;
}

function formatBlock(name: string, item: PlainObject, eol: string): string {
  const lines = formatTableLines(item, { multiline: eol === '\n' });
  return [`[[${formatKey(name)}]]`, ...lines].join(eol) + eol;
}

/** Sections that belong to the list `name`: its `[[name]]` blocks and their sub-tables. */
function listSections(doc: TomlDoc, name: string): SectionNode[] {
  return doc.sections.filter((section) => section.header !== null && section.path[0] === name);
}

/** Replaces every `[[name]]` block with fresh ones, at the position of the first. */
function replaceList(doc: TomlDoc, name: string, items: PlainObject[]): string {
  const { source, eol } = doc;
  const related = listSections(doc, name);
  const ranges = related.map((section) => sectionRange(source, section));
  // Anything else that defines `name` (dotted keys before the first header) has to go too.
  for (const located of doc.values) {
    if (located.owner.type === 'section' && located.path[0] === name) {
      ranges.push(entryRange(source, located));
    }
  }
  const first = related[0];
  const { text, map } = removeRanges(source, ranges, first?.start);
  const blocks = items.map((item) => formatBlock(name, item, eol)).join(eol);
  if (!first) return insertBlock(text, text.length, blocks, eol);
  // The new blocks take the place of the first old one, under whatever sat above it.
  return insertBlock(text, map(first.start), blocks, eol, false);
}

/** Adds one `[[name]]` block after the existing ones. */
function appendListItem(doc: TomlDoc, name: string, item: PlainObject): string {
  const { source, eol } = doc;
  const related = listSections(doc, name);
  const last = related[related.length - 1];
  const offset = last ? bodyEnd(source, last) : source.length;
  return insertBlock(source, offset, formatBlock(name, item, eol), eol);
}

/** Removes one entry of an inline table or one element of an array, with its comma. */
function removeNested(source: string, located: Located): string {
  const { owner } = located;
  if (owner.type === 'inline') {
    const entries = owner.table.entries ?? [];
    const i = entries.indexOf(owner.entry);
    if (entries.length <= 1) return splice(source, owner.table.start, owner.table.end, '{}');
    return i < entries.length - 1
      ? splice(source, owner.entry.keyStart, entries[i + 1]!.keyStart, '')
      : splice(source, entries[i - 1]!.value.end, owner.entry.value.end, '');
  }
  if (owner.type === 'array') {
    const items = owner.array.items ?? [];
    const i = owner.index;
    if (items.length <= 1) return splice(source, owner.array.start, owner.array.end, '[]');
    return i < items.length - 1
      ? splice(source, items[i]!.start, items[i + 1]!.start, '')
      : splice(source, items[i - 1]!.end, items[i]!.end, '');
  }
  return source;
}

/** Removes everything that defines `path`: key lines, dotted keys and whole table blocks. */
function removePath(text: string, path: ConfigPath): string {
  let source = text;
  for (let guard = 0; guard < 10_000; guard++) {
    const doc = scanToml(source);
    const hits = doc.values.filter((located) => isPathPrefix(path, located.path));
    // A value nested inside another hit goes when its container goes.
    const top = hits.filter(
      (located) =>
        !hits.some(
          (other) =>
            other.path.length < located.path.length && isPathPrefix(other.path, located.path),
        ),
    );
    const nested = top.find((located) => located.owner.type !== 'section');
    if (nested) {
      source = removeNested(source, nested);
      // Removing an array element shifts the next one into its index: stop here.
      if (nested.owner.type === 'array') return source;
      continue;
    }
    const ranges = top.map((located) => entryRange(source, located));
    for (const section of doc.sections) {
      if (section.header !== null && isPathPrefix(path, section.path)) {
        ranges.push(sectionRange(source, section));
      }
    }
    return removeRanges(source, ranges).text;
  }
  return source;
}

/** Adds an entry to an inline table. */
function insertInline(source: string, located: Located, keys: string[], value: unknown): string {
  const entry = `${formatKeyPath(keys)} = ${formatValue(value)}`;
  const entries = located.node.entries ?? [];
  const last = entries[entries.length - 1];
  if (!last) return splice(source, located.node.start, located.node.end, `{ ${entry} }`);
  return splice(source, last.value.end, last.value.end, `, ${entry}`);
}

/** Appends a `key = value` line to the body of a section. */
function appendLine(doc: TomlDoc, section: SectionNode, line: string): string {
  const { source, eol } = doc;
  if (section.header !== null || section.entries.length > 0) {
    const offset = bodyEnd(source, section);
    const lead = offset > 0 && source[offset - 1] !== '\n' ? eol : '';
    return splice(source, offset, offset, lead + line + eol);
  }
  // No top-level keys yet: put the first one under the file's leading comment.
  let offset = 0;
  while (offset < section.end) {
    const next = lineEndAfter(source, offset);
    if (!source.slice(offset, next).trimStart().startsWith('#')) break;
    offset = next;
  }
  let lead = '';
  if (offset > 0) lead = source[offset - 1] === '\n' ? eol : eol + eol;
  const following = source.slice(offset, lineEndAfter(source, offset));
  const trail = isBlank(following) ? '' : eol;
  return splice(source, offset, offset, lead + line + eol + trail);
}

/**
 * Writes one value as a single token: replaces it where it stands, or adds a
 * `key = value` line to the table that owns it, creating the table if needed.
 */
function setLeaf(doc: TomlDoc, parsed: unknown, path: ConfigPath, value: unknown): string {
  const { source, eol } = doc;
  const lineOptions = { multiline: eol === '\n' };

  const exact = findValue(doc, path);
  if (exact) {
    const options = exact.owner.type === 'section' ? lineOptions : {};
    return splice(source, exact.node.start, exact.node.end, formatValue(value, options));
  }

  // The path runs through a value written as a single token: an inline table
  // takes a new entry; anything else is rewritten with the change applied.
  for (let length = path.length - 1; length >= 1; length--) {
    const prefix = path.slice(0, length);
    const located = findValue(doc, prefix);
    if (!located) continue;
    const rest = path.slice(length);
    if (located.node.kind === 'inline' && rest.every(isString)) {
      return insertInline(source, located, rest, value);
    }
    const rewritten = setAt(cloneValue(getAt(parsed, prefix)), rest, value);
    return splice(source, located.node.start, located.node.end, formatValue(rewritten));
  }

  const tablePath = path.slice(0, -1);
  let owner = doc.sections[0]!;
  for (const section of doc.sections) {
    if (
      section.header !== null &&
      section.path.length <= tablePath.length &&
      section.path.length >= owner.path.length &&
      isPathPrefix(section.path, path)
    ) {
      owner = section;
    }
  }
  const rest = path.slice(owner.path.length);
  if (!rest.every(isString)) {
    throw new TomlPatchError(`No such list item: ${JSON.stringify(path)}`);
  }

  const emptyTable = isPlainObject(value) && Object.keys(value).length === 0;
  // Whether a dotted key is legal here depends on how the table is already spelled.
  const dottedHere = owner.entries.some((entry) => entry.key[0] === rest[0]);
  const firstStep = path.slice(0, owner.path.length + 1);
  const headerBelow = doc.sections.some(
    (section) =>
      section !== owner && section.header !== null && isPathPrefix(firstStep, section.path),
  );
  let newHeader: boolean;
  if (dottedHere) newHeader = false;
  else if (rest.length >= 2) newHeader = owner.header === null || headerBelow;
  else newHeader = emptyTable && owner.header === null;

  if (!newHeader) {
    return appendLine(doc, owner, `${formatKeyPath(rest)} = ${formatValue(value, lineOptions)}`);
  }

  const headerPath = emptyTable ? path : tablePath;
  let block = `[${formatKeyPath(headerPath.filter(isString))}]${eol}`;
  if (!emptyTable) {
    block += `${formatKey(rest[rest.length - 1]!)} = ${formatValue(value, lineOptions)}${eol}`;
  }
  let offset = source.length;
  const lastIndex = headerPath.findLastIndex((part) => typeof part === 'number');
  if (lastIndex !== -1) {
    // A sub-table of a list item must stay inside that item, not at the end of the file.
    const item = headerPath.slice(0, lastIndex + 1);
    const inItem = doc.sections.filter((section) => isPathPrefix(item, section.path));
    offset = inItem[inItem.length - 1]?.end ?? source.length;
  } else if (headerPath.length >= 2) {
    // Keep `[providers.openai]` next to `[providers.groq]` rather than below the profiles.
    const parent = headerPath.slice(0, -1);
    const siblings = doc.sections.filter(
      (section) => section.header !== null && isPathPrefix(parent, section.path),
    );
    const last = siblings[siblings.length - 1];
    if (last) offset = bodyEnd(source, last);
  }
  return insertBlock(source, offset, block, eol);
}

/** Makes the table at `path` hold exactly the keys of `value`, key by key. */
function setTable(text: string, path: ConfigPath, value: PlainObject, current: unknown): string {
  let out = text;
  if (isPlainObject(current)) {
    for (const key of Object.keys(current)) {
      if (!Object.hasOwn(value, key)) out = removePath(out, [...path, key]);
    }
  } else if (current !== undefined) {
    out = removePath(out, path);
  }
  for (const [key, child] of Object.entries(value)) out = setValue(out, [...path, key], child);
  // An empty table, or one whose keys were all dotted, still has to exist.
  const parsed = parse(out);
  if (getAt(parsed, path) === undefined) out = setLeaf(scanToml(out), parsed, path, {});
  return out;
}

function setValue(text: string, path: ConfigPath, value: unknown): string {
  const parsed = parse(text);
  const current = getAt(parsed, path);
  if (deepEqual(current, value)) return text;
  const doc = scanToml(text);
  const exact = findValue(doc, path);
  const head = path[0];

  if (isTableList(value) && path.length === 1 && typeof head === 'string' && !exact) {
    return replaceList(doc, head, value);
  }

  if (isPlainObject(value)) {
    // A scalar becoming a table keeps its place, as an inline table.
    if (exact && exact.node.kind !== 'inline') return setLeaf(doc, parsed, path, value);
    const newItem =
      path.length === 2 &&
      typeof head === 'string' &&
      typeof path[1] === 'number' &&
      current === undefined &&
      !findValue(doc, [head]);
    if (newItem) return appendListItem(doc, head, value);
    return setTable(text, path, value, current);
  }

  if (!exact && current !== undefined) {
    // A table or a list of blocks is being replaced by a plain value.
    const cleared = removePath(text, path);
    return setLeaf(scanToml(cleared), parse(cleared), path, value);
  }
  return setLeaf(doc, parsed, path, value);
}

/** Rejects paths that index past the end of a list, or into something that is not a list. */
function checkIndexes(root: unknown, path: ConfigPath): void {
  path.forEach((part, i) => {
    if (typeof part !== 'number') return;
    const parent = getAt(root, path.slice(0, i));
    const length = Array.isArray(parent) ? parent.length : parent === undefined ? 0 : -1;
    const isLast = i === path.length - 1;
    if (!Number.isInteger(part) || part < 0 || part > length || (part === length && !isLast)) {
      throw new TomlPatchError(`No such list item: ${JSON.stringify(path)}`);
    }
  });
}

/** Empty tables and empty lists read the same as absent ones. */
function prune(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(prune);
  if (!isPlainObject(value)) return value;
  const out: PlainObject = {};
  for (const [key, child] of Object.entries(value)) {
    const pruned = prune(child);
    const empty =
      (Array.isArray(pruned) && pruned.length === 0) ||
      (isPlainObject(pruned) && Object.keys(pruned).length === 0);
    if (!empty) out[key] = pruned;
  }
  return out;
}

function applyPatch(text: string, patch: ConfigPatch): string {
  const { path } = patch;
  if (path.length === 0 || typeof path[0] !== 'string') {
    throw new TomlPatchError(`Invalid config path: ${JSON.stringify(path)}`);
  }
  const before = parse(text);
  const current = getAt(before, path);
  let value = normaliseValue(patch.value);

  const head = path[0];
  if (Array.isArray(value) && value.length === 0 && path.length === 1 && typeof head === 'string') {
    // An emptied list of blocks: drop the blocks instead of writing `name = []`.
    if (listSections(scanToml(text), head).length > 0) value = undefined;
    else if (current === undefined && LIST_KEYS.has(head)) return text;
  }

  if (value === undefined ? current === undefined : deepEqual(current, value)) return text;
  checkIndexes(before, path);

  const expected = cloneValue(before);
  if (value === undefined) removeAt(expected, path);
  else setAt(expected, path, cloneValue(value));

  try {
    const out = value === undefined ? removePath(text, path) : setValue(text, path, value);
    if (deepEqual(prune(parse(out)), prune(expected))) return out;
  } catch (error) {
    if (error instanceof TomlPatchError) throw error;
  }
  // Last resort for a layout the in-place editor cannot express: the change
  // is kept and the formatting is not.
  return stringify(expected);
}

/**
 * Applies patches to TOML source text, preserving everything not touched.
 * Throws if `source` is not valid TOML, or a `TomlPatchError` for a path that
 * cannot be addressed.
 */
export function patchToml(source: string, patches: ConfigPatch[]): string {
  let text = source;
  for (const patch of patches) text = applyPatch(text, patch);
  return text;
}
