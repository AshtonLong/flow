import { existsSync } from 'node:fs';
import { mkdtemp, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parse } from 'smol-toml';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { CONFIG_VERSION, DEFAULT_CONFIG, type ConfigSnapshot } from '@shared/config';
import {
  ConfigStore,
  DEFAULT_CONFIG_TEXT,
  stripApiKeys,
  type ConfigStoreOptions,
  type Migration,
} from '../../../src/main/config';

const EXAMPLE = (
  await readFile(new URL('./fixtures/example.toml', import.meta.url), 'utf8')
).replace(/\r\n/g, '\n');

let dir: string;
let file: string;
const stores: ConfigStore[] = [];

function open(options?: ConfigStoreOptions): ConfigStore {
  const store = new ConfigStore(file, options);
  stores.push(store);
  return store;
}

const read = () => readFile(file, 'utf8');
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** The next `changed` event, or a rejection after `ms`. */
function nextChange(store: ConfigStore, ms = 3000): Promise<ConfigSnapshot> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`no change within ${ms} ms`)), ms);
    store.once('changed', (snapshot: ConfigSnapshot) => {
      clearTimeout(timer);
      resolve(snapshot);
    });
  });
}

beforeEach(async () => {
  dir = await mkdtemp(join(tmpdir(), 'flow-config-'));
  file = join(dir, 'Flow', 'config.toml');
});

afterEach(async () => {
  await Promise.all(stores.splice(0).map((store) => store.close()));
  await rm(dir, { recursive: true, force: true });
});

async function seed(text: string): Promise<void> {
  const { mkdir } = await import('node:fs/promises');
  await mkdir(join(dir, 'Flow'), { recursive: true });
  await writeFile(file, text);
}

describe('ConfigStore: loading', () => {
  it('returns defaults before loading', () => {
    const store = open();
    expect(store.path).toBe(file);
    expect(store.get()).toEqual(DEFAULT_CONFIG);
    expect(store.issues()).toEqual([]);
  });

  it('creates a missing file with a commented header and the version', async () => {
    const store = open();
    await store.load();
    const text = await read();
    expect(text).toBe(DEFAULT_CONFIG_TEXT);
    expect(text.startsWith('#')).toBe(true);
    expect(text).toContain(`version = ${CONFIG_VERSION}\n`);
    expect(parse(text)).toEqual({ version: CONFIG_VERSION });
    expect(store.get()).toEqual(DEFAULT_CONFIG);
    expect(store.issues()).toEqual([]);
    expect(store.snapshot()).toEqual({ config: DEFAULT_CONFIG, issues: [], path: file });
  });

  it('accepts an empty file and leaves it alone', async () => {
    await seed('');
    const store = open();
    await store.load();
    expect(store.get()).toEqual(DEFAULT_CONFIG);
    expect(store.issues()).toEqual([]);
    expect(await read()).toBe('');
  });

  it('loads the spec example without issues', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    expect(store.issues()).toEqual([]);
    const config = store.get();
    expect(config.general.launch_at_login).toBe(true);
    expect(config.providers['custom-lan']).toEqual({
      base_url: 'http://192.168.1.20:8000/v1',
      api_key: 'secret:custom-lan',
    });
    expect(config.profiles.map((profile) => profile.name)).toEqual(['Terminals', 'Slack']);
    expect(config.profiles[0]!.insert).toEqual({ paste_shortcut: 'Ctrl+Shift+V' });
    expect(await read()).toBe(EXAMPLE);
  });

  it('falls back to the default for a bad key and reports its line', async () => {
    await seed(
      [
        'version = 1',
        '',
        '[audio]',
        'sounds = "loud"   # line 4',
        'max_recording_seconds = 2',
        'keep_mic_warm = true',
        '',
        '[overlay]',
        'style = "banner"',
        '',
        '[providers.groq]',
        'api_key = "gsk_plaintext"',
        '',
        '[[profiles]]',
        'name = "A"',
        'insert.method = "bogus"',
        '',
      ].join('\n'),
    );
    const store = open();
    await store.load();
    const config = store.get();
    expect(config.audio.sounds).toBe(true);
    expect(config.audio.max_recording_seconds).toBe(300);
    expect(config.audio.keep_mic_warm).toBe(true);
    expect(config.overlay.style).toBe('pill');
    expect(config.providers.groq).toEqual({});
    // Override values are only checked when a profile is applied.
    expect(config.profiles).toHaveLength(1);
    const byPath = Object.fromEntries(
      store.issues().map((issue) => [issue.path.join('.'), issue.line]),
    );
    expect(byPath).toEqual({
      'audio.sounds': 4,
      'audio.max_recording_seconds': 5,
      'overlay.style': 9,
      'providers.groq.api_key': 12,
    });
    for (const issue of store.issues()) expect(issue.message).not.toBe('');
  });

  it('reports the line of a broken list item, dotted keys and inline values', async () => {
    await seed(
      [
        'audio.sounds = 5',
        'insert = { method = "paste",',
        '  paste_shortcut = 7 }',
        '',
        '[[dictionary]]',
        'heard = ["a"]',
        'write = "A"',
        '',
        '[[dictionary]]',
        'heard = ["b"]',
        '',
        '[[snippets]]',
        'trigger = ""',
        'text = "x"',
        '',
      ].join('\n'),
    );
    const store = open();
    await store.load();
    expect(store.get().dictionary).toEqual([{ heard: ['a'], write: 'A' }]);
    expect(store.get().snippets).toEqual([]);
    const byPath = Object.fromEntries(
      store.issues().map((issue) => [issue.path.join('.'), issue.line]),
    );
    expect(byPath).toEqual({
      'audio.sounds': 1,
      'insert.paste_shortcut': 3,
      // The missing `write` has no line of its own: point at the item.
      'dictionary.1.write': 9,
      'snippets.0.trigger': 13,
    });
  });

  it('uses defaults and reports the line of a syntax error on first load', async () => {
    await seed('version = 1\n[audio]\nsounds = = true\n');
    const store = open();
    await store.load();
    expect(store.get()).toEqual(DEFAULT_CONFIG);
    expect(store.issues()).toEqual([
      { path: [], message: expect.stringContaining('Invalid TOML'), line: 3 },
    ]);
    expect(store.issues()[0]!.message).not.toContain('\n');
    expect(await read()).toBe('version = 1\n[audio]\nsounds = = true\n');
  });

  it('keeps the last good config when a later edit has a syntax error', async () => {
    await seed('[audio]\nsounds = false\n');
    const store = open();
    await store.load();
    expect(store.get().audio.sounds).toBe(false);

    await seed('[audio]\nsounds = false\nkeep_mic_warm = tru\n');
    const changed = nextChange(store);
    await store.load();
    const snapshot = await changed;
    expect(snapshot.config.audio.sounds).toBe(false);
    expect(snapshot.issues).toHaveLength(1);
    expect(snapshot.issues[0]!.line).toBe(3);

    await seed('[audio]\nsounds = false\nkeep_mic_warm = true\n');
    await store.load();
    expect(store.issues()).toEqual([]);
    expect(store.get().audio.keep_mic_warm).toBe(true);
  });

  it('reads a file saved with a byte-order mark and keeps the mark when writing', async () => {
    await seed('﻿[audio]\nsounds = false\n');
    const store = open();
    await store.load();
    expect(store.issues()).toEqual([]);
    expect(store.get().audio.sounds).toBe(false);
    await store.set([{ path: ['audio', 'sounds'], value: true }]);
    expect(await read()).toBe('﻿[audio]\nsounds = true\n');
  });

  it('never throws, even when the path cannot be read', async () => {
    const { mkdir } = await import('node:fs/promises');
    await mkdir(file, { recursive: true }); // a directory where the file should be
    const store = open();
    await expect(store.load()).resolves.toBeUndefined();
    expect(store.get()).toEqual(DEFAULT_CONFIG);
    expect(store.issues()).toHaveLength(1);
    expect(store.issues()[0]!.message).toContain('Could not read config.toml');
  });

  it('emits changed on a later load only when something changed', async () => {
    await seed('[audio]\nsounds = false\n');
    const store = open();
    const events: ConfigSnapshot[] = [];
    store.on('changed', (snapshot) => events.push(snapshot));
    await store.load();
    await store.load();
    expect(events).toHaveLength(0);
    await seed('[audio]\nsounds = true\n');
    await store.load();
    expect(events).toHaveLength(1);
    expect(events[0]!.config.audio.sounds).toBe(true);
  });
});

describe('ConfigStore: set', () => {
  it('patches the file in place, keeps comments, and emits changed', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const events: ConfigSnapshot[] = [];
    store.on('changed', (snapshot) => events.push(snapshot));

    const snapshot = await store.set([
      { path: ['general', 'theme'], value: 'dark' },
      { path: ['audio', 'sounds'], value: false },
      { path: ['providers', 'openai', 'api_key'], value: 'secret:openai' },
      { path: ['profiles', 1, 'cleanup', 'llm_prompt'], value: 'email' },
    ]);

    expect(snapshot.config.general.theme).toBe('dark');
    expect(snapshot.config.audio.sounds).toBe(false);
    expect(snapshot.config.providers.openai).toEqual({ api_key: 'secret:openai' });
    expect(snapshot.config.profiles[1]!.cleanup).toEqual({
      llm_enabled: true,
      llm_prompt: 'email',
    });
    expect(snapshot.issues).toEqual([]);
    expect(snapshot.path).toBe(file);
    expect(store.get()).toBe(snapshot.config);
    expect(events).toEqual([snapshot]);

    const text = await read();
    expect(text).toContain('theme = "dark"              # system | light | dark');
    const comment = (line: string) => line.slice(line.indexOf('#'));
    const before = EXAMPLE.split('\n').filter((line) => line.includes('#'));
    const after = text.split('\n').filter((line) => line.includes('#'));
    expect(after.map(comment)).toEqual(before.map(comment));
    expect(text.split('\n').length).toBe(EXAMPLE.split('\n').length + 3);
    expect(existsSync(`${file}.tmp`)).toBe(false);
  });

  it('does not rewrite the file when nothing changes', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const { stat } = await import('node:fs/promises');
    const before = (await stat(file)).mtimeMs;
    await sleep(20);
    const snapshot = await store.set([{ path: ['audio', 'sounds'], value: true }]);
    expect(snapshot.config.audio.sounds).toBe(true);
    expect((await stat(file)).mtimeMs).toBe(before);
    expect(await read()).toBe(EXAMPLE);
  });

  it('creates the file when setting before it exists', async () => {
    const store = open();
    const snapshot = await store.set([{ path: ['audio', 'sounds'], value: false }]);
    expect(snapshot.config.audio.sounds).toBe(false);
    expect(await read()).toBe(`${DEFAULT_CONFIG_TEXT}\n[audio]\nsounds = false\n`);
  });

  it('writes an invalid value but reports it and falls back to the default', async () => {
    await seed('[audio]\nsounds = true\n');
    const store = open();
    await store.load();
    const snapshot = await store.set([{ path: ['audio', 'max_recording_seconds'], value: 1 }]);
    expect(snapshot.config.audio.max_recording_seconds).toBe(300);
    expect(snapshot.issues).toEqual([
      { path: ['audio', 'max_recording_seconds'], message: expect.any(String), line: 3 },
    ]);
  });

  it('rejects and leaves the file alone while it has a syntax error', async () => {
    const broken = '[audio]\nsounds = = true\n';
    await seed(broken);
    const store = open();
    await store.load();
    await expect(store.set([{ path: ['audio', 'sounds'], value: false }])).rejects.toThrow(
      /syntax error on line 2/,
    );
    expect(await read()).toBe(broken);
  });

  it('rejects a patch for a list item that does not exist', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    await expect(store.set([{ path: ['profiles', 9, 'name'], value: 'x' }])).rejects.toThrow(
      /No such list item/,
    );
    expect(await read()).toBe(EXAMPLE);
    // The store still works afterwards.
    await expect(store.set([{ path: ['audio', 'sounds'], value: false }])).resolves.toBeDefined();
  });

  it('applies concurrent calls one after another', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const results = await Promise.all([
      store.set([{ path: ['audio', 'sounds'], value: false }]),
      store.set([{ path: ['general', 'theme'], value: 'light' }]),
      store.set([{ path: ['history', 'enabled'], value: true }]),
    ]);
    const last = results[2]!.config;
    expect([last.audio.sounds, last.general.theme, last.history.enabled]).toEqual([
      false,
      'light',
      true,
    ]);
    const text = await read();
    expect(text).toContain('sounds = false\n');
    expect(text).toContain('theme = "light" ');
    expect(text).toContain('[history]\nenabled = true\n');
  });

  it('patches the file as it is on disk, not as it was last loaded', async () => {
    await seed('[audio]\nsounds = true\n');
    const store = open();
    await store.load();
    await seed('# edited by hand\n[audio]\nsounds = true\nkeep_mic_warm = true\n');
    const snapshot = await store.set([{ path: ['audio', 'sounds'], value: false }]);
    expect(await read()).toBe('# edited by hand\n[audio]\nsounds = false\nkeep_mic_warm = true\n');
    expect(snapshot.config.audio.keep_mic_warm).toBe(true);
  });
});

describe('ConfigStore: watching', () => {
  it('applies an edit made in a text editor within a second', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    store.watch();
    await store.watchReady();

    const changed = nextChange(store);
    const started = Date.now();
    await writeFile(file, EXAMPLE.replace('sounds = true', 'sounds = false'));
    const snapshot = await changed;
    expect(Date.now() - started).toBeLessThan(1000);
    expect(snapshot.config.audio.sounds).toBe(false);
    expect(store.get().audio.sounds).toBe(false);
  });

  it('sees an editor that saves by replacing the file', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    store.watch();
    await store.watchReady();

    const changed = nextChange(store);
    const swap = join(dir, 'Flow', 'config.toml.swp');
    await writeFile(swap, EXAMPLE.replace('theme = "system"', 'theme = "light"'));
    await rename(swap, file);
    expect((await changed).config.general.theme).toBe('light');

    // And it keeps watching after the replacement.
    const again = nextChange(store);
    await writeFile(file, EXAMPLE.replace('theme = "system"', 'theme = "dark"'));
    expect((await again).config.general.theme).toBe('dark');
  });

  it('reports a syntax error from a hand edit and recovers when it is fixed', async () => {
    await seed('[audio]\nsounds = false\n');
    const store = open();
    await store.load();
    store.watch();
    await store.watchReady();

    let changed = nextChange(store);
    await writeFile(file, '[audio]\nsounds = fals\n');
    let snapshot = await changed;
    expect(snapshot.config.audio.sounds).toBe(false);
    expect(snapshot.issues).toEqual([{ path: [], message: expect.any(String), line: 2 }]);

    changed = nextChange(store);
    await writeFile(file, '[audio]\nsounds = true\n');
    snapshot = await changed;
    expect(snapshot.config.audio.sounds).toBe(true);
    expect(snapshot.issues).toEqual([]);
  });

  it('ignores the echo of its own writes', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    store.watch();
    await store.watchReady();
    const events: ConfigSnapshot[] = [];
    store.on('changed', (snapshot) => events.push(snapshot));

    await store.set([{ path: ['audio', 'sounds'], value: false }]);
    await sleep(600);
    expect(events).toHaveLength(1);
  });

  it('ignores a save that does not change the content', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    store.watch();
    await store.watchReady();
    const events: ConfigSnapshot[] = [];
    store.on('changed', (snapshot) => events.push(snapshot));
    await writeFile(file, EXAMPLE);
    await sleep(600);
    expect(events).toHaveLength(0);
  });

  it('stops reloading after close', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    store.watch();
    await store.watchReady();
    await store.close();
    const events: ConfigSnapshot[] = [];
    store.on('changed', (snapshot) => events.push(snapshot));
    await writeFile(file, EXAMPLE.replace('sounds = true', 'sounds = false'));
    await sleep(500);
    expect(events).toHaveLength(0);
    expect(store.get().audio.sounds).toBe(true);
  });
});

describe('ConfigStore: migrations', () => {
  /** A made-up v1 → v2 → v3 history, to exercise the framework. */
  const migrations: Migration[] = [
    {
      from: 1,
      to: 2,
      migrate(raw) {
        const audio = raw.audio as Record<string, unknown> | undefined;
        if (audio && 'sounds' in audio) {
          audio.feedback_sounds = audio.sounds;
          delete audio.sounds;
        }
        return raw;
      },
    },
    {
      from: 2,
      to: 3,
      migrate(raw) {
        return { ...raw, general: { ...(raw.general as object), theme: 'dark' } };
      },
    },
  ];

  it('has no migrations yet: version 1 is the first format', async () => {
    const { MIGRATIONS } = await import('../../../src/main/config');
    expect(CONFIG_VERSION).toBe(1);
    expect(MIGRATIONS).toEqual([]);
  });

  it('migrates an older file in place, in order, and keeps a backup', async () => {
    await seed(EXAMPLE);
    const store = open({ migrations, version: 3 });
    await store.load();

    expect(await readFile(`${file}.bak-v1`, 'utf8')).toBe(EXAMPLE);
    const text = await read();
    expect(text).toBe(
      EXAMPLE.replace('version = 1', 'version = 3')
        .replace('theme = "system"   ', 'theme = "dark"   ')
        .replace('sounds = true\nmax_recording_seconds = 300\n', 'max_recording_seconds = 300\n')
        .replace(
          'max_recording_seconds = 300\n',
          'max_recording_seconds = 300\nfeedback_sounds = true\n',
        ),
    );
    expect(store.get().version).toBe(3);
    expect(store.get().general.theme).toBe('dark');
    expect(store.issues()).toEqual([]);
  });

  it('does nothing to a file already at the current version', async () => {
    await seed(EXAMPLE.replace('version = 1', 'version = 3'));
    const store = open({ migrations, version: 3 });
    await store.load();
    expect(await read()).toBe(EXAMPLE.replace('version = 1', 'version = 3'));
    expect((await readdir(join(dir, 'Flow'))).sort()).toEqual(['config.toml']);
  });

  it('starts from the right step for a file part-way along', async () => {
    await seed('version = 2\n\n[audio]\nsounds = true # stays: the 1→2 step is not re-run\n');
    const store = open({ migrations, version: 3 });
    await store.load();
    expect(await read()).toBe(
      'version = 3\n\n[audio]\nsounds = true # stays: the 1→2 step is not re-run\n\n[general]\ntheme = "dark"\n',
    );
    expect(existsSync(`${file}.bak-v2`)).toBe(true);
  });

  it('treats a file with no version as the first version', async () => {
    await seed('[audio]\nsounds = false\n');
    const store = open({ migrations: [migrations[0]!], version: 2 });
    await store.load();
    expect(await read()).toBe('version = 2\n\n[audio]\nfeedback_sounds = false\n');
    expect(await readFile(`${file}.bak-v1`, 'utf8')).toBe('[audio]\nsounds = false\n');
  });

  it('does not overwrite an existing backup', async () => {
    await seed(EXAMPLE);
    await writeFile(`${file}.bak-v1`, 'the true original');
    const store = open({ migrations, version: 3 });
    await store.load();
    expect(await readFile(`${file}.bak-v1`, 'utf8')).toBe('the true original');
    expect(store.get().version).toBe(3);
  });

  it('leaves the file alone and reports it when a migration throws', async () => {
    await seed(EXAMPLE);
    const store = open({
      version: 2,
      migrations: [
        {
          from: 1,
          to: 2,
          migrate() {
            throw new Error('boom');
          },
        },
      ],
    });
    await store.load();
    expect(await read()).toBe(EXAMPLE);
    expect(existsSync(`${file}.bak-v1`)).toBe(false);
    expect(store.issues()).toEqual([
      { path: ['version'], message: expect.stringContaining('boom'), line: 1 },
    ]);
    expect(store.get().general.launch_at_login).toBe(true);
  });

  it('reports a missing migration step and a file from a newer version', async () => {
    await seed('version = 1\n');
    const older = open({ migrations: [], version: 2 });
    await older.load();
    expect(older.issues()).toEqual([
      { path: ['version'], message: expect.stringContaining('No migration'), line: 1 },
    ]);
    expect(await read()).toBe('version = 1\n');

    await seed('# from the future\nversion = 9\n[audio]\nsounds = false\n');
    const newer = open();
    await newer.load();
    expect(newer.issues()).toEqual([
      { path: ['version'], message: expect.stringContaining('newer'), line: 2 },
    ]);
    expect(newer.get().audio.sounds).toBe(false);
    expect(await read()).toBe('# from the future\nversion = 9\n[audio]\nsounds = false\n');
  });
});

describe('ConfigStore: export and import', () => {
  it('exports the file without any provider api_key line', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const target = join(dir, 'exports', 'flow-config.toml');
    await store.exportTo(target);
    const exported = await readFile(target, 'utf8');
    expect(exported).toBe(
      EXAMPLE.replace('api_key = "secret:groq"\n', '').replace(
        'api_key = "secret:custom-lan"\n',
        '',
      ),
    );
    expect(exported).not.toContain('api_key');
    expect(exported).toContain('base_url = "http://192.168.1.20:8000/v1"');
    // The live file is untouched.
    expect(await read()).toBe(EXAMPLE);
  });

  it('strips keys however they are spelled, and from a file that does not parse', () => {
    const inline =
      'providers = { groq = { api_key = "secret:groq", model = "m" } }\n[audio]\nsounds = true\n';
    expect(stripApiKeys(inline)).toBe(
      'providers = { groq = { model = "m" } }\n[audio]\nsounds = true\n',
    );
    const dotted = '[providers]\ngroq.api_key = "secret:groq"\ngroq.model = "m"\n';
    expect(stripApiKeys(dotted)).toBe('[providers]\ngroq.model = "m"\n');
    const broken = '[providers.groq]\napi_key = "secret:groq"\nmodel = = "m"\n';
    expect(stripApiKeys(broken)).toBe('[providers.groq]\nmodel = = "m"\n');
    // A snippet that happens to mention api_key is not a provider key.
    const other = '[[snippets]]\ntrigger = "k"\ntext = "api_key = 1"\n';
    expect(stripApiKeys(other)).toBe(other);
  });

  it('imports a file: backs up the current one, replaces it, drops key references', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const events: ConfigSnapshot[] = [];
    store.on('changed', (snapshot) => events.push(snapshot));

    const incoming = join(dir, 'incoming.toml');
    await writeFile(
      incoming,
      '# shared by a friend\nversion = 1\n\n[audio]\nsounds = false\n\n[providers.groq]\napi_key = "secret:groq"\nmodel = "whisper"\n',
    );
    const snapshot = await store.importFrom(incoming);

    expect(await read()).toBe(
      '# shared by a friend\nversion = 1\n\n[audio]\nsounds = false\n\n[providers.groq]\nmodel = "whisper"\n',
    );
    expect(snapshot.config.audio.sounds).toBe(false);
    expect(snapshot.config.providers).toEqual({ groq: { model: 'whisper' } });
    expect(snapshot.config.profiles).toEqual([]);
    expect(snapshot.issues).toEqual([]);
    expect(events).toEqual([snapshot]);

    const backups = (await readdir(join(dir, 'Flow'))).filter((name) =>
      /^config\.toml\.bak-\d{8}-\d{6}$/.test(name),
    );
    expect(backups).toHaveLength(1);
    expect(await readFile(join(dir, 'Flow', backups[0]!), 'utf8')).toBe(EXAMPLE);
  });

  it('round-trips an export through import', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const target = join(dir, 'export.toml');
    await store.exportTo(target);
    await store.set([{ path: ['audio', 'sounds'], value: false }]);
    const snapshot = await store.importFrom(target);
    expect(snapshot.config.audio.sounds).toBe(true);
    expect(snapshot.config.providers).toEqual({
      groq: {},
      'custom-lan': { base_url: 'http://192.168.1.20:8000/v1' },
    });
    expect(snapshot.config.profiles).toHaveLength(2);
  });

  it('rejects a file that is not valid TOML and changes nothing', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const incoming = join(dir, 'incoming.toml');
    await writeFile(incoming, 'version = 1\n[audio\nsounds = false\n');
    await expect(store.importFrom(incoming)).rejects.toThrow(/not valid TOML \(line 2\)/);
    expect(await read()).toBe(EXAMPLE);
    expect((await readdir(join(dir, 'Flow'))).sort()).toEqual(['config.toml']);
    expect(store.get().audio.sounds).toBe(true);
  });

  it('rejects a TOML file that is not a Flow config, and a missing file', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const incoming = join(dir, 'Cargo.toml');
    await writeFile(incoming, '[package]\nname = "x"\n');
    await expect(store.importFrom(incoming)).rejects.toThrow(/does not look like a Flow config/);
    await expect(store.importFrom(join(dir, 'nope.toml'))).rejects.toThrow();
    expect(await read()).toBe(EXAMPLE);
  });

  it('imports a file with invalid values and reports them', async () => {
    await seed(EXAMPLE);
    const store = open();
    await store.load();
    const incoming = join(dir, 'incoming.toml');
    await writeFile(incoming, '[audio]\nsounds = false\nmax_recording_seconds = 1\n');
    const snapshot = await store.importFrom(incoming);
    expect(snapshot.config.audio.sounds).toBe(false);
    expect(snapshot.config.audio.max_recording_seconds).toBe(300);
    expect(snapshot.issues).toEqual([
      { path: ['audio', 'max_recording_seconds'], message: expect.any(String), line: 3 },
    ]);
  });

  it('migrates an older imported file', async () => {
    await seed('version = 2\n');
    const store = open({
      version: 2,
      migrations: [{ from: 1, to: 2, migrate: (raw) => ({ ...raw, history: { enabled: true } }) }],
    });
    await store.load();
    const incoming = join(dir, 'old.toml');
    await writeFile(incoming, 'version = 1\n\n[audio]\nsounds = false\n');
    const snapshot = await store.importFrom(incoming);
    expect(snapshot.config.version).toBe(2);
    expect(snapshot.config.history.enabled).toBe(true);
    expect(await read()).toBe(
      'version = 2\n\n[audio]\nsounds = false\n\n[history]\nenabled = true\n',
    );
    expect(await readFile(`${file}.bak-v1`, 'utf8')).toBe(
      'version = 1\n\n[audio]\nsounds = false\n',
    );
  });
});
