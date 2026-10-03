import { readFileSync } from 'node:fs';
import { parse } from 'smol-toml';
import { describe, expect, it } from 'vitest';
import type { ConfigPatch, ConfigPath } from '@shared/config';
import { validateConfig } from '@shared/config';
import { patchToml, TomlPatchError } from '../../../src/main/config/patch';

/** The example from SPEC.md "Configuration", comments included. */
const EXAMPLE = readFileSync(new URL('./fixtures/example.toml', import.meta.url), 'utf8').replace(
  /\r\n/g,
  '\n',
);

type Table = Record<string, unknown>;

const isTable = (value: unknown): value is Table =>
  value !== null && typeof value === 'object' && !Array.isArray(value);

/** Reference implementation of a patch, on the parsed structure. */
function applyToObject(root: Table, { path, value }: ConfigPatch): void {
  // Tables and lists are both indexed by the path parts here.
  let node = root as Record<string | number, unknown>;
  for (let i = 0; i < path.length - 1; i++) {
    const part = path[i]!;
    if (node[part] === undefined || typeof node[part] !== 'object') {
      if (value === undefined) return;
      node[part] = typeof path[i + 1] === 'number' ? [] : {};
    }
    node = node[part] as Record<string | number, unknown>;
  }
  const last = path[path.length - 1]!;
  if (value !== undefined) node[last] = structuredClone(value);
  else if (Array.isArray(node)) node.splice(last as number, 1);
  else delete node[last];
}

function applyAll(source: string, patches: ConfigPatch[]): Table {
  const root = parse(source) as Table;
  for (const patch of patches) applyToObject(root, patch);
  return root;
}

/** An empty table or list reads the same as an absent one. */
function prune(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(prune);
  if (!isTable(value)) return value;
  const out: Table = {};
  for (const [key, child] of Object.entries(value)) {
    const pruned = prune(child);
    if (Array.isArray(pruned) && pruned.length === 0) continue;
    if (isTable(pruned) && Object.keys(pruned).length === 0) continue;
    out[key] = pruned;
  }
  return out;
}

/** The comment part of every line that has one (no fixture has `#` inside a string). */
function comments(text: string, marker = '#'): string[] {
  return text
    .split(/\r?\n/)
    .filter((line) => line.includes(marker))
    .map((line) => line.slice(line.lastIndexOf('#')).trimEnd());
}

function expectCommentsKept(before: string, after: string, marker?: string): void {
  const kept = comments(after, marker);
  for (const comment of comments(before, marker)) expect(kept).toContain(comment);
}

/** Patches, then checks the result parses to "old parse + patches applied". */
function patchAndCheck(source: string, patches: ConfigPatch[]): string {
  const out = patchToml(source, patches);
  expect(parse(out)).toEqual(applyAll(source, patches));
  return out;
}

function lines(text: string): string[] {
  return text.split('\n');
}

describe('patchToml: existing values', () => {
  it('replaces only the value token of a key in a table', () => {
    const out = patchAndCheck(EXAMPLE, [{ path: ['general', 'theme'], value: 'dark' }]);
    expect(out).toBe(
      EXAMPLE.replace(
        'theme = "system"              # system | light | dark',
        'theme = "dark"              # system | light | dark',
      ),
    );
  });

  it('replaces booleans and numbers and keeps the trailing comment', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['audio', 'keep_mic_warm'], value: true },
      { path: ['model', 'keep_loaded_minutes'], value: -1 },
      { path: ['audio', 'max_recording_seconds'], value: 120 },
    ]);
    expect(out).toContain(
      'keep_mic_warm = true         # true = faster start, mic indicator stays on',
    );
    expect(out).toContain('keep_loaded_minutes = -1      # 0 = unload at once, -1 = never unload');
    expect(out).toContain('max_recording_seconds = 120\n');
    expectCommentsKept(EXAMPLE, out);
  });

  it('replaces the top-level version', () => {
    const out = patchAndCheck(EXAMPLE, [{ path: ['version'], value: 2 }]);
    expect(out).toBe(EXAMPLE.replace('version = 1', 'version = 2'));
  });

  it('replaces a dotted key inside a list item', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['profiles', 0, 'insert', 'paste_shortcut'], value: 'Shift+Insert' },
      { path: ['profiles', 1, 'cleanup', 'llm_enabled'], value: false },
    ]);
    expect(out).toBe(
      EXAMPLE.replace(
        'insert.paste_shortcut = "Ctrl+Shift+V"',
        'insert.paste_shortcut = "Shift+Insert"',
      ).replace(
        'model.active = "groq/whisper-large-v3-turbo"\ncleanup.llm_enabled = true',
        'model.active = "groq/whisper-large-v3-turbo"\ncleanup.llm_enabled = false',
      ),
    );
  });

  it('replaces a top-level dotted key', () => {
    const source = '# top\ninsert.paste_shortcut = "Ctrl+V"   # dotted\nother = 1\n';
    const out = patchAndCheck(source, [{ path: ['insert', 'paste_shortcut'], value: 'Ctrl+Y' }]);
    expect(out).toBe('# top\ninsert.paste_shortcut = "Ctrl+Y"   # dotted\nother = 1\n');
  });

  it('replaces a value inside an inline table', () => {
    const source = 'insert = { method = "paste", paste_shortcut = "Ctrl+V" }  # inline\n';
    const out = patchAndCheck(source, [{ path: ['insert', 'method'], value: 'type' }]);
    expect(out).toBe('insert = { method = "type", paste_shortcut = "Ctrl+V" }  # inline\n');
  });

  it('replaces a value inside a nested inline table of a list item', () => {
    const source = '[[profiles]]\nname = "A"\ncleanup = { llm_enabled = true, llm_prompt = "x" }\n';
    const out = patchAndCheck(source, [
      { path: ['profiles', 0, 'cleanup', 'llm_prompt'], value: 'casual' },
    ]);
    expect(out).toBe(
      '[[profiles]]\nname = "A"\ncleanup = { llm_enabled = true, llm_prompt = "casual" }\n',
    );
  });

  it('replaces an array of scalars', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['profiles', 0, 'match_process'], value: ['wt.exe', 'alacritty.exe', 'kitty.exe'] },
      { path: ['dictionary', 0, 'heard'], value: [] },
    ]);
    expect(out).toContain('match_process = ["wt.exe", "alacritty.exe", "kitty.exe"]\n');
    expect(out).toContain('[[dictionary]]\nheard = []\nwrite = "Tauri"\n');
  });

  it('replaces a multi-line array and a literal string, leaving neighbours alone', () => {
    const source = [
      '[[profiles]]',
      "name = 'Terminals'  # literal",
      'match_process = [',
      '  "a.exe",  # first',
      '  "b.exe",',
      ']  # after',
      'enabled = true',
      '',
    ].join('\n');
    const out = patchAndCheck(source, [
      { path: ['profiles', 0, 'match_process'], value: ['c.exe'] },
      { path: ['profiles', 0, 'name'], value: 'Shells' },
    ]);
    expect(out).toBe(
      '[[profiles]]\nname = "Shells"  # literal\nmatch_process = ["c.exe"]  # after\nenabled = true\n',
    );
  });

  it('replaces one element of an array', () => {
    const out = patchAndCheck(EXAMPLE, [{ path: ['dictionary', 0, 'heard', 1], value: 'tawri' }]);
    expect(out).toContain('heard = ["tory", "tawri"]');
  });

  it('handles keys that need quoting', () => {
    const source =
      '[providers."my server"]\nbase_url = "http://a"\n\n["odd.name"]\n"key with space" = 1\n';
    const out = patchAndCheck(source, [
      { path: ['providers', 'my server', 'base_url'], value: 'http://b' },
      { path: ['odd.name', 'key with space'], value: 2 },
      { path: ['odd.name', 'another one'], value: 3 },
    ]);
    expect(out).toBe(
      '[providers."my server"]\nbase_url = "http://b"\n\n["odd.name"]\n"key with space" = 2\n"another one" = 3\n',
    );
  });

  it('is not fooled by brackets, hashes and equals signs inside strings and comments', () => {
    const source = [
      '# [audio] sounds = true',
      '[prompts.clean]',
      'text = "use [brackets] # not a comment = here"  # [audio]',
      'other = \'sounds = "x" # y\'',
      '',
      '[audio]',
      'sounds = true # sounds = false',
      '',
    ].join('\n');
    const out = patchAndCheck(source, [
      { path: ['audio', 'sounds'], value: false },
      { path: ['prompts', 'clean', 'text'], value: 'new' },
    ]);
    expect(out).toBe(
      source
        .replace('sounds = true # sounds', 'sounds = false # sounds')
        .replace('"use [brackets] # not a comment = here"', '"new"'),
    );
  });
});

describe('patchToml: identical values', () => {
  function leafPatches(value: unknown, path: ConfigPath, out: ConfigPatch[]): ConfigPatch[] {
    out.push({ path, value });
    if (Array.isArray(value)) value.forEach((item, i) => leafPatches(item, [...path, i], out));
    else if (isTable(value)) {
      for (const [key, child] of Object.entries(value)) leafPatches(child, [...path, key], out);
    }
    return out;
  }

  it('leaves the text byte-for-byte unchanged for every path in the example', () => {
    const root = parse(EXAMPLE) as Table;
    const patches = Object.entries(root).flatMap(([key, value]) => leafPatches(value, [key], []));
    expect(patches.length).toBeGreaterThan(70);
    for (const patch of patches) expect(patchToml(EXAMPLE, [patch])).toBe(EXAMPLE);
    expect(patchToml(EXAMPLE, patches)).toBe(EXAMPLE);
  });

  it('leaves odd formatting alone when nothing changes', () => {
    const source =
      "a   =   'x'   # c\r\n[t]\r\n  n = 0x10\r\n  f = 1_000.5\r\n  arr = [ 1,\r\n    2 ]\r\n";
    expect(
      patchToml(source, [
        { path: ['a'], value: 'x' },
        { path: ['t', 'n'], value: 16 },
        { path: ['t', 'f'], value: 1000.5 },
        { path: ['t', 'arr'], value: [1, 2] },
        { path: ['t'], value: { n: 16, f: 1000.5, arr: [1, 2] } },
      ]),
    ).toBe(source);
  });

  it('treats removing a missing key and emptying a missing list as no-ops', () => {
    expect(
      patchToml(EXAMPLE, [
        { path: ['general', 'nope'], value: undefined },
        { path: ['providers', 'nobody'], value: undefined },
        { path: ['profiles', 7], value: undefined },
        { path: ['local_models'], value: [] },
      ]),
    ).toBe(EXAMPLE);
  });
});

describe('patchToml: new keys', () => {
  it('appends to the end of the table body, before the blank line', () => {
    const out = patchAndCheck(EXAMPLE, [{ path: ['general', 'debug_logging'], value: true }]);
    expect(out).toBe(
      EXAMPLE.replace(
        'language = "en"               # v1 supports English only\n\n[hotkeys]',
        'language = "en"               # v1 supports English only\ndebug_logging = true\n\n[hotkeys]',
      ),
    );
  });

  it('appends to the last table without a trailing newline', () => {
    const out = patchAndCheck('[audio]\nsounds = true', [
      { path: ['audio', 'keep_mic_warm'], value: true },
    ]);
    expect(out).toBe('[audio]\nsounds = true\nkeep_mic_warm = true\n');
  });

  it('appends to an empty table', () => {
    const out = patchAndCheck('[audio]\n\n[model]\nactive = "x"\n', [
      { path: ['audio', 'sounds'], value: false },
    ]);
    expect(out).toBe('[audio]\nsounds = false\n\n[model]\nactive = "x"\n');
  });

  it('keeps comments that follow the last key where they are', () => {
    const source =
      '[audio]\nsounds = true\n# max_recording_seconds = 300\n\n# about model\n[model]\n';
    const out = patchAndCheck(source, [{ path: ['audio', 'keep_mic_warm'], value: true }]);
    expect(out).toBe(
      '[audio]\nsounds = true\nkeep_mic_warm = true\n# max_recording_seconds = 300\n\n# about model\n[model]\n',
    );
  });

  it('creates a missing table at the end of the file', () => {
    const source = '# header\nversion = 1\n\n[audio]\nsounds = true\n';
    const out = patchAndCheck(source, [
      { path: ['overlay', 'style'], value: 'minimal' },
      { path: ['overlay', 'show_waveform'], value: false },
    ]);
    expect(out).toBe(
      '# header\nversion = 1\n\n[audio]\nsounds = true\n\n[overlay]\nstyle = "minimal"\nshow_waveform = false\n',
    );
  });

  it('creates a table in an empty file and in a comment-only file', () => {
    expect(patchAndCheck('', [{ path: ['audio', 'sounds'], value: false }])).toBe(
      '[audio]\nsounds = false\n',
    );
    expect(patchAndCheck('# just a comment', [{ path: ['audio', 'sounds'], value: false }])).toBe(
      '# just a comment\n\n[audio]\nsounds = false\n',
    );
  });

  it('puts a new sub-table next to its siblings', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['providers', 'openai', 'api_key'], value: 'secret:openai' },
      { path: ['prompts', 'email', 'text'], value: 'Be polite.' },
    ]);
    expect(out).toContain(
      'api_key = "secret:custom-lan"\n\n[providers.openai]\napi_key = "secret:openai"\n\n[cleanup]',
    );
    expect(out).toContain('tone."\n\n[prompts.email]\ntext = "Be polite."\n\n[insert]');
    expectCommentsKept(EXAMPLE, out);
  });

  it('adds a top-level key before the first table', () => {
    expect(patchAndCheck('[audio]\nsounds = true\n', [{ path: ['version'], value: 1 }])).toBe(
      'version = 1\n\n[audio]\nsounds = true\n',
    );
    expect(
      patchAndCheck('# Flow settings\n# more\n\n[audio]\nsounds = true\n', [
        { path: ['version'], value: 1 },
      ]),
    ).toBe('# Flow settings\n# more\n\nversion = 1\n\n[audio]\nsounds = true\n');
    expect(patchAndCheck('a = 1\n\n[audio]\n', [{ path: ['version'], value: 1 }])).toBe(
      'a = 1\nversion = 1\n\n[audio]\n',
    );
  });

  it('adds nested profile overrides as dotted keys', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['profiles', 0, 'cleanup', 'filler_removal'], value: false },
      { path: ['profiles', 1, 'insert', 'method'], value: 'type' },
      { path: ['profiles', 1, 'enabled'], value: false },
    ]);
    expect(out).toContain(
      'cleanup.llm_enabled = false\ncleanup.filler_removal = false\n\n[[profiles]]\nname = "Slack"',
    );
    expect(
      out.endsWith('cleanup.llm_prompt = "casual"\ninsert.method = "type"\nenabled = false\n'),
    ).toBe(true);
  });

  it('uses an existing sub-table header of a list item', () => {
    const source = [
      '[[profiles]]',
      'name = "A"',
      '[profiles.cleanup]',
      'llm_enabled = true  # keep',
      '',
      '[[profiles]]',
      'name = "B"',
      '',
    ].join('\n');
    const out = patchAndCheck(source, [
      { path: ['profiles', 0, 'cleanup', 'llm_prompt'], value: 'email' },
      { path: ['profiles', 0, 'cleanup', 'llm_enabled'], value: false },
      { path: ['profiles', 0, 'insert', 'method'], value: 'type' },
      { path: ['profiles', 1, 'cleanup', 'llm_prompt'], value: 'casual' },
    ]);
    expect(out).toBe(
      [
        '[[profiles]]',
        'name = "A"',
        'insert.method = "type"',
        '[profiles.cleanup]',
        'llm_enabled = false  # keep',
        'llm_prompt = "email"',
        '',
        '[[profiles]]',
        'name = "B"',
        'cleanup.llm_prompt = "casual"',
        '',
      ].join('\n'),
    );
  });

  it('adds an entry to an inline table', () => {
    const source = 'insert = { method = "paste" } # c\nempty = {}\n';
    const out = patchAndCheck(source, [
      { path: ['insert', 'trailing_space'], value: false },
      { path: ['empty', 'a'], value: 1 },
    ]);
    expect(out).toBe(
      'insert = { method = "paste", trailing_space = false } # c\nempty = { a = 1 }\n',
    );
  });

  it('extends a table spelled with dotted keys using dotted keys', () => {
    const source = 'audio.sounds = true\n\n[model]\nactive = "x"\n';
    const out = patchAndCheck(source, [{ path: ['audio', 'keep_mic_warm'], value: true }]);
    expect(out).toBe('audio.sounds = true\naudio.keep_mic_warm = true\n\n[model]\nactive = "x"\n');
  });

  it('creates a header when a dotted key would clash with a deeper header', () => {
    const source = '[a.b.x]\nq = 1\n\n[a]\nk = 1\n';
    const out = patchAndCheck(source, [{ path: ['a', 'b', 'c'], value: 2 }]);
    expect(parse(out)).toEqual({ a: { k: 1, b: { c: 2, x: { q: 1 } } } });
    expect(out).toContain('[a.b]\nc = 2\n');
  });

  it('keeps a new sub-table header inside the list item it belongs to', () => {
    const source =
      '[[profiles]]\nname = "A"\n[profiles.cleanup.deep]\nx = 1\n\n[[profiles]]\nname = "B"\n';
    const out = patchAndCheck(source, [
      { path: ['profiles', 0, 'cleanup', 'llm_enabled'], value: true },
    ]);
    expect(out).toBe(
      '[[profiles]]\nname = "A"\n[profiles.cleanup.deep]\nx = 1\n\n[profiles.cleanup]\nllm_enabled = true\n\n[[profiles]]\nname = "B"\n',
    );
  });

  it('keeps CRLF line endings on the lines it adds', () => {
    const source = EXAMPLE.replace(/\n/g, '\r\n');
    const out = patchAndCheck(source, [
      { path: ['general', 'debug_logging'], value: true },
      { path: ['providers', 'openai', 'api_key'], value: 'secret:openai' },
      { path: ['snippets'], value: [{ trigger: 'a', text: 'one\ntwo' }] },
      { path: ['audio', 'sounds'], value: undefined },
    ]);
    expect(out.replace(/\r\n/g, '')).not.toMatch(/[\r\n]/);
    expect(out).toContain('debug_logging = true\r\n');
    expect(out).toContain('text = "one\\ntwo"\r\n');
    expectCommentsKept(source, out);
  });
});

describe('patchToml: removing', () => {
  it('removes just the key line', () => {
    const out = patchAndCheck(EXAMPLE, [{ path: ['audio', 'sounds'], value: undefined }]);
    expect(out).toBe(EXAMPLE.replace('sounds = true\n', ''));
  });

  it('keeps the table header when its last key is removed', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['providers', 'groq', 'api_key'], value: undefined },
    ]);
    expect(out).toBe(
      EXAMPLE.replace('[providers.groq]\napi_key = "secret:groq"\n', '[providers.groq]\n'),
    );
  });

  it('treats null like undefined', () => {
    const out = patchToml(EXAMPLE, [{ path: ['audio', 'sounds'], value: null }]);
    expect(out).toBe(EXAMPLE.replace('sounds = true\n', ''));
  });

  it('removes a dotted key and a whole dotted table from a list item', () => {
    const patches = [
      { path: ['profiles', 0, 'insert', 'paste_shortcut'], value: undefined },
      { path: ['profiles', 1, 'cleanup'], value: undefined },
    ];
    const out = patchToml(EXAMPLE, patches);
    // A table spelled only with dotted keys goes when its last key goes.
    expect(prune(parse(out))).toEqual(prune(applyAll(EXAMPLE, patches)));
    expect(out).not.toContain('insert.paste_shortcut');
    expect(
      out.endsWith('match_process = ["slack.exe"]\nmodel.active = "groq/whisper-large-v3-turbo"\n'),
    ).toBe(true);
  });

  it('removes a multi-line value completely', () => {
    const source = '[t]\na = [\n  1,\n  2,\n]  # gone\nb = 2\n';
    expect(patchAndCheck(source, [{ path: ['t', 'a'], value: undefined }])).toBe('[t]\nb = 2\n');
  });

  it('removes entries of an inline table and elements of an array', () => {
    const source = 't = { a = 1, b = 2, c = 3 }\narr = ["x", "y", "z"]\none = { a = 1 }\n';
    expect(patchAndCheck(source, [{ path: ['t', 'a'], value: undefined }])).toContain(
      't = { b = 2, c = 3 }',
    );
    expect(patchAndCheck(source, [{ path: ['t', 'c'], value: undefined }])).toContain(
      't = { a = 1, b = 2 }',
    );
    expect(patchAndCheck(source, [{ path: ['arr', 1], value: undefined }])).toContain(
      'arr = ["x", "z"]',
    );
    expect(patchAndCheck(source, [{ path: ['arr', 2], value: undefined }])).toContain(
      'arr = ["x", "y"]',
    );
    expect(patchToml(source, [{ path: ['one', 'a'], value: undefined }])).toContain('one = {}');
  });

  it('removes a whole table block and the blank line after it', () => {
    const out = patchAndCheck(EXAMPLE, [{ path: ['providers', 'custom-lan'], value: undefined }]);
    expect(out).toBe(
      EXAMPLE.replace(
        '[providers.custom-lan]\nbase_url = "http://192.168.1.20:8000/v1"\napi_key = "secret:custom-lan"\n\n',
        '',
      ),
    );
  });

  it('removes a parent table with all its sub-tables', () => {
    const out = patchToml(EXAMPLE, [{ path: ['providers'], value: undefined }]);
    expect(out).toBe(
      EXAMPLE.replace(
        '[providers.groq]\napi_key = "secret:groq"\n\n[providers.custom-lan]\nbase_url = "http://192.168.1.20:8000/v1"\napi_key = "secret:custom-lan"\n\n',
        '',
      ),
    );
  });

  it('leaves a comment that introduces the next table', () => {
    const source = '[a]\nx = 1\n\n# about b\n[b]\ny = 2  # gone\n# about c\n[c]\nz = 3\n';
    expect(patchAndCheck(source, [{ path: ['b'], value: undefined }])).toBe(
      '[a]\nx = 1\n\n# about b\n# about c\n[c]\nz = 3\n',
    );
  });

  it('does not leave blank lines at the end of the file', () => {
    const out = patchAndCheck(EXAMPLE, [{ path: ['profiles', 1], value: undefined }]);
    expect(out.endsWith('cleanup.llm_enabled = false\n')).toBe(true);
  });
});

describe('patchToml: lists of tables', () => {
  it('replaces every block at the position of the first one', () => {
    const dictionary = [
      { heard: ['pie torch'], write: 'PyTorch' },
      { heard: ['tory', 'towery'], write: 'Tauri' },
      { heard: ['j son'], write: 'JSON' },
    ];
    const out = patchAndCheck(EXAMPLE, [{ path: ['dictionary'], value: dictionary }]);
    expect(out).toBe(
      EXAMPLE.replace(
        '[[dictionary]]\nheard = ["tory", "towery"]\nwrite = "Tauri"\n',
        '[[dictionary]]\nheard = ["pie torch"]\nwrite = "PyTorch"\n\n' +
          '[[dictionary]]\nheard = ["tory", "towery"]\nwrite = "Tauri"\n\n' +
          '[[dictionary]]\nheard = ["j son"]\nwrite = "JSON"\n',
      ),
    );
  });

  it('serialises profile overrides as dotted keys', () => {
    const profiles = [
      {
        name: 'Editors',
        match_process: ['code.exe'],
        match_title: [],
        enabled: true,
        cleanup: { llm_prompt: 'code_comment', filler_removal: false },
        insert: { method: 'type' },
        model: {},
      },
      { name: 'Games', match_title: ['/^Steam/i'], enabled: false },
    ];
    const out = patchAndCheck(EXAMPLE, [{ path: ['profiles'], value: profiles }]);
    expect(out.slice(out.indexOf('[[profiles]]'))).toBe(
      [
        '[[profiles]]',
        'name = "Editors"',
        'match_process = ["code.exe"]',
        'match_title = []',
        'enabled = true',
        'cleanup.llm_prompt = "code_comment"',
        'cleanup.filler_removal = false',
        'insert.method = "type"',
        'model = {}',
        '',
        '[[profiles]]',
        'name = "Games"',
        'match_title = ["/^Steam/i"]',
        'enabled = false',
        '',
      ].join('\n'),
    );
    expect(out.slice(0, out.indexOf('[[profiles]]'))).toBe(
      EXAMPLE.slice(0, EXAMPLE.indexOf('[[profiles]]')),
    );
  });

  it('appends a list that does not exist yet to the end of the file', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['local_models'], value: [{ id: 'mine', path: 'C:\\models\\mine.gguf' }] },
    ]);
    expect(out).toBe(
      `${EXAMPLE}\n[[local_models]]\nid = "mine"\npath = "C:\\\\models\\\\mine.gguf"\n`,
    );
  });

  it('removes the blocks when the list is emptied', () => {
    const out = patchToml(EXAMPLE, [
      { path: ['dictionary'], value: [] },
      { path: ['profiles'], value: [] },
    ]);
    expect(out).toBe(
      EXAMPLE.slice(0, EXAMPLE.indexOf('[[dictionary]]')) +
        '[[snippets]]\ntrigger = "my address"\ntext = "221B Baker Street, London"\n',
    );
    const parsed = parse(out);
    expect(parsed.dictionary).toBeUndefined();
    expect(validateConfig(parsed).config.dictionary).toEqual([]);
  });

  it('loses comments inside replaced blocks but keeps those around them', () => {
    const source = [
      '# top KEEP',
      '[history]',
      'enabled = false  # KEEP',
      '',
      '# before the dictionary KEEP',
      '[[dictionary]]',
      '# inside, lost',
      'heard = ["a"]  # lost too',
      'write = "A"',
      '',
      '[[dictionary]]',
      'heard = ["b"]',
      'write = "B"',
      '# after the dictionary KEEP',
      '',
      '[[snippets]]',
      'trigger = "t"  # KEEP',
      'text = "T"',
      '',
    ].join('\n');
    const out = patchAndCheck(source, [
      { path: ['dictionary'], value: [{ heard: ['c'], write: 'C' }] },
    ]);
    expect(out).toBe(
      [
        '# top KEEP',
        '[history]',
        'enabled = false  # KEEP',
        '',
        '# before the dictionary KEEP',
        '[[dictionary]]',
        'heard = ["c"]',
        'write = "C"',
        '',
        '# after the dictionary KEEP',
        '',
        '[[snippets]]',
        'trigger = "t"  # KEEP',
        'text = "T"',
        '',
      ].join('\n'),
    );
  });

  it('replaces blocks together with their sub-table headers', () => {
    const source =
      '[[profiles]]\nname = "A"\n[profiles.cleanup]\nllm_enabled = true\n\n[[profiles]]\nname = "B"\n\n[audio]\nsounds = true\n';
    const out = patchAndCheck(source, [{ path: ['profiles'], value: [{ name: 'C' }] }]);
    expect(out).toBe('[[profiles]]\nname = "C"\n\n[audio]\nsounds = true\n');
  });

  it('rewrites a list written as an inline array in place', () => {
    const source =
      'dictionary = [{ heard = ["a"], write = "A" }]  # inline\n\n[audio]\nsounds = true\n';
    const out = patchAndCheck(source, [
      { path: ['dictionary'], value: [{ heard: ['b'], write: 'B' }] },
      { path: ['dictionary', 0, 'write'], value: 'BB' },
      { path: ['dictionary', 1], value: { heard: ['c'], write: 'C' } },
    ]);
    expect(out).toBe(
      'dictionary = [{ heard = ["b"], write = "BB" }, { heard = ["c"], write = "C" }]  # inline\n\n[audio]\nsounds = true\n',
    );
  });

  it('appends one item after the existing blocks', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['dictionary', 1], value: { heard: ['j son'], write: 'JSON' } },
      { path: ['local_models', 0], value: { id: 'm', path: 'm.gguf' } },
    ]);
    expect(out).toContain(
      'write = "Tauri"\n\n[[dictionary]]\nheard = ["j son"]\nwrite = "JSON"\n\n[[snippets]]',
    );
    expect(out.endsWith('\n\n[[local_models]]\nid = "m"\npath = "m.gguf"\n')).toBe(true);
    expectCommentsKept(EXAMPLE, out);
  });

  it('removes one item and keeps the others, comments included', () => {
    const source = EXAMPLE.replace('name = "Slack"', 'name = "Slack"  # chat');
    const out = patchAndCheck(source, [{ path: ['profiles', 0], value: undefined }]);
    expect(out).toBe(
      source.replace(
        '[[profiles]]\nname = "Terminals"\nmatch_process = ["WindowsTerminal.exe", "wezterm-gui.exe"]\ninsert.paste_shortcut = "Ctrl+Shift+V"\ncleanup.llm_enabled = false\n\n',
        '',
      ),
    );
  });

  it('replaces one item key by key, keeping its comments', () => {
    const source =
      '[[snippets]]\n# the address\ntrigger = "my address"  # spoken\ntext = "old"\nextra = 1\n';
    const out = patchAndCheck(source, [
      { path: ['snippets', 0], value: { trigger: 'my address', text: 'new' } },
    ]);
    expect(out).toBe(
      '[[snippets]]\n# the address\ntrigger = "my address"  # spoken\ntext = "new"\n',
    );
  });

  it('rejects an item index past the end of the list', () => {
    expect(() => patchToml(EXAMPLE, [{ path: ['profiles', 5, 'name'], value: 'x' }])).toThrow(
      TomlPatchError,
    );
    expect(() => patchToml(EXAMPLE, [{ path: ['profiles', 3], value: { name: 'x' } }])).toThrow(
      TomlPatchError,
    );
    expect(() => patchToml(EXAMPLE, [{ path: ['audio', 0], value: 1 }])).toThrow(TomlPatchError);
    expect(() => patchToml(EXAMPLE, [{ path: [], value: 1 }])).toThrow(TomlPatchError);
  });
});

describe('patchToml: whole tables', () => {
  it('creates a table from an object', () => {
    const out = patchAndCheck(EXAMPLE, [
      {
        path: ['providers', 'office'],
        value: { base_url: 'http://10.0.0.2/v1', api_key: 'secret:office', model: 'whisper-1' },
      },
      { path: ['prompts', 'mine'], value: { text: 'Shout.' } },
    ]);
    expect(out).toContain(
      '\n[providers.office]\nbase_url = "http://10.0.0.2/v1"\napi_key = "secret:office"\nmodel = "whisper-1"\n\n[cleanup]',
    );
    expect(out).toContain('\n[prompts.mine]\ntext = "Shout."\n\n[insert]');
    expectCommentsKept(EXAMPLE, out);
  });

  it('replaces the keys of an existing table, keeping comments on kept keys', () => {
    const source = EXAMPLE.replace(
      'base_url = "http://192.168.1.20:8000/v1"',
      'base_url = "http://192.168.1.20:8000/v1"  # the NAS',
    );
    const out = patchAndCheck(source, [
      {
        path: ['providers', 'custom-lan'],
        value: { base_url: 'http://nas:9000/v1', model: 'large' },
      },
    ]);
    expect(out).toContain(
      '[providers.custom-lan]\nbase_url = "http://nas:9000/v1"  # the NAS\nmodel = "large"\n\n[cleanup]',
    );
  });

  it('creates an empty table', () => {
    const out = patchToml('[audio]\nsounds = true\n', [{ path: ['providers', 'x'], value: {} }]);
    expect(out).toBe('[audio]\nsounds = true\n\n[providers.x]\n');
    expect(parse(out)).toEqual({ audio: { sounds: true }, providers: { x: {} } });
  });

  it('empties a table without removing it', () => {
    const out = patchToml(EXAMPLE, [{ path: ['providers', 'groq'], value: {} }]);
    expect((parse(out).providers as Table).groq).toEqual({});
    expect(out).toContain('[providers.groq]\n\n[providers.custom-lan]');
  });

  it('sets a whole override section of a profile as dotted keys', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['profiles', 1, 'cleanup'], value: { llm_enabled: true, filler_removal: false } },
      { path: ['profiles', 1, 'overlay'], value: { style: 'none' } },
    ]);
    expect(
      out.endsWith(
        'model.active = "groq/whisper-large-v3-turbo"\ncleanup.llm_enabled = true\ncleanup.filler_removal = false\noverlay.style = "none"\n',
      ),
    ).toBe(true);
  });

  it('turns a scalar into a table and a table into a scalar', () => {
    expect(patchAndCheck('[t]\na = 1  # c\n', [{ path: ['t', 'a'], value: { b: 2 } }])).toBe(
      '[t]\na = { b = 2 }  # c\n',
    );
    expect(patchAndCheck('[t]\na = 1  # c\n', [{ path: ['t', 'a', 'b'], value: 2 }])).toBe(
      '[t]\na = { b = 2 }  # c\n',
    );
    const out = patchAndCheck(EXAMPLE, [{ path: ['providers', 'groq'], value: 'nope' }]);
    expect(parse(out).providers).toMatchObject({ groq: 'nope' });
    expectCommentsKept(EXAMPLE, out);
  });
});

describe('patchToml: strings', () => {
  const tricky = [
    '',
    'plain',
    'He said "hi"',
    'back\\slash and C:\\Users\\me',
    'tab\there',
    "single 'quotes'",
    'unicode: café – 日本語 – 😀',
    'control: \u0000\u0007\u001f\u007f',
    '# not a comment',
    'ends with quote"',
    '"""',
    'a = "b" [c] {d}',
    ' leading and trailing ',
  ];
  const multiline = [
    'two\nlines',
    'trailing newline\n',
    '\nleading newline',
    'windows\r\nline endings\r\n',
    'quotes """ inside\nand "" here\nand at the end"',
    'backslash at end of line \\\nnext',
    'Dear team,\n\n  Indented "quote".\n\tTabbed.\n\nBest,\nA',
    '"',
    '\n',
  ];

  it('round-trips tricky single-line strings', () => {
    for (const text of tricky) {
      const out = patchAndCheck(EXAMPLE, [
        { path: ['prompts', 'clean', 'text'], value: text },
        { path: ['snippets', 0, 'text'], value: text },
        { path: ['dictionary', 0, 'heard'], value: [text, 'x'] },
      ]);
      expect(lines(out)).toHaveLength(lines(EXAMPLE).length);
    }
  });

  it('writes text with line breaks as a readable multi-line string', () => {
    const out = patchAndCheck(EXAMPLE, [
      { path: ['prompts', 'clean', 'text'], value: 'Fix grammar.\nKeep the "tone".\n' },
    ]);
    expect(out).toContain(
      '[prompts.clean]\ntext = """\nFix grammar.\nKeep the "tone".\n"""\n\n[insert]',
    );
  });

  it('round-trips multi-line strings, new and replaced, in tables and blocks', () => {
    for (const text of multiline) {
      patchAndCheck(EXAMPLE, [
        { path: ['prompts', 'clean', 'text'], value: text },
        { path: ['prompts', 'new', 'text'], value: text },
        { path: ['snippets'], value: [{ trigger: 'x', text }] },
        { path: ['snippets', 1], value: { trigger: 'y', text } },
        { path: ['profiles', 0, 'cleanup', 'note'], value: text },
        { path: ['dictionary', 0, 'heard'], value: [text] },
      ]);
    }
  });

  it('replaces a multi-line string with another value cleanly', () => {
    const source =
      '[prompts.a]\ntext = """\nline 1\nline 2 # not a comment\n"""  # real comment\nx = 1\n';
    expect(patchAndCheck(source, [{ path: ['prompts', 'a', 'text'], value: 'short' }])).toBe(
      '[prompts.a]\ntext = "short"  # real comment\nx = 1\n',
    );
    expect(patchAndCheck(source, [{ path: ['prompts', 'a', 'x'], value: 2 }])).toBe(
      source.replace('x = 1', 'x = 2'),
    );
    expect(patchAndCheck(source, [{ path: ['prompts', 'a', 'text'], value: undefined }])).toBe(
      '[prompts.a]\nx = 1\n',
    );
  });

  it('uses escapes instead of real line breaks in a CRLF file and inside inline values', () => {
    const crlf = '[prompts.a]\r\ntext = "x"\r\n';
    const out = patchAndCheck(crlf, [{ path: ['prompts', 'a', 'text'], value: 'one\ntwo' }]);
    expect(out).toBe('[prompts.a]\r\ntext = "one\\ntwo"\r\n');
    const inline = patchAndCheck('t = { a = "x" }\n', [{ path: ['t', 'a'], value: 'one\ntwo' }]);
    expect(inline).toBe('t = { a = "one\\ntwo" }\n');
  });

  it('writes numbers so they read back as the same number', () => {
    for (const value of [0, -1, 42, 1.5, -0.25, 1e21, 1e-7, 2 ** 53, Infinity, -Infinity]) {
      patchAndCheck(EXAMPLE, [{ path: ['model', 'keep_loaded_minutes'], value }]);
    }
    const out = patchToml(EXAMPLE, [{ path: ['model', 'keep_loaded_minutes'], value: NaN }]);
    expect((parse(out).model as Table).keep_loaded_minutes).toBeNaN();
  });
});

describe('patchToml: errors', () => {
  it('throws on a file that is not valid TOML', () => {
    expect(() => patchToml('a = = 1\n', [{ path: ['a'], value: 2 }])).toThrow(/Invalid TOML/);
  });
});

// A small deterministic generator, so a failure can be reproduced from its seed.
function mulberry32(seed: number): () => number {
  let a = seed;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** A hand-written file with the layouts the example does not use. */
const MESSY = `# Flow settings KEEP
# second line KEEP
version = 1   # KEEP version
insert.method = "paste" # KEEP dotted
insert.trailing_space = true

[general]   # KEEP on header
  theme = 'dark'   # KEEP literal
  launch_at_login = false

# KEEP about audio
[audio]
sounds = true
input_device = """
Microphone (USB)"""
# KEEP commented = "option"

[providers]   # KEEP providers parent
groq = { api_key = "secret:groq" }   # KEEP inline
[providers.local]
base_url = "http://localhost:11434/v1"   # KEEP url

[cleanup]
llm_enabled = true   # KEEP llm

[prompts."my prompt"]   # KEEP quoted
text = "Be brief."

[overlay]
style = "pill"   # KEEP style


# KEEP before lists
[[dictionary]]
heard = [
  "tory",
  "towery",
]
write = "Tauri"

[[profiles]]
name = "Terminals"
match_process = ["wt.exe"]
[profiles.insert]
paste_shortcut = "Ctrl+Shift+V"

# KEEP between profiles
[[profiles]]
name = "Chat"
match_title = ["/slack|discord/i"]
cleanup = { llm_enabled = true }

[history]   # KEEP history
enabled = true
`;

describe('patchToml: property', () => {
  const STRINGS = [
    'x',
    '',
    'with "quotes"',
    'C:\\path\\file',
    'multi\nline\ntext',
    'ends\n',
    'tab\tand # hash',
    'ünïcödé 😀',
    '[not.a.header]',
    'a = 1',
    '"""',
  ];
  const NEW_KEYS = ['extra', 'another_one', 'key with space', 'dotted.name', 'ключ', 'x-1'];
  const SECTIONS = [
    'general',
    'hotkeys',
    'audio',
    'model',
    'cleanup',
    'insert',
    'overlay',
    'history',
  ];
  const OVERRIDE_SECTIONS = ['audio', 'model', 'cleanup', 'insert', 'overlay', 'history'];
  const LISTS = ['dictionary', 'snippets', 'profiles', 'local_models'];

  function generator(random: () => number) {
    const int = (n: number) => Math.floor(random() * n);
    const pick = <T>(items: readonly T[]): T => items[int(items.length)]!;
    const scalar = (): unknown =>
      pick<() => unknown>([
        () => pick(STRINGS),
        () => pick(STRINGS),
        () => int(1000) - 500,
        () => (int(100000) - 50000) / 128,
        () => random() < 0.5,
      ])();
    const leaf = (): unknown =>
      random() < 0.8 ? scalar() : Array.from({ length: int(4) }, () => pick(STRINGS));
    const keyOf = (table: unknown): string => {
      const existing = isTable(table) ? Object.keys(table) : [];
      return existing.length > 0 && random() < 0.6 ? pick(existing) : pick(NEW_KEYS);
    };
    const item = (name: string): Table => {
      if (name === 'dictionary') return { heard: [pick(STRINGS), 'b'], write: pick(STRINGS) };
      if (name === 'snippets') return { trigger: pick(STRINGS), text: pick(STRINGS) };
      if (name === 'local_models') return { id: pick(STRINGS), path: pick(STRINGS) };
      const profile: Table = { name: pick(STRINGS), match_process: [pick(STRINGS)] };
      if (random() < 0.5) profile.enabled = random() < 0.5;
      for (const section of OVERRIDE_SECTIONS) {
        if (random() < 0.3)
          profile[section] = { [pick(NEW_KEYS)]: scalar(), llm_prompt: pick(STRINGS) };
      }
      return profile;
    };

    /** Lines with a KEEP comment must not be deleted, so those keys are never removed. */
    return (state: Table, commented: Set<string>): ConfigPatch => {
      const removable = (path: ConfigPath) => !commented.has(JSON.stringify(path));
      for (;;) {
        const roll = random();
        if (roll < 0.35) {
          const section = pick(SECTIONS);
          return { path: [section, keyOf(state[section])], value: leaf() };
        }
        if (roll < 0.45) {
          const section = pick(SECTIONS);
          const path = [section, keyOf(state[section])];
          if (removable(path)) return { path, value: undefined };
        } else if (roll < 0.55) {
          const group = pick(['providers', 'prompts']);
          const name = keyOf(state[group]);
          const table = (state[group] as Table | undefined)?.[name];
          const path = [group, name, isTable(table) || table === undefined ? keyOf(table) : 'k'];
          if (table !== undefined && !isTable(table)) continue;
          if (random() < 0.8) return { path, value: scalar() };
          if (removable(path)) return { path, value: undefined };
        } else if (roll < 0.62) {
          const group = pick(['providers', 'prompts']);
          const path = [group, keyOf(state[group])];
          if (!removable(path)) continue;
          const table: Table = {};
          for (let i = int(4); i > 0; i--) table[pick(NEW_KEYS)] = scalar();
          if (Object.keys(table).some((key) => !removable([...path, key]))) continue;
          const current = (state[group] as Table | undefined)?.[path[1] as string];
          if (isTable(current) && Object.keys(current).some((key) => !removable([...path, key]))) {
            continue;
          }
          return { path, value: random() < 0.75 ? table : undefined };
        } else if (roll < 0.8) {
          const profiles = (state.profiles as Table[] | undefined) ?? [];
          if (profiles.length === 0) continue;
          const index = int(profiles.length);
          const profile = profiles[index]!;
          if (random() < 0.3) {
            const key = pick(['name', 'enabled', 'match_process', 'match_title']);
            const value =
              key === 'enabled' ? random() < 0.5 : key === 'name' ? pick(STRINGS) : [pick(STRINGS)];
            return { path: ['profiles', index, key], value };
          }
          const section = pick(OVERRIDE_SECTIONS);
          const current = profile[section];
          if (current !== undefined && !isTable(current)) continue;
          const path = ['profiles', index, section, keyOf(current)];
          return { path, value: random() < 0.8 ? scalar() : undefined };
        } else if (roll < 0.88) {
          const name = pick(LISTS);
          return { path: [name], value: Array.from({ length: int(4) }, () => item(name)) };
        } else if (roll < 0.94) {
          const name = pick(LISTS);
          const length = ((state[name] as unknown[] | undefined) ?? []).length;
          return { path: [name, length], value: item(name) };
        } else {
          const name = pick(LISTS);
          const length = ((state[name] as unknown[] | undefined) ?? []).length;
          if (length === 0) continue;
          const index = int(length);
          return random() < 0.5
            ? { path: [name, index], value: undefined }
            : { path: [name, index], value: item(name) };
        }
      }
    };
  }

  /** Paths of keys whose line carries a comment that has to survive. */
  function commentedPaths(source: string, marker: string): Set<string> {
    const out = new Set<string>();
    let table: string[] = [];
    for (const line of source.split(/\r?\n/)) {
      const header = /^\s*\[([^[\]]+)\]/.exec(line);
      if (header) table = header[1]!.split('.').map((part) => part.replace(/"/g, ''));
      const key = /^\s*([\w.-]+)\s*=/.exec(line);
      if (key && line.includes(marker)) {
        const path = [...table, ...key[1]!.split('.')];
        // The key, and every table above it, must stay.
        for (let i = 1; i <= path.length; i++) out.add(JSON.stringify(path.slice(0, i)));
      }
      if (header && line.includes(marker)) {
        for (let i = 1; i <= table.length; i++) out.add(JSON.stringify(table.slice(0, i)));
      }
    }
    return out;
  }

  function run(source: string, seed: number, steps: number, marker: string): void {
    const next = generator(mulberry32(seed));
    const commented = commentedPaths(source, marker);
    let text = source;
    const state = parse(source) as Table;
    const applied: ConfigPatch[] = [];
    for (let step = 0; step < steps; step++) {
      const patch = next(state, commented);
      applied.push(patch);
      const context = `seed ${seed}, step ${step}: ${JSON.stringify(applied.slice(-3))}`;
      let out: string;
      try {
        out = patchToml(text, [patch]);
      } catch (error) {
        throw new Error(`${context}\n${String(error)}\n--- text ---\n${text}`);
      }
      applyToObject(state, patch);
      expect(prune(parse(out)), context).toEqual(prune(state));
      // Applying the same patch again must change nothing (list indexes shift, so skip those).
      if (typeof patch.path[patch.path.length - 1] !== 'number') {
        expect(patchToml(out, [patch]), context).toBe(out);
      }
      const kept = comments(out, marker);
      for (const comment of comments(source, marker)) expect(kept, context).toContain(comment);
      text = out;
    }
    // What the app would load is the same either way.
    expect(validateConfig(prune(parse(text))).config).toEqual(validateConfig(prune(state)).config);
  }

  it('re-parses to "old parse + patches" and keeps every comment (spec example)', () => {
    expect(comments(EXAMPLE).length).toBe(9);
    for (let seed = 1; seed <= 80; seed++) run(EXAMPLE, seed, 40, '#');
  });

  it('holds for a CRLF copy of the example', () => {
    const crlf = EXAMPLE.replace(/\n/g, '\r\n');
    for (let seed = 100; seed < 130; seed++) run(crlf, seed, 40, '#');
  });

  it('holds for a hand-formatted file with inline tables, sub-headers and odd spacing', () => {
    expect(comments(MESSY, 'KEEP').length).toBe(17);
    for (let seed = 200; seed < 280; seed++) run(MESSY, seed, 40, 'KEEP');
  });

  it('holds when starting from an empty or comment-only file', () => {
    for (let seed = 300; seed < 310; seed++) run('', seed, 40, '#');
    // The comment doubles as a canary: a full rewrite of the file would drop it.
    for (let seed = 310; seed < 340; seed++) run('# only a comment KEEP', seed, 40, 'KEEP');
  });

  it('applies a varied batch of patches to the example in one call', () => {
    const patches: ConfigPatch[] = [
      { path: ['audio', 'sounds'], value: false },
      { path: ['general', 'theme'], value: 'light' },
      { path: ['general', 'onboarded'], value: true },
      { path: ['hotkeys', 'hold_to_talk'], value: 'RightCtrl' },
      { path: ['model', 'active'], value: 'groq/whisper-large-v3-turbo' },
      { path: ['model', 'keep_loaded_minutes'], value: 0 },
      { path: ['providers', 'groq', 'api_key'], value: 'secret:groq' },
      { path: ['providers', 'openai', 'api_key'], value: 'secret:openai' },
      { path: ['providers', 'custom-lan'], value: { base_url: 'http://nas/v1', model: 'm' } },
      { path: ['prompts', 'email', 'text'], value: 'Rewrite as an email.\nKeep every "fact".' },
      { path: ['prompts', 'clean'], value: undefined },
      { path: ['insert', 'match_case'], value: false },
      { path: ['overlay', 'size'], value: 'large' },
      { path: ['history', 'enabled'], value: true },
      { path: ['history', 'retention_days'], value: undefined },
      {
        path: ['dictionary'],
        value: [
          { heard: ['a', 'b'], write: 'C' },
          { heard: ['d'], write: 'E' },
        ],
      },
      { path: ['snippets', 0, 'text'], value: '10 Downing Street\nLondon' },
      { path: ['profiles', 1, 'cleanup', 'llm_enabled'], value: false },
      { path: ['profiles', 1, 'overlay', 'style'], value: 'none' },
      { path: ['profiles', 0, 'match_title'], value: ['/powershell/i'] },
      {
        path: ['profiles', 2],
        value: { name: 'Games', match_process: ['*.game.exe'], enabled: false },
      },
      { path: ['local_models'], value: [{ id: 'mine', path: 'D:\\models\\mine.gguf' }] },
    ];
    const out = patchAndCheck(EXAMPLE, patches);
    expectCommentsKept(EXAMPLE, out);
    expect(validateConfig(parse(out)).issues).toEqual([]);
    expect(patchToml(out, patches)).toBe(out);
  });
});
