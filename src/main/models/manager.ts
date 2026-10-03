/**
 * The model manager: owns the catalog (bundled, then refreshed from a signed remote file),
 * local model files (download, verify, remove) and the status snapshot the UI renders.
 * No Electron imports; paths, config, secrets and `fetch` are injected.
 */
import { EventEmitter } from 'node:events';
import { statSync } from 'node:fs';
import { mkdir, readFile, rename, rm, stat, statfs, writeFile } from 'node:fs/promises';
import { basename, join, resolve } from 'node:path';
import {
  CUSTOM_PREFIX,
  type Catalog,
  type CatalogSnapshot,
  type CloudModelEntry,
  type DownloadProgress,
  type LocalModelEntry,
  type ModelEntry,
  type ModelStatus,
} from '@shared/catalog';
import type { Config } from '@shared/config';
import {
  CATALOG_PUBLIC_KEY,
  CATALOG_URL,
  EMPTY_CATALOG,
  parseCatalog,
  verifyCatalog,
} from './catalog';
import {
  abortError,
  ChecksumError,
  downloadFile,
  fileSize,
  isAbortError,
  sha256File,
} from './downloader';

export interface ModelManagerDeps {
  /** Where model files live: `%LOCALAPPDATA%\Flow\models`. */
  modelsDir: string;
  /** `%APPDATA%\Flow`; the refreshed catalog is cached here. */
  userDataDir: string;
  /** `resources/catalog.json`; its `.sig` sits beside it. */
  bundledCatalogPath: string;
  getConfig(): Config;
  hasSecret(id: string): boolean;
  /** Id of the local model currently loaded in the engine, if any. */
  loadedModelId?(): string | null;
  fetch?: typeof fetch;
  /** Overrides for tests. */
  catalogUrl?: string;
  catalogPublicKey?: string;
  /** Free bytes on the volume holding `dir`, or null if unknown. Defaults to `fs.statfs`. */
  freeBytes?(dir: string): Promise<number | null>;
}

export interface ModelManagerEvents {
  progress: [progress: DownloadProgress];
  changed: [];
}

/** A file whose SHA-256 has been checked against the manifest, keyed by file name in `installed.json`. */
interface VerifiedFile {
  sha256: string;
  sizeBytes: number;
  mtimeMs: number;
}

interface ActiveDownload {
  controller: AbortController;
  promise: Promise<void>;
}

const INDEX_FILE = 'installed.json';
const CACHED_CATALOG_FILE = 'catalog.json';
const PROGRESS_INTERVAL_MS = 200;
const CATALOG_TIMEOUT_MS = 15_000;
const UNKNOWN_SHA256 = '0'.repeat(64);
/** Model name sent to a custom endpoint that does not set `model`. */
const DEFAULT_CUSTOM_MODEL = 'whisper-1';

async function defaultFreeBytes(dir: string): Promise<number | null> {
  try {
    const stats = await statfs(dir);
    return stats.bavail * stats.bsize;
  } catch {
    return null;
  }
}

function formatBytes(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${Math.ceil(bytes / 1024 ** 2)} MB`;
}

async function writeAtomic(path: string, data: string | Uint8Array): Promise<void> {
  const tmp = `${path}.tmp`;
  await writeFile(tmp, data);
  await rename(tmp, path);
}

/** A one-line, user-facing reason for a failed download. */
function describeError(error: unknown): string {
  if (error instanceof ChecksumError) {
    return 'The downloaded file failed its checksum and was discarded. Try again.';
  }
  if (!(error instanceof Error)) return String(error);
  // `fetch` hides the useful part ("ENOTFOUND", "ECONNRESET") in `cause`.
  const cause = (error as { cause?: unknown }).cause;
  if (error.name === 'TypeError' && cause instanceof Error) {
    const code = (cause as NodeJS.ErrnoException).code;
    return `Network error: ${code ?? cause.message}`.split('\n')[0]!;
  }
  return error.message.split('\n')[0]!;
}

function errorCode(error: unknown): string | undefined {
  return error instanceof Error ? (error as NodeJS.ErrnoException).code : undefined;
}

export class ModelManager extends EventEmitter<ModelManagerEvents> {
  readonly #deps: ModelManagerDeps;
  #catalog: Catalog = EMPTY_CATALOG;
  #verified = new Map<string, VerifiedFile>();
  /** Files that failed verification but could not be renamed out of the way. */
  #corrupt = new Set<string>();
  #downloads = new Map<string, ActiveDownload>();
  /** Latest progress per model id. Kept after an error or cancel; dropped once installed. */
  #progress = new Map<string, DownloadProgress>();
  #background: Promise<void> = Promise.resolve();
  #refreshing: Promise<boolean> | null = null;

  constructor(deps: ModelManagerDeps) {
    super();
    this.#deps = deps;
  }

  /**
   * Loads the bundled catalog, then the cached refreshed one if it verifies and is at least as
   * new. Adopts model files already on disk. Never throws (the bundled catalog is the floor).
   */
  async init(): Promise<void> {
    await this.#loadCatalog();
    await this.#loadIndex();
    await this.#notePartialDownloads();
    this.#queueVerification();
  }

  catalog(): Catalog {
    return this.#catalog;
  }

  /**
   * Fetches the remote catalog and its signature; on a valid signature and schema, caches it and
   * emits 'changed'. Returns whether the catalog was updated. Never throws.
   */
  refreshCatalog(): Promise<boolean> {
    this.#refreshing ??= this.#refresh()
      .catch(() => false)
      .finally(() => {
        this.#refreshing = null;
      });
    return this.#refreshing;
  }

  /** The catalog with the runtime status of every model, for the UI. */
  snapshot(): CatalogSnapshot {
    const config = this.#deps.getConfig();
    const active = config.model.active;
    const loaded = this.#deps.loadedModelId?.() ?? null;
    const status: Record<string, ModelStatus> = {};

    const localStatus = (id: string, ready: boolean): ModelStatus => {
      const download = this.#progress.get(id);
      return {
        id,
        ready,
        active: active === id,
        loaded: loaded === id,
        ...(download ? { download } : {}),
      };
    };

    for (const entry of this.#catalog.local) {
      status[entry.id] = localStatus(entry.id, this.#fileOk(entry));
    }
    for (const entry of this.#catalog.cloud) {
      status[entry.id] = {
        id: entry.id,
        ready: this.#providerReady(entry.provider, config),
        active: active === entry.id,
        loaded: false,
      };
    }

    const custom: ModelEntry[] = [];
    for (const entry of this.#customLocal(config)) {
      custom.push(entry);
      status[entry.id] = { ...localStatus(entry.id, entry.sizeBytes > 0), custom: true };
    }
    for (const entry of this.#customCloud(config)) {
      custom.push(entry);
      status[entry.id] = {
        id: entry.id,
        ready: this.#providerReady(entry.provider, config),
        active: active === entry.id,
        loaded: false,
        custom: true,
      };
    }

    const keys: Record<string, boolean> = {};
    const providerIds = [
      ...this.#catalog.providers.map((p) => p.id),
      ...Object.keys(config.providers),
    ];
    for (const id of providerIds) keys[id] = this.#deps.hasSecret(this.#secretId(id, config));

    return { catalog: this.#catalog, status, keys, custom };
  }

  /** Catalog entry or a user-registered `[[local_models]]` file (presented as a LocalModelEntry). */
  localEntry(id: string): LocalModelEntry | undefined {
    return (
      this.#catalogLocal(id) ?? this.#customLocal(this.#deps.getConfig()).find((e) => e.id === id)
    );
  }

  /** Catalog entry, or the custom endpoint behind `custom/<provider-id>` from `config.providers`. */
  cloudEntry(id: string): CloudModelEntry | undefined {
    return (
      this.#catalog.cloud.find((e) => e.id === id) ??
      this.#customCloud(this.#deps.getConfig()).find((e) => e.id === id)
    );
  }

  isInstalled(id: string): boolean {
    return this.modelPath(id) !== null;
  }

  /** Absolute path of an installed local model's file, else null. */
  modelPath(id: string): string | null {
    const entry = this.#catalogLocal(id);
    if (entry) return this.#fileOk(entry) ? this.#pathOf(entry) : null;
    const custom = this.#customLocal(this.#deps.getConfig()).find((e) => e.id === id);
    return custom && custom.sizeBytes > 0 ? custom.file : null;
  }

  /**
   * Downloads a catalog model, resuming a partial file if there is one. One download runs per id
   * at a time; a second call joins the first. Emits throttled 'progress'. Resolves once the file
   * is verified and in place; rejects on failure, and with an `AbortError` when cancelled.
   */
  download(id: string): Promise<void> {
    const running = this.#downloads.get(id);
    if (running) {
      // A cancelled download is still winding down: start afresh once it has.
      if (running.controller.signal.aborted) {
        return running.promise.catch(() => undefined).then(() => this.download(id));
      }
      return running.promise;
    }
    const entry = this.#catalogLocal(id);
    if (!entry) {
      const custom = this.localEntry(id);
      return Promise.reject(
        new Error(
          custom
            ? `${custom.name} is a file you registered yourself; there is nothing to download`
            : `Unknown model: ${id}`,
        ),
      );
    }
    if (this.#fileOk(entry)) return Promise.resolve();

    const controller = new AbortController();
    const promise = this.#runDownload(entry, controller);
    this.#downloads.set(id, { controller, promise });
    return promise;
  }

  /** Stops a download. The partial file is kept so the next `download` resumes it. */
  cancelDownload(id: string): void {
    this.#downloads.get(id)?.controller.abort();
  }

  /**
   * Deletes a catalog model's file and any partial download. Rejects for a user-registered file,
   * which Flow never deletes: remove its `[[local_models]]` entry instead.
   */
  async remove(id: string): Promise<void> {
    const entry = this.#catalogLocal(id);
    if (!entry) {
      const custom = this.localEntry(id);
      throw new Error(
        custom
          ? `${custom.name} is a file you registered yourself, so Flow will not delete it. ` +
              'Remove its [[local_models]] entry from the config instead.'
          : `Unknown model: ${id}`,
      );
    }
    const running = this.#downloads.get(id);
    if (running) {
      running.controller.abort();
      await running.promise.catch(() => undefined);
    }
    const path = this.#pathOf(entry);
    try {
      await rm(path, { force: true });
    } catch (error) {
      const code = errorCode(error);
      if (code === 'EBUSY' || code === 'EPERM') {
        throw new Error(`${entry.name} is in use. Switch models or wait for it to unload first.`);
      }
      throw error;
    }
    await rm(`${path}.part`, { force: true });
    await rm(`${path}.corrupt`, { force: true });
    this.#verified.delete(entry.file);
    this.#corrupt.delete(entry.file);
    this.#progress.delete(id);
    await this.#saveIndex();
    this.emit('changed');
  }

  /** Ids of local models that are installed (catalog + user-registered whose file exists). */
  installedIds(): string[] {
    return [
      ...this.#catalog.local.filter((e) => this.#fileOk(e)).map((e) => e.id),
      ...this.#customLocal(this.#deps.getConfig())
        .filter((e) => e.sizeBytes > 0)
        .map((e) => e.id),
    ];
  }

  /** Resolves when background verification of files on disk has finished. */
  whenIdle(): Promise<void> {
    return this.#background;
  }

  // ── Catalog ────────────────────────────────────────────────────────────────

  get #publicKey(): string {
    return this.#deps.catalogPublicKey ?? CATALOG_PUBLIC_KEY;
  }

  get #cachedCatalogPath(): string {
    return join(this.#deps.userDataDir, CACHED_CATALOG_FILE);
  }

  async #loadCatalog(): Promise<void> {
    // The bundled file is trusted as part of the signed installer, so it loads even without a
    // matching signature (a contributor's build cannot re-sign it).
    try {
      this.#catalog = parseCatalog(await readFile(this.#deps.bundledCatalogPath));
    } catch {
      this.#catalog = EMPTY_CATALOG;
    }
    try {
      const [bytes, signature] = await Promise.all([
        readFile(this.#cachedCatalogPath),
        readFile(`${this.#cachedCatalogPath}.sig`, 'utf8'),
      ]);
      if (!verifyCatalog(bytes, signature, this.#publicKey)) return;
      const cached = parseCatalog(bytes);
      // An app update can ship a newer catalog than the one cached by the old version.
      if (cached.asOf >= this.#catalog.asOf) this.#catalog = cached;
    } catch {
      // No cache, or an unreadable one: the bundled catalog stands.
    }
  }

  async #refresh(): Promise<boolean> {
    const fetchFn = this.#deps.fetch ?? globalThis.fetch;
    const url = this.#deps.catalogUrl ?? CATALOG_URL;
    const signal = AbortSignal.timeout(CATALOG_TIMEOUT_MS);
    const get = async (target: string): Promise<Buffer> => {
      const response = await fetchFn(target, { signal, cache: 'no-store' });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      return Buffer.from(await response.arrayBuffer());
    };
    const [bytes, signature] = await Promise.all([get(url), get(`${url}.sig`)]);
    if (!verifyCatalog(bytes, signature.toString('utf8'), this.#publicKey)) return false;
    const next = parseCatalog(bytes);
    // A correctly signed but older catalog must not replace a newer one (replay).
    if (next.asOf < this.#catalog.asOf) return false;
    if (JSON.stringify(next) === JSON.stringify(this.#catalog)) return false;

    this.#catalog = next;
    try {
      await mkdir(this.#deps.userDataDir, { recursive: true });
      await writeAtomic(this.#cachedCatalogPath, bytes);
      await writeAtomic(`${this.#cachedCatalogPath}.sig`, signature);
    } catch {
      // Still usable for this run; the next launch refreshes again.
    }
    this.emit('changed');
    this.#queueVerification();
    return true;
  }

  // ── Entries and status ─────────────────────────────────────────────────────

  #catalogLocal(id: string): LocalModelEntry | undefined {
    return this.#catalog.local.find((e) => e.id === id);
  }

  #pathOf(entry: LocalModelEntry): string {
    return join(this.#deps.modelsDir, entry.file);
  }

  /** Cheap install check: the file exists with the manifest's exact size and is not known-bad. */
  #fileOk(entry: LocalModelEntry): boolean {
    if (this.#corrupt.has(entry.file)) return false;
    try {
      const stats = statSync(this.#pathOf(entry));
      return stats.isFile() && stats.size === entry.sizeBytes;
    } catch {
      return false;
    }
  }

  /** `[[local_models]]` files as entries. `file` is the absolute path; `sizeBytes` is 0 if missing. */
  #customLocal(config: Config): LocalModelEntry[] {
    const entries: LocalModelEntry[] = [];
    const seen = new Set(this.#catalog.local.map((e) => e.id));
    for (const model of config.local_models) {
      if (seen.has(model.id)) continue; // a catalog id, or a duplicate: the first one wins
      seen.add(model.id);
      // A bare file name means a file the user dropped into the models directory.
      const path = resolve(this.#deps.modelsDir, model.path);
      let sizeBytes = 0;
      try {
        const stats = statSync(path);
        if (stats.isFile()) sizeBytes = stats.size;
      } catch {
        // Missing: listed, but not ready.
      }
      entries.push({
        kind: 'local',
        id: model.id,
        name: model.name ?? basename(path),
        family: 'custom',
        description: 'Your own model file',
        url: '',
        file: path,
        sha256: UNKNOWN_SHA256,
        sizeBytes,
        languages: [config.general.language],
        licence: 'User-provided',
        runsOn: 'cpu',
        tier: 'core',
        note: path,
      });
    }
    return entries;
  }

  /** `[providers.<id>]` tables with a `base_url` that are not catalog providers, as `custom/<id>`. */
  #customCloud(config: Config): CloudModelEntry[] {
    const entries: CloudModelEntry[] = [];
    for (const [id, provider] of Object.entries(config.providers)) {
      if (!provider.base_url || this.#catalog.providers.some((p) => p.id === id)) continue;
      entries.push({
        kind: 'cloud',
        id: `${CUSTOM_PREFIX}${id}`,
        provider: id,
        model: provider.model ?? DEFAULT_CUSTOM_MODEL,
        name: id,
        description: `Custom endpoint at ${provider.base_url}`,
        pricePerHour: 0,
      });
    }
    return entries;
  }

  /** The secret a provider's key is stored under: `api_key = "secret:<id>"`, else the provider id. */
  #secretId(providerId: string, config: Config): string {
    const reference = config.providers[providerId]?.api_key;
    return reference?.slice('secret:'.length) || providerId;
  }

  #providerReady(providerId: string, config: Config): boolean {
    const provider = this.#catalog.providers.find((p) => p.id === providerId);
    if (provider?.keyless) return true;
    // A custom endpoint with no `api_key` line is taken to need no key.
    if (!provider && !config.providers[providerId]?.api_key) return true;
    return this.#deps.hasSecret(this.#secretId(providerId, config));
  }

  // ── Downloads ──────────────────────────────────────────────────────────────

  async #runDownload(entry: LocalModelEntry, controller: AbortController): Promise<void> {
    const { id, sizeBytes: total } = entry;
    const destPath = this.#pathOf(entry);
    const partPath = `${destPath}.part`;
    let progress: DownloadProgress = {
      id,
      receivedBytes: 0,
      totalBytes: total,
      speed: 0,
      state: 'downloading',
    };
    const publish = (change: Partial<DownloadProgress>): void => {
      progress = { ...progress, ...change };
      this.#progress.set(id, progress);
      this.emit('progress', progress);
    };
    const finish = (): void => {
      if (this.#downloads.get(id)?.controller === controller) this.#downloads.delete(id);
    };

    try {
      await mkdir(this.#deps.modelsDir, { recursive: true });
      const have = Math.min(await fileSize(partPath), total);
      publish({ receivedBytes: have });

      const free = await (this.#deps.freeBytes ?? defaultFreeBytes)(this.#deps.modelsDir);
      if (free !== null && free < total - have) {
        throw new Error(
          `Not enough disk space: ${formatBytes(total - have)} needed, ${formatBytes(free)} free`,
        );
      }

      // Speed is measured between emitted samples, starting from the first byte of this session
      // so that bytes already on disk from an earlier attempt do not count.
      let sampleAt = 0;
      let sampleBytes = 0;
      let speed = 0;
      await downloadFile({
        url: entry.url,
        destPath,
        sha256: entry.sha256,
        sizeBytes: total,
        fetch: this.#deps.fetch,
        signal: controller.signal,
        onProgress: (received) => {
          if (received >= total) {
            publish({ receivedBytes: total, speed: 0, state: 'verifying' });
            return;
          }
          const now = Date.now();
          if (sampleAt === 0 || received < sampleBytes) {
            sampleAt = now;
            sampleBytes = received;
            // Differs from the first report only if the server made us start over.
            if (received !== progress.receivedBytes) publish({ receivedBytes: received });
            return;
          }
          const elapsed = now - sampleAt;
          if (elapsed < PROGRESS_INTERVAL_MS) return;
          const current = ((received - sampleBytes) * 1000) / elapsed;
          speed = speed === 0 ? current : speed * 0.7 + current * 0.3;
          sampleAt = now;
          sampleBytes = received;
          publish({ receivedBytes: received, speed: Math.round(speed) });
        },
      });

      const stats = await stat(destPath);
      this.#verified.set(entry.file, {
        sha256: entry.sha256.toLowerCase(),
        sizeBytes: stats.size,
        mtimeMs: stats.mtimeMs,
      });
      this.#corrupt.delete(entry.file);
      await this.#saveIndex();
      finish();
      this.#progress.delete(id);
      this.emit('progress', { ...progress, receivedBytes: total, speed: 0, state: 'done' });
      this.emit('changed');
    } catch (error) {
      finish();
      const receivedBytes = Math.min(await fileSize(partPath), total);
      if (controller.signal.aborted || isAbortError(error)) {
        publish({ receivedBytes, speed: 0, state: 'cancelled' });
        throw isAbortError(error) ? error : abortError();
      }
      publish({ receivedBytes, speed: 0, state: 'error', error: describeError(error) });
      throw error;
    }
  }

  /** Surfaces partial files from an earlier run as resumable (`cancelled`) downloads. */
  async #notePartialDownloads(): Promise<void> {
    for (const entry of this.#catalog.local) {
      const receivedBytes = await fileSize(`${this.#pathOf(entry)}.part`);
      if (receivedBytes === 0 || receivedBytes > entry.sizeBytes || this.#fileOk(entry)) continue;
      this.#progress.set(entry.id, {
        id: entry.id,
        receivedBytes,
        totalBytes: entry.sizeBytes,
        speed: 0,
        state: 'cancelled',
      });
    }
  }

  // ── Verified-file index ────────────────────────────────────────────────────

  get #indexPath(): string {
    return join(this.#deps.modelsDir, INDEX_FILE);
  }

  async #loadIndex(): Promise<void> {
    this.#verified.clear();
    try {
      const parsed = JSON.parse(await readFile(this.#indexPath, 'utf8')) as {
        files?: Record<string, Partial<VerifiedFile>>;
      };
      for (const [file, record] of Object.entries(parsed.files ?? {})) {
        if (
          typeof record?.sha256 === 'string' &&
          typeof record.sizeBytes === 'number' &&
          typeof record.mtimeMs === 'number'
        ) {
          this.#verified.set(file, record as VerifiedFile);
        }
      }
    } catch {
      // Missing or unreadable: every file on disk is simply verified again.
    }
  }

  async #saveIndex(): Promise<void> {
    try {
      await mkdir(this.#deps.modelsDir, { recursive: true });
      const files = Object.fromEntries([...this.#verified].sort(([a], [b]) => a.localeCompare(b)));
      await writeAtomic(this.#indexPath, `${JSON.stringify({ version: 1, files }, null, 2)}\n`);
    } catch {
      // The index is a cache of work already done; losing it only costs a re-hash.
    }
  }

  #queueVerification(): void {
    this.#background = this.#background.then(() => this.#verifyFiles()).catch(() => undefined);
  }

  /**
   * Hashes every catalog file on disk that has the right size but no matching record, such as a
   * file copied in by hand. A mismatch is renamed to `<file>.corrupt` and stops counting as
   * installed. Size alone decides `isInstalled` in the meantime: hashing 1.6 GB per query is too slow.
   */
  async #verifyFiles(): Promise<void> {
    let changed = false;
    let pruned = false;
    for (const entry of this.#catalog.local) {
      if (this.#downloads.has(entry.id)) continue;
      const path = this.#pathOf(entry);
      const stats = await stat(path).catch(() => null);
      if (!stats?.isFile() || stats.size !== entry.sizeBytes) continue;
      const expected = entry.sha256.toLowerCase();
      const record = this.#verified.get(entry.file);
      if (
        record?.sha256 === expected &&
        record.sizeBytes === stats.size &&
        record.mtimeMs === stats.mtimeMs
      ) {
        continue;
      }

      let actual: string;
      try {
        actual = await sha256File(path);
      } catch {
        continue; // unreadable right now; try again on the next launch
      }
      changed = true;
      if (actual === expected) {
        this.#verified.set(entry.file, {
          sha256: expected,
          sizeBytes: stats.size,
          mtimeMs: stats.mtimeMs,
        });
        this.#corrupt.delete(entry.file);
        continue;
      }
      this.#verified.delete(entry.file);
      try {
        await rm(`${path}.corrupt`, { force: true });
        await rename(path, `${path}.corrupt`);
      } catch {
        // Locked (for example loaded in the engine): remember it instead.
        this.#corrupt.add(entry.file);
      }
    }

    // Forget records of files that are gone.
    for (const file of [...this.#verified.keys()]) {
      if ((await fileSize(join(this.#deps.modelsDir, file))) === 0) {
        this.#verified.delete(file);
        pruned = true;
      }
    }

    if (changed || pruned) await this.#saveIndex();
    if (changed) this.emit('changed');
  }
}
