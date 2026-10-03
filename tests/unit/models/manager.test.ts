import { existsSync } from 'node:fs';
import { mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { Catalog, DownloadProgress } from '@shared/catalog';
import { ConfigSchema, type Config } from '@shared/config';
import { isAbortError } from '../../../src/main/models/downloader';
import { ModelManager, type ModelManagerDeps } from '../../../src/main/models/manager';
import {
  encode,
  fakeServer,
  localEntry,
  randomBytes,
  sha256,
  signBytes,
  tempDir,
  testCatalog,
  testKey,
  type FakeServer,
} from './helpers';

const CATALOG_URL = 'https://catalog.test/catalog.json';
const key = testKey();
const alpha = randomBytes(40_000, 1);
const beta = randomBytes(30_000, 2);
const entryA = localEntry('alpha', alpha);
const entryB = localEntry('beta', beta);
const baseCatalog = testCatalog([entryA, entryB]);

let root: string;
let cleanup: () => Promise<void>;
let modelsDir: string;
let userDataDir: string;
let bundledCatalogPath: string;
let config: Config;
let secrets: Set<string>;
let loaded: string | null;
let server: FakeServer;

beforeEach(async () => {
  ({ dir: root, cleanup } = await tempDir());
  modelsDir = join(root, 'models');
  userDataDir = join(root, 'userData');
  bundledCatalogPath = join(root, 'resources', 'catalog.json');
  await mkdir(join(root, 'resources'), { recursive: true });
  await writeFile(bundledCatalogPath, encode(baseCatalog));
  config = ConfigSchema.parse({});
  secrets = new Set();
  loaded = null;
  server = fakeServer({
    [entryA.url]: { bytes: alpha },
    [entryB.url]: { bytes: beta },
  });
});
afterEach(() => cleanup());

function create(overrides: Partial<ModelManagerDeps> = {}): ModelManager {
  return new ModelManager({
    modelsDir,
    userDataDir,
    bundledCatalogPath,
    getConfig: () => config,
    hasSecret: (id) => secrets.has(id),
    loadedModelId: () => loaded,
    fetch: server.fetch,
    catalogUrl: CATALOG_URL,
    catalogPublicKey: key.publicPem,
    freeBytes: async () => 1e12,
    ...overrides,
  });
}

async function ready(overrides: Partial<ModelManagerDeps> = {}): Promise<ModelManager> {
  const manager = create(overrides);
  await manager.init();
  await manager.whenIdle();
  return manager;
}

/** Publishes a catalog (and its signature) on the fake server. */
function publish(catalog: Catalog, signature?: string): Buffer {
  const bytes = encode(catalog);
  server.files[CATALOG_URL] = { bytes };
  server.files[`${CATALOG_URL}.sig`] = {
    bytes: Buffer.from(signature ?? signBytes(bytes, key), 'utf8'),
  };
  return bytes;
}

async function cacheCatalog(catalog: Catalog, signature?: string): Promise<void> {
  const bytes = encode(catalog);
  await mkdir(userDataDir, { recursive: true });
  await writeFile(join(userDataDir, 'catalog.json'), bytes);
  await writeFile(join(userDataDir, 'catalog.json.sig'), signature ?? signBytes(bytes, key));
}

function collect(manager: ModelManager): { progress: DownloadProgress[]; changed: number } {
  const seen = { progress: [] as DownloadProgress[], changed: 0 };
  manager.on('progress', (p) => seen.progress.push(p));
  manager.on('changed', () => seen.changed++);
  return seen;
}

describe('catalog loading', () => {
  it('loads the bundled catalog', async () => {
    const manager = await ready();
    expect(manager.catalog()).toEqual(baseCatalog);
  });

  it('prefers a cached catalog that verifies and is at least as new', async () => {
    const newer = testCatalog([entryA], '2026-11-01');
    await cacheCatalog(newer);
    expect((await ready()).catalog()).toEqual(newer);

    const sameDay = testCatalog([entryB], baseCatalog.asOf);
    await cacheCatalog(sameDay);
    expect((await ready()).catalog()).toEqual(sameDay);
  });

  it('ignores a cached catalog that is older than the bundled one', async () => {
    await cacheCatalog(testCatalog([entryA], '2026-01-01'));
    expect((await ready()).catalog()).toEqual(baseCatalog);
  });

  it('ignores a cached catalog with a bad or missing signature', async () => {
    const newer = testCatalog([entryA], '2026-11-01');
    await cacheCatalog(newer, signBytes(encode(newer), testKey()));
    expect((await ready()).catalog()).toEqual(baseCatalog);

    await cacheCatalog(newer, 'garbage');
    expect((await ready()).catalog()).toEqual(baseCatalog);

    // Signed bytes, then edited on disk.
    await cacheCatalog(newer);
    const tampered = { ...newer, local: [{ ...entryA, url: 'https://evil.test/a.gguf' }] };
    await writeFile(join(userDataDir, 'catalog.json'), encode(tampered));
    expect((await ready()).catalog()).toEqual(baseCatalog);
  });

  it('does not throw when the bundled catalog is missing or broken', async () => {
    await writeFile(bundledCatalogPath, '{ not json');
    const manager = await ready();
    expect(manager.catalog().local).toEqual([]);
    expect(manager.snapshot().status).toEqual({});

    const missing = await ready({ bundledCatalogPath: join(root, 'nope.json') });
    expect(missing.catalog().cloud).toEqual([]);
  });
});

describe('refreshCatalog', () => {
  it('adopts and caches a signed, newer catalog', async () => {
    const manager = await ready();
    const seen = collect(manager);
    const newer = testCatalog([entryA], '2026-12-01');
    const bytes = publish(newer);

    expect(await manager.refreshCatalog()).toBe(true);
    expect(manager.catalog()).toEqual(newer);
    expect(seen.changed).toBe(1);
    expect((await readFile(join(userDataDir, 'catalog.json'))).equals(bytes)).toBe(true);
    expect(existsSync(join(userDataDir, 'catalog.json.sig'))).toBe(true);

    // The cache survives a restart, and an identical remote is not an update.
    expect((await ready()).catalog()).toEqual(newer);
    expect(await manager.refreshCatalog()).toBe(false);
    expect(seen.changed).toBe(1);
  });

  it('rejects tampered bytes, a wrong key and a missing signature', async () => {
    const manager = await ready();
    const seen = collect(manager);
    const newer = testCatalog([entryA], '2026-12-01');

    const signature = signBytes(encode(newer), key);
    publish({ ...newer, local: [{ ...entryA, url: 'https://evil.test/a.gguf' }] }, signature);
    expect(await manager.refreshCatalog()).toBe(false);

    publish(newer, signBytes(encode(newer), testKey()));
    expect(await manager.refreshCatalog()).toBe(false);

    publish(newer);
    delete server.files[`${CATALOG_URL}.sig`];
    expect(await manager.refreshCatalog()).toBe(false);

    expect(manager.catalog()).toEqual(baseCatalog);
    expect(seen.changed).toBe(0);
    expect(existsSync(join(userDataDir, 'catalog.json'))).toBe(false);
  });

  it('never rolls back to an older signed catalog', async () => {
    const manager = await ready();
    publish(testCatalog([entryA], '2026-01-01'));
    expect(await manager.refreshCatalog()).toBe(false);
    expect(manager.catalog()).toEqual(baseCatalog);
  });

  it('rejects a signed catalog that fails the schema', async () => {
    const manager = await ready();
    publish({ ...testCatalog([entryA], '2026-12-01'), version: 2 });
    expect(await manager.refreshCatalog()).toBe(false);
    publish({ asOf: '2026-12-01' } as unknown as Catalog);
    expect(await manager.refreshCatalog()).toBe(false);
    expect(manager.catalog()).toEqual(baseCatalog);
  });

  it('returns false on HTTP and network errors', async () => {
    const manager = await ready();
    server.files[CATALOG_URL] = { bytes: new Uint8Array(), status: 500 };
    expect(await manager.refreshCatalog()).toBe(false);

    const offline = await ready({
      fetch: async () => {
        throw new TypeError('fetch failed');
      },
    });
    expect(await offline.refreshCatalog()).toBe(false);
  });
});

describe('files on disk', () => {
  it('adopts a correct file that was copied in by hand', async () => {
    await mkdir(modelsDir, { recursive: true });
    await writeFile(join(modelsDir, entryA.file), alpha);

    const manager = create();
    const seen = collect(manager);
    await manager.init();
    // Installed by size at once; the hash is checked in the background.
    expect(manager.isInstalled('alpha')).toBe(true);
    expect(manager.modelPath('alpha')).toBe(join(modelsDir, entryA.file));

    await manager.whenIdle();
    expect(seen.changed).toBe(1);
    expect(manager.isInstalled('alpha')).toBe(true);
    expect(manager.installedIds()).toEqual(['alpha']);
    const index = JSON.parse(await readFile(join(modelsDir, 'installed.json'), 'utf8'));
    expect(index.files[entryA.file].sha256).toBe(entryA.sha256);
    expect(server.requests).toHaveLength(0);
  });

  it('quarantines a file with the right size but the wrong contents', async () => {
    await mkdir(modelsDir, { recursive: true });
    await writeFile(join(modelsDir, entryA.file), randomBytes(alpha.length, 55));

    const manager = create();
    const seen = collect(manager);
    await manager.init();
    await manager.whenIdle();

    expect(manager.isInstalled('alpha')).toBe(false);
    expect(manager.modelPath('alpha')).toBeNull();
    expect(seen.changed).toBe(1);
    expect(existsSync(join(modelsDir, entryA.file))).toBe(false);
    expect(existsSync(join(modelsDir, `${entryA.file}.corrupt`))).toBe(true);
  });

  it('does not hash a file again once it is recorded as verified', async () => {
    const path = join(modelsDir, entryA.file);
    await mkdir(modelsDir, { recursive: true });
    // Wrong contents, but a record saying it was verified: proof that no re-hash happens.
    await writeFile(path, randomBytes(alpha.length, 55));
    const { size, mtimeMs } = await stat(path);
    const files = { [entryA.file]: { sha256: entryA.sha256, sizeBytes: size, mtimeMs } };
    await writeFile(join(modelsDir, 'installed.json'), JSON.stringify({ version: 1, files }));

    const manager = create();
    const seen = collect(manager);
    await manager.init();
    await manager.whenIdle();
    expect(manager.isInstalled('alpha')).toBe(true);
    expect(seen.changed).toBe(0);
  });

  it('treats a file of the wrong size as not installed', async () => {
    await mkdir(modelsDir, { recursive: true });
    await writeFile(join(modelsDir, entryA.file), alpha.subarray(0, 100));
    const manager = await ready();
    expect(manager.isInstalled('alpha')).toBe(false);
    expect(manager.installedIds()).toEqual([]);
  });

  it('reports a leftover partial download as resumable', async () => {
    await mkdir(modelsDir, { recursive: true });
    await writeFile(join(modelsDir, `${entryA.file}.part`), alpha.subarray(0, 10_000));
    const manager = await ready();
    expect(manager.snapshot().status.alpha).toMatchObject({
      ready: false,
      download: { state: 'cancelled', receivedBytes: 10_000, totalBytes: alpha.length },
    });
  });
});

describe('download', () => {
  it('downloads, verifies and installs a model', async () => {
    const manager = await ready();
    const seen = collect(manager);

    await manager.download('alpha');

    expect(manager.isInstalled('alpha')).toBe(true);
    expect(sha256(await readFile(manager.modelPath('alpha')!))).toBe(entryA.sha256);
    expect(seen.changed).toBe(1);
    const states = seen.progress.map((p) => p.state);
    expect(states[0]).toBe('downloading');
    expect(states.slice(-2)).toEqual(['verifying', 'done']);
    expect(new Set(states)).toEqual(new Set(['downloading', 'verifying', 'done']));
    expect(seen.progress.every((p) => p.id === 'alpha' && p.totalBytes === alpha.length)).toBe(
      true,
    );
    expect(seen.progress.at(-1)?.receivedBytes).toBe(alpha.length);
    expect(manager.snapshot().status.alpha).toEqual({
      id: 'alpha',
      ready: true,
      active: false,
      loaded: false,
    });

    // Recorded as verified, so the next launch does not hash it again.
    const index = JSON.parse(await readFile(join(modelsDir, 'installed.json'), 'utf8'));
    expect(index.files[entryA.file].sha256).toBe(entryA.sha256);
    const next = create();
    const nextSeen = collect(next);
    await next.init();
    await next.whenIdle();
    expect(next.isInstalled('alpha')).toBe(true);
    expect(nextSeen.changed).toBe(0);
  });

  it('throttles progress events', async () => {
    const manager = await ready();
    const seen = collect(manager);
    await manager.download('alpha');
    // 40 chunks arrive within one 200 ms window: far fewer events than chunks.
    expect(server.requests).toHaveLength(1);
    expect(seen.progress.length).toBeLessThan(8);
  });

  it('runs one download per id and shares it between callers', async () => {
    const manager = await ready();
    const first = manager.download('alpha');
    const second = manager.download('alpha');
    expect(second).toBe(first);
    await Promise.all([first, second, manager.download('beta')]);
    expect(server.requests.map((r) => r.url).sort()).toEqual([entryA.url, entryB.url]);
    expect(manager.installedIds()).toEqual(['alpha', 'beta']);

    // Already installed: nothing to do.
    await manager.download('alpha');
    expect(server.requests).toHaveLength(2);
  });

  it('reports an error state with a one-line message', async () => {
    server.files[entryA.url] = { bytes: alpha, status: 503 };
    const manager = await ready();
    const seen = collect(manager);

    await expect(manager.download('alpha')).rejects.toThrow(/HTTP 503/);
    const last = seen.progress.at(-1)!;
    expect(last.state).toBe('error');
    expect(last.error).toMatch(/HTTP 503/);
    expect(last.error).not.toMatch(/\n/);
    expect(manager.snapshot().status.alpha).toMatchObject({
      ready: false,
      download: { state: 'error' },
    });
    expect(seen.changed).toBe(0);

    // A retry clears the error.
    server.files[entryA.url] = { bytes: alpha };
    await manager.download('alpha');
    expect(manager.snapshot().status.alpha?.download).toBeUndefined();
  });

  it('reports a checksum mismatch and leaves nothing installed', async () => {
    server.files[entryA.url] = { bytes: randomBytes(alpha.length, 77) };
    const manager = await ready();
    const seen = collect(manager);

    await expect(manager.download('alpha')).rejects.toThrow(/Checksum mismatch/);
    expect(seen.progress.at(-1)).toMatchObject({ state: 'error', receivedBytes: 0 });
    expect(seen.progress.at(-1)?.error).toMatch(/checksum/);
    expect(manager.isInstalled('alpha')).toBe(false);
    expect(existsSync(join(modelsDir, `${entryA.file}.part`))).toBe(false);
  });

  it('cancels, keeps the partial file, and resumes', async () => {
    const manager = await ready();
    const seen = collect(manager);
    server.onChunk = (sent) => {
      if (sent >= 10_000) manager.cancelDownload('alpha');
    };

    const error = await manager.download('alpha').catch((e: unknown) => e);
    expect(isAbortError(error)).toBe(true);
    const cancelled = seen.progress.at(-1)!;
    expect(cancelled.state).toBe('cancelled');
    expect(cancelled.receivedBytes).toBeGreaterThan(0);
    expect(cancelled.receivedBytes).toBeLessThan(alpha.length);
    expect((await stat(join(modelsDir, `${entryA.file}.part`))).size).toBe(cancelled.receivedBytes);
    expect(manager.isInstalled('alpha')).toBe(false);
    expect(manager.snapshot().status.alpha?.download?.state).toBe('cancelled');

    server.onChunk = undefined;
    await manager.download('alpha');
    expect(server.requests.at(-1)?.range).toBe(`bytes=${cancelled.receivedBytes}-`);
    expect(manager.isInstalled('alpha')).toBe(true);
    expect(seen.progress.at(-1)?.state).toBe('done');
  });

  it('restarts cleanly when download is called right after cancel', async () => {
    const manager = await ready();
    let cancelled = false;
    let retry: Promise<void> | undefined;
    server.onChunk = (sent) => {
      if (sent >= 10_000 && !cancelled) {
        cancelled = true;
        manager.cancelDownload('alpha');
        retry = manager.download('alpha');
      }
    };
    await expect(manager.download('alpha')).rejects.toSatisfy(isAbortError);
    await retry;
    expect(manager.isInstalled('alpha')).toBe(true);
  });

  it('fails early when the disk is too full', async () => {
    const manager = await ready({ freeBytes: async () => 1000 });
    const seen = collect(manager);
    await expect(manager.download('alpha')).rejects.toThrow(/Not enough disk space/);
    expect(seen.progress.at(-1)?.state).toBe('error');
    expect(server.requests).toHaveLength(0);
  });

  it('rejects unknown ids and user-registered files', async () => {
    config = ConfigSchema.parse({ local_models: [{ id: 'mine', path: 'C:\\mine.gguf' }] });
    const manager = await ready();
    await expect(manager.download('nope')).rejects.toThrow(/Unknown model/);
    await expect(manager.download('mine')).rejects.toThrow(/nothing to download/);
  });

  it('only downloads from the https URL in the catalog', async () => {
    // Bypass parseCatalog's own check to prove the downloader enforces it too.
    const manager = await ready();
    (manager.catalog().local[0] as { url: string }).url = 'http://models.test/alpha.gguf';
    await expect(manager.download('alpha')).rejects.toThrow(/not https/);
    expect(server.requests).toHaveLength(0);
  });
});

describe('remove', () => {
  it('deletes the file, its partial download and its record', async () => {
    const manager = await ready();
    await manager.download('alpha');
    const path = manager.modelPath('alpha')!;
    const seen = collect(manager);

    await manager.remove('alpha');
    expect(existsSync(path)).toBe(false);
    expect(manager.isInstalled('alpha')).toBe(false);
    expect(manager.installedIds()).toEqual([]);
    expect(seen.changed).toBe(1);
    const index = JSON.parse(await readFile(join(modelsDir, 'installed.json'), 'utf8'));
    expect(index.files).toEqual({});
  });

  it('cancels a running download and clears the partial file', async () => {
    const manager = await ready();
    let removal: Promise<void> | undefined;
    server.onChunk = (sent) => {
      if (sent >= 10_000) removal ??= manager.remove('alpha');
    };
    await expect(manager.download('alpha')).rejects.toSatisfy(isAbortError);
    await removal;
    expect(existsSync(join(modelsDir, `${entryA.file}.part`))).toBe(false);
    expect(manager.snapshot().status.alpha?.download).toBeUndefined();
  });

  it('refuses to delete a user-registered file', async () => {
    const mine = join(root, 'mine.gguf');
    await writeFile(mine, randomBytes(500));
    config = ConfigSchema.parse({ local_models: [{ id: 'mine', path: mine }] });
    const manager = await ready();

    await expect(manager.remove('mine')).rejects.toThrow(/registered yourself/);
    expect(existsSync(mine)).toBe(true);
    await expect(manager.remove('nope')).rejects.toThrow(/Unknown model/);
  });
});

describe('snapshot', () => {
  it('reports local, cloud and key status', async () => {
    await mkdir(modelsDir, { recursive: true });
    await writeFile(join(modelsDir, entryA.file), alpha);
    config = ConfigSchema.parse({
      model: { active: 'alpha' },
      providers: { openai: { api_key: 'secret:work-openai' } },
    });
    secrets = new Set(['groq', 'openai']);
    loaded = 'alpha';
    const manager = await ready();
    const snapshot = manager.snapshot();

    expect(snapshot.catalog).toEqual(baseCatalog);
    expect(snapshot.custom).toEqual([]);
    expect(Object.keys(snapshot.status).sort()).toEqual(
      ['alpha', 'beta', ...baseCatalog.cloud.map((e) => e.id)].sort(),
    );
    expect(snapshot.status.alpha).toEqual({
      id: 'alpha',
      ready: true,
      active: true,
      loaded: true,
    });
    expect(snapshot.status.beta).toEqual({
      id: 'beta',
      ready: false,
      active: false,
      loaded: false,
    });
    expect(snapshot.status['groq/whisper-large-v3-turbo']?.ready).toBe(true);
    // The key is looked up under the id the config references, not the provider id.
    expect(snapshot.status['openai/gpt-4o-transcribe']?.ready).toBe(false);
    expect(snapshot.status['ollama/whisper']?.ready).toBe(true);
    expect(snapshot.keys).toEqual({ groq: true, openai: false, ollama: false });

    secrets.add('work-openai');
    config = ConfigSchema.parse({
      model: { active: 'openai/gpt-4o-transcribe' },
      providers: { openai: { api_key: 'secret:work-openai' } },
    });
    const next = manager.snapshot();
    expect(next.status['openai/gpt-4o-transcribe']).toEqual({
      id: 'openai/gpt-4o-transcribe',
      ready: true,
      active: true,
      loaded: false,
    });
    expect(next.status.alpha?.active).toBe(false);
    expect(next.keys.openai).toBe(true);
  });

  it('shows a download in progress', async () => {
    const manager = await ready();
    let during: DownloadProgress | undefined;
    server.onChunk = () => {
      during ??= manager.snapshot().status.alpha?.download;
    };
    await manager.download('alpha');
    expect(during).toMatchObject({ id: 'alpha', state: 'downloading' });
  });

  it('lists user-registered files and custom endpoints', async () => {
    const mine = join(root, 'mine.gguf');
    await writeFile(mine, randomBytes(500));
    await mkdir(modelsDir, { recursive: true });
    await writeFile(join(modelsDir, 'dropped.gguf'), randomBytes(700));
    config = ConfigSchema.parse({
      model: { active: 'custom/lan' },
      providers: {
        groq: { base_url: 'https://proxy.test/groq/v1' },
        lan: { base_url: 'http://192.168.1.20:8000/v1', model: 'large-v3' },
        vpn: { base_url: 'https://stt.example.com/v1', api_key: 'secret:vpn-key' },
        nourl: { api_key: 'secret:nourl' },
      },
      local_models: [
        { id: 'mine', name: 'My model', path: mine },
        { id: 'dropped', path: 'dropped.gguf' },
        { id: 'gone', path: join(root, 'gone.gguf') },
        { id: 'alpha', path: mine },
      ],
    });
    loaded = 'mine';
    const manager = await ready();
    const snapshot = manager.snapshot();

    expect(snapshot.custom.map((e) => e.id)).toEqual([
      'mine',
      'dropped',
      'gone',
      'custom/lan',
      'custom/vpn',
    ]);
    expect(manager.localEntry('mine')).toMatchObject({
      kind: 'local',
      id: 'mine',
      name: 'My model',
      file: mine,
      sizeBytes: 500,
      sha256: '0'.repeat(64),
      licence: 'User-provided',
    });
    expect(manager.localEntry('dropped')).toMatchObject({ name: 'dropped.gguf', sizeBytes: 700 });
    // A catalog id cannot be shadowed by a registered file.
    expect(manager.localEntry('alpha')).toEqual(entryA);
    expect(manager.localEntry('nope')).toBeUndefined();

    expect(snapshot.status.mine).toEqual({
      id: 'mine',
      ready: true,
      active: false,
      loaded: true,
      custom: true,
    });
    expect(snapshot.status.gone).toMatchObject({ ready: false, custom: true });
    expect(manager.modelPath('mine')).toBe(mine);
    expect(manager.modelPath('dropped')).toBe(join(modelsDir, 'dropped.gguf'));
    expect(manager.modelPath('gone')).toBeNull();
    expect(manager.isInstalled('gone')).toBe(false);
    expect(manager.installedIds()).toEqual(['mine', 'dropped']);

    expect(manager.cloudEntry('custom/lan')).toEqual({
      kind: 'cloud',
      id: 'custom/lan',
      provider: 'lan',
      model: 'large-v3',
      name: 'lan',
      description: 'Custom endpoint at http://192.168.1.20:8000/v1',
      pricePerHour: 0,
    });
    expect(manager.cloudEntry('custom/vpn')?.model).toBe('whisper-1');
    expect(manager.cloudEntry('custom/groq')).toBeUndefined();
    expect(manager.cloudEntry('custom/nourl')).toBeUndefined();
    expect(manager.cloudEntry('groq/whisper-large-v3-turbo')?.provider).toBe('groq');

    // No `api_key` line means the endpoint needs no key; otherwise the key must be stored.
    expect(snapshot.status['custom/lan']).toEqual({
      id: 'custom/lan',
      ready: true,
      active: true,
      loaded: false,
      custom: true,
    });
    expect(snapshot.status['custom/vpn']).toMatchObject({ ready: false, custom: true });
    expect(snapshot.keys).toEqual({
      groq: false,
      openai: false,
      ollama: false,
      lan: false,
      vpn: false,
      nourl: false,
    });
    secrets.add('vpn-key');
    expect(manager.snapshot().status['custom/vpn']?.ready).toBe(true);
    expect(manager.snapshot().keys.vpn).toBe(true);
  });
});
