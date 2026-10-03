/**
 * Config file migrations. Each step takes the parsed file at one version and
 * returns it at the next; the store writes the result back as in-place patches
 * so comments survive a migration too.
 */
import type { ConfigPatch, ConfigPath } from '@shared/config';
import { cloneValue, deepEqual, isPlainObject, type PlainObject } from './toml-write';

export interface Migration {
  from: number;
  to: number;
  /** Receives a private copy of the parsed file; may edit it in place or return a new object. */
  migrate(raw: PlainObject): PlainObject;
}

/** Ordered list of migrations. Version 1 is the first format, so there are none yet. */
export const MIGRATIONS: Migration[] = [];

/**
 * The version a parsed file is at. A file with no `version` key counts as the
 * first version, so later migrations still reach it; `null` means the value is
 * not a usable version number.
 */
export function fileVersion(raw: PlainObject): number | null {
  const { version } = raw;
  if (version === undefined) return 1;
  return typeof version === 'number' && Number.isInteger(version) && version >= 1 ? version : null;
}

/**
 * Runs every applicable migration in order, from `version` towards `target`.
 * Stops early if a step is missing. Throws whatever a migration throws.
 */
export function runMigrations(
  raw: PlainObject,
  version: number,
  target: number,
  migrations: Migration[],
): { raw: PlainObject; version: number } {
  let current = cloneValue(raw);
  let reached = version;
  while (reached < target) {
    const step = migrations.find((migration) => migration.from === reached);
    if (!step || step.to <= reached) break;
    current = step.migrate(current) ?? current;
    reached = step.to;
  }
  if (reached !== version) current.version = reached;
  return { raw: current, version: reached };
}

/** The patches that turn `before` into `after`. Tables are compared key by key. */
export function diffPatches(
  before: PlainObject,
  after: PlainObject,
  base: ConfigPath = [],
): ConfigPatch[] {
  const patches: ConfigPatch[] = [];
  for (const key of new Set([...Object.keys(before), ...Object.keys(after)])) {
    const path = [...base, key];
    const a = before[key];
    const b = after[key];
    if (deepEqual(a, b)) continue;
    if (isPlainObject(a) && isPlainObject(b)) patches.push(...diffPatches(a, b, path));
    else patches.push({ path, value: b });
  }
  return patches;
}
