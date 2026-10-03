/**
 * The config store: `config.toml` is the single source of settings. The store
 * reads, migrates and validates it, hot-reloads it when it is edited by hand,
 * and applies settings-window edits as in-place patches.
 */
import { EventEmitter } from 'node:events';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import { watch as watchPath, type FSWatcher } from 'chokidar';
import { parse, TomlError } from 'smol-toml';
import {
  CONFIG_VERSION,
  ConfigSchema,
  DEFAULT_CONFIG,
  validateConfig,
  type Config,
  type ConfigIssue,
  type ConfigPatch,
  type ConfigSnapshot,
} from '@shared/config';
import { writeFileAtomic } from './atomic-write';
import { diffPatches, fileVersion, MIGRATIONS, runMigrations, type Migration } from './migrations';
import { patchToml } from './patch';
import { createLineLocator } from './toml-doc';
import { isPlainObject, type PlainObject } from './toml-write';

/** What a new `config.toml` contains. */
export const DEFAULT_CONFIG_TEXT = `# Flow settings. Edit this file by hand or use the settings window: changes apply when you save.
# Every option has a default, so anything left out keeps its default value.
# API keys are never stored here. Providers hold a reference such as api_key = "secret:groq".

version = ${CONFIG_VERSION}
`;

export interface ConfigStoreOptions {
  /** Ordered migrations; defaults to the built-in list. */
  migrations?: Migration[];
  /** The version this build understands; defaults to `CONFIG_VERSION`. */
  version?: number;
  /** Quiet time after a file event before reloading, in milliseconds. */
  debounceMs?: number;
}

const BOM = '﻿';
// Matches `api_key = ...` and dotted forms such as `groq.api_key = ...`.
const API_KEY_LINE = /^[ \t]*(?:[\w"'-]+[ \t]*\.[ \t]*)*api_key[ \t]*=/;

function errorMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  // smol-toml appends a code excerpt after the first line.
  return message.split('\n')[0]!.trim();
}

/** Removes every provider's `api_key` from config text, leaving the rest untouched. */
export function stripApiKeys(text: string): string {
  try {
    const { providers } = parse(text);
    if (!isPlainObject(providers)) return text;
    const patches: ConfigPatch[] = [];
    for (const [id, provider] of Object.entries(providers)) {
      if (isPlainObject(provider) && Object.hasOwn(provider, 'api_key')) {
        patches.push({ path: ['providers', id, 'api_key'], value: undefined });
      }
    }
    return patchToml(text, patches);
  } catch {
    // The text does not parse, so it cannot be patched: drop the lines instead.
    return text
      .split('\n')
      .filter((line) => !API_KEY_LINE.test(line))
      .join('\n');
  }
}

function timestamp(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return (
    `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}` +
    `-${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`
  );
}

export class ConfigStore extends EventEmitter {
  readonly path: string;
  private readonly migrations: Migration[];
  private readonly version: number;
  private readonly debounceMs: number;

  private config: Config = structuredClone(DEFAULT_CONFIG);
  private currentIssues: ConfigIssue[] = [];
  /** The file's text as last read or written, without a byte-order mark. */
  private text: string | null = null;
  /** Notepad may add a byte-order mark; it is kept so our writes do not change the encoding. */
  private bom = false;
  private loaded = false;
  private queue: Promise<unknown> = Promise.resolve();
  private watcher: FSWatcher | null = null;
  private ready: Promise<void> = Promise.resolve();
  private timer: NodeJS.Timeout | null = null;

  constructor(filePath: string, options: ConfigStoreOptions = {}) {
    super();
    this.path = filePath;
    this.migrations = options.migrations ?? MIGRATIONS;
    this.version = options.version ?? CONFIG_VERSION;
    this.debounceMs = options.debounceMs ?? 150;
  }

  /**
   * Reads, migrates, validates. Never throws: problems end up in `issues()`.
   * Emits `changed` when a later call finds different settings.
   */
  load(): Promise<void> {
    return this.enqueue(() => this.reload(false));
  }

  get(): Config {
    return this.config;
  }

  issues(): ConfigIssue[] {
    return [...this.currentIssues];
  }

  snapshot(): ConfigSnapshot {
    return { config: this.config, issues: this.issues(), path: this.path };
  }

  /**
   * Patches the file in place, writes it atomically, reloads, emits `changed`.
   * Rejects, leaving the file alone, if it currently has a syntax error or a
   * patch addresses a list item that does not exist.
   */
  set(patches: ConfigPatch[]): Promise<ConfigSnapshot> {
    return this.enqueue(async () => {
      const text = await this.read();
      let next: string;
      try {
        next = patchToml(text, patches);
      } catch (error) {
        if (error instanceof TomlError) {
          throw new Error(
            `config.toml has a syntax error on line ${error.line}. Fix it before changing settings.`,
            { cause: error },
          );
        }
        throw error;
      }
      if (next !== text) await this.write(next);
      await this.ingest(next);
      return this.announce();
    });
  }

  /** Starts hot-reloading the file when it changes on disk. */
  watch(): void {
    if (this.watcher) return;
    const watcher = watchPath(this.path, { ignoreInitial: true });
    this.watcher = watcher;
    this.ready = new Promise((resolve) => watcher.once('ready', () => resolve()));
    watcher.on('all', () => this.scheduleReload());
    // A watcher error must not take the app down; the next save retries.
    watcher.on('error', () => undefined);
  }

  /** Resolves once the watcher started by `watch()` is seeing changes. */
  watchReady(): Promise<void> {
    return this.ready;
  }

  /** Stops watching and waits for pending work. */
  async close(): Promise<void> {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    const watcher = this.watcher;
    this.watcher = null;
    await watcher?.close();
    await this.queue;
  }

  /** Writes the current file to `targetPath` with every provider `api_key` line removed. */
  exportTo(targetPath: string): Promise<void> {
    return this.enqueue(async () => {
      const text = await this.read();
      await mkdir(dirname(targetPath), { recursive: true });
      await writeFile(targetPath, stripApiKeys(text));
    });
  }

  /**
   * Replaces the config with the file at `sourcePath`, after backing up the
   * current one to `config.toml.bak-<timestamp>`. Rejects, changing nothing,
   * if the file is not valid TOML or is not a Flow config. Invalid values are
   * accepted and reported in the snapshot, as they are for hand edits.
   * `api_key` references are dropped: secrets do not travel with a config.
   */
  importFrom(sourcePath: string): Promise<ConfigSnapshot> {
    return this.enqueue(async () => {
      let incoming = await readFile(sourcePath, 'utf8');
      if (incoming.startsWith(BOM)) incoming = incoming.slice(1);
      let raw: PlainObject;
      try {
        raw = parse(incoming);
      } catch (error) {
        const where = error instanceof TomlError ? ` (line ${error.line})` : '';
        throw new Error(
          `Cannot import: the file is not valid TOML${where}. ${errorMessage(error)}`,
          {
            cause: error,
          },
        );
      }
      const keys = Object.keys(raw);
      if (keys.length > 0 && !keys.some((key) => key in ConfigSchema.shape)) {
        throw new Error('Cannot import: the file does not look like a Flow config.');
      }
      const next = stripApiKeys(incoming);

      try {
        const current = await readFile(this.path);
        await writeFile(`${this.path}.bak-${timestamp(new Date())}`, current);
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
      }
      await this.write(next);
      await this.ingest(next);
      return this.announce();
    });
  }

  /** Runs tasks one at a time so reads, patches and reloads never interleave. */
  private enqueue<T>(task: () => Promise<T>): Promise<T> {
    const run = this.queue.then(task, task);
    this.queue = run.catch(() => undefined);
    return run;
  }

  private announce(): ConfigSnapshot {
    this.loaded = true;
    const snapshot = this.snapshot();
    this.emit('changed', snapshot);
    return snapshot;
  }

  private scheduleReload(): void {
    if (!this.watcher) return;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.enqueue(() => this.reload(true));
    }, this.debounceMs);
  }

  private async reload(fromWatcher: boolean): Promise<void> {
    const first = !this.loaded;
    let changed: boolean;
    try {
      const text = await this.read();
      // Our own writes come back through the watcher: same content, nothing to do.
      if (fromWatcher && text === this.text) return;
      changed = await this.ingest(text);
    } catch (error) {
      const issues: ConfigIssue[] = [
        { path: [], message: `Could not read config.toml: ${errorMessage(error)}` },
      ];
      changed = JSON.stringify(issues) !== JSON.stringify(this.currentIssues);
      this.currentIssues = issues;
    }
    this.loaded = true;
    if (changed && !first) this.emit('changed', this.snapshot());
  }

  /** Reads the file, creating it with the default header if it does not exist. */
  private async read(): Promise<string> {
    let text: string;
    try {
      text = await readFile(this.path, 'utf8');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
      this.bom = false;
      await writeFileAtomic(this.path, DEFAULT_CONFIG_TEXT);
      return DEFAULT_CONFIG_TEXT;
    }
    this.bom = text.startsWith(BOM);
    return this.bom ? text.slice(1) : text;
  }

  private async write(text: string): Promise<void> {
    await writeFileAtomic(this.path, this.bom ? BOM + text : text);
    this.text = text;
  }

  /**
   * Turns file text into the current config and issues. Returns whether
   * either changed. A syntax error keeps the last good config.
   */
  private async ingest(fileText: string): Promise<boolean> {
    const before = JSON.stringify([this.config, this.currentIssues]);
    let text = fileText;
    this.text = text;

    let raw: PlainObject;
    try {
      raw = parse(text);
    } catch (error) {
      const line = error instanceof TomlError ? error.line : undefined;
      this.currentIssues = [{ path: [], message: errorMessage(error), line }];
      return before !== JSON.stringify([this.config, this.currentIssues]);
    }

    const issues: ConfigIssue[] = [];
    const version = fileVersion(raw);
    if (version !== null && version < this.version) {
      try {
        const migrated = runMigrations(raw, version, this.version, this.migrations);
        if (migrated.version !== version) {
          const next = patchToml(text, diffPatches(raw, migrated.raw));
          raw = migrated.raw;
          await this.backup(`${this.path}.bak-v${version}`, text);
          await this.write(next);
          text = next;
          raw = parse(next);
        }
        if (migrated.version < this.version) {
          issues.push({
            path: ['version'],
            message: `No migration from config version ${migrated.version} to ${this.version}`,
          });
        }
      } catch (error) {
        issues.push({
          path: ['version'],
          message: `Could not migrate config from version ${version}: ${errorMessage(error)}`,
        });
      }
    } else if (version !== null && version > this.version) {
      issues.push({
        path: ['version'],
        message: `This file is at config version ${version}, newer than this version of Flow understands (${this.version}). Settings it does not know are ignored.`,
      });
    }

    const result = validateConfig(raw);
    const locate = createLineLocator(text);
    this.config = result.config;
    this.currentIssues = [...issues, ...result.issues].map((issue) => {
      const line = locate(issue.path);
      return line === undefined ? issue : { ...issue, line };
    });
    return before !== JSON.stringify([this.config, this.currentIssues]);
  }

  /** Keeps the first backup for a version: a re-run must not overwrite the true original. */
  private async backup(path: string, text: string): Promise<void> {
    try {
      await writeFile(path, this.bom ? BOM + text : text, { flag: 'wx' });
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error;
    }
  }
}
