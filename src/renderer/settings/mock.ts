/**
 * An in-memory `SettingsApi` for running the settings window in a plain
 * browser (`scripts/preview-settings.vite.config.ts`). It is installed only
 * when the real preload has not defined `window.flow`.
 *
 * Query parameters shape the starting state, for screenshots:
 *   ?theme=dark|light   ?contrast=high   ?motion=reduced   ?accent=c2410c
 *   ?onboarding         first-run flow (`general.onboarded = false`)
 *   ?fresh              nothing installed, no keys, empty lists
 *   ?history=off        history disabled
 *   ?downloading=<id>   a download in flight for that model
 *   ?degraded           a failed native module in `app.info()`
 *   ?noissues           no config problems
 */
import catalogJson from '../../../resources/catalog.json';
import {
  CatalogSchema,
  CUSTOM_PREFIX,
  cloudCost,
  type CatalogSnapshot,
  type CloudModelEntry,
  type DownloadProgress,
  type ModelEntry,
  type ModelStatus,
} from '@shared/catalog';
import {
  validateConfig,
  type ConfigIssue,
  type ConfigPatch,
  type ConfigSnapshot,
} from '@shared/config';
import { bindingFromKeys, formatBinding } from '@shared/hotkeys';
import type { BenchResult, SettingsApi } from '@shared/ipc';
import {
  SAMPLE_RATE,
  type AppInfo,
  type DictationEvent,
  type HistoryEntry,
  type ThemeInfo,
} from '@shared/types';
import { applyPatches } from './lib/patch';

type Listener<T> = (value: T) => void;

function emitter<T>() {
  const listeners = new Set<Listener<T>>();
  return {
    on(cb: Listener<T>) {
      listeners.add(cb);
      return () => {
        listeners.delete(cb);
      };
    },
    emit(value: T) {
      for (const cb of [...listeners]) cb(value);
    },
  };
}

const delay = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

const SENTENCES = [
  'Can you send me the quarterly numbers before the stand-up tomorrow?',
  'Remind me to renew the domain for the Tauri prototype on Friday.',
  'I think the retry logic should back off exponentially, starting at two hundred milliseconds.',
  'Thanks for the review. I have pushed a fix for the flaky test and rebased onto main.',
  'The meeting with Priya moved to three thirty, same room.',
  'Add oat milk, coffee filters and a pack of AA batteries to the list.',
  'Let us ship the settings window first and tidy the overlay animation afterwards.',
  'Dear Sam, it was good to meet you last week. I have attached the proposal we discussed.',
  'TODO: handle the case where the microphone is unplugged halfway through a recording.',
  'Sounds good, see you at seven.',
  'The parser drops trailing commas, which is why the config failed to load.',
  'Could you check whether the installer is still under one hundred and twenty megabytes?',
];

const APPS = [
  'Code.exe',
  'slack.exe',
  'chrome.exe',
  'WindowsTerminal.exe',
  'OUTLOOK.EXE',
  'notepad.exe',
];

function sampleHistory(count: number): HistoryEntry[] {
  const now = Date.now();
  const models = ['parakeet-tdt-0.6b-v2', 'groq/whisper-large-v3-turbo'];
  return Array.from({ length: count }, (_, i) => {
    const text = SENTENCES[i % SENTENCES.length]!;
    const audioMs = 1800 + ((i * 937) % 9000);
    return {
      id: count - i,
      createdAt: now - i * 47 * 60_000 - (i > 6 ? 86_400_000 : 0) - (i > 20 ? 40 * 86_400_000 : 0),
      text,
      rawText: `um ${text.charAt(0).toLowerCase()}${text.slice(1)}`,
      model: models[i % 3 === 0 ? 1 : 0]!,
      app: APPS[i % APPS.length]!,
      audioMs,
      elapsedMs: 380 + ((i * 173) % 500),
    };
  });
}

const CODE_KEYS: Record<string, string> = {
  ControlLeft: 'LCtrl',
  ControlRight: 'RCtrl',
  AltLeft: 'LAlt',
  AltRight: 'RAlt',
  ShiftLeft: 'LShift',
  ShiftRight: 'RShift',
  MetaLeft: 'LWin',
  MetaRight: 'RWin',
  Escape: 'Esc',
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
};

/** Maps a DOM `KeyboardEvent.code` to the hook's key id. */
function keyIdOf(code: string): string | null {
  if (CODE_KEYS[code]) return CODE_KEYS[code];
  if (code.startsWith('Key')) return code.slice(3);
  if (code.startsWith('Digit')) return code.slice(5);
  if (/^(F\d{1,2}|Numpad\d)$/.test(code)) return code;
  const named = [
    'Space',
    'Enter',
    'Tab',
    'Backspace',
    'Delete',
    'Insert',
    'Home',
    'End',
    'PageUp',
    'PageDown',
    'CapsLock',
    'ScrollLock',
    'NumLock',
    'PrintScreen',
    'Pause',
    'Minus',
    'Equal',
    'BracketLeft',
    'BracketRight',
    'Backslash',
    'Semicolon',
    'Quote',
    'Comma',
    'Period',
    'Slash',
    'Backquote',
  ];
  return named.includes(code) ? code : null;
}

const MOUSE_KEYS: Record<number, string> = { 1: 'Mouse3', 3: 'Mouse4', 4: 'Mouse5' };

export function createMock(params: URLSearchParams): SettingsApi {
  const fresh = params.has('fresh');
  const catalog = CatalogSchema.parse(catalogJson);

  // ---- Config -------------------------------------------------------------
  let raw: Record<string, unknown> = {
    version: 1,
    general: { launch_at_login: true, theme: 'system', onboarded: !params.has('onboarding') },
    audio: params.has('noissues') || fresh ? {} : { max_recording_seconds: 2 },
    model: { active: 'parakeet-tdt-0.6b-v2', fallback: 'parakeet-tdt-0.6b-v2' },
    history: { enabled: params.get('history') !== 'off' && !fresh, retention_days: 30 },
    ...(fresh
      ? {}
      : {
          providers: {
            groq: { api_key: 'secret:groq' },
            'custom-lan': {
              base_url: 'http://192.168.1.20:8000/v1',
              model: 'whisper-large-v3',
              api_key: 'secret:custom-lan',
            },
          },
          prompts: {
            standup: {
              text: 'Rewrite as short stand-up notes: what I did, what is next, blockers.',
            },
          },
          dictionary: [
            { heard: ['tory', 'towery'], write: 'Tauri' },
            { heard: ['pre a', 'priya'], write: 'Priya' },
            { heard: ['k eight s', 'kates'], write: 'k8s' },
          ],
          snippets: [
            { trigger: 'my address', text: '221B Baker Street\nLondon NW1 6XE' },
            { trigger: 'sign off', text: 'Best regards,\nAshton' },
          ],
          profiles: [
            {
              name: 'Terminals',
              match_process: ['WindowsTerminal.exe', 'wezterm-gui.exe'],
              insert: { paste_shortcut: 'Ctrl+Shift+V' },
              cleanup: { llm_enabled: false },
            },
            {
              name: 'Slack',
              match_process: ['slack.exe'],
              model: { active: 'groq/whisper-large-v3-turbo' },
              cleanup: { llm_enabled: true, llm_prompt: 'casual' },
            },
          ],
        }),
  };
  const configPath = 'C:\\Users\\Ashton\\AppData\\Roaming\\Flow\\config.toml';
  const configChanged = emitter<ConfigSnapshot>();

  const configSnapshot = (): ConfigSnapshot => {
    const { config, issues } = validateConfig(raw);
    const located: ConfigIssue[] = issues.map((issue, i) => ({ ...issue, line: 14 + i * 9 }));
    return { config, issues: located, path: configPath };
  };

  // ---- Models -------------------------------------------------------------
  const installed = new Set(fresh ? [] : ['parakeet-tdt-0.6b-v2', 'moonshine-v2-tiny']);
  const keys = new Set(fresh ? [] : ['groq', 'custom-lan']);
  const keyValues = new Map<string, string>();
  const downloads = new Map<string, DownloadProgress>();
  const timers = new Map<string, ReturnType<typeof setInterval>>();
  const modelsChanged = emitter<CatalogSnapshot>();
  const modelsProgress = emitter<DownloadProgress>();

  const customEntries = (): ModelEntry[] => {
    const known = new Set(catalog.providers.map((p) => p.id));
    const { config } = validateConfig(raw);
    return Object.entries(config.providers)
      .filter(([id, p]) => !known.has(id) && p.base_url)
      .map(([id, p]): CloudModelEntry => ({
        kind: 'cloud',
        id: `${CUSTOM_PREFIX}${id}`,
        provider: id,
        model: p.model ?? '',
        name: `${id} (${p.model ?? 'custom'})`,
        description: p.base_url ?? '',
        pricePerHour: 0,
      }));
  };

  const catalogSnapshot = (): CatalogSnapshot => {
    const active = validateConfig(raw).config.model.active;
    const custom = customEntries();
    const status: Record<string, ModelStatus> = {};
    for (const m of catalog.local) {
      status[m.id] = {
        id: m.id,
        ready: installed.has(m.id),
        active: active === m.id,
        loaded: active === m.id && installed.has(m.id),
        download: downloads.get(m.id),
      };
    }
    const keyless = new Set(catalog.providers.filter((p) => p.keyless).map((p) => p.id));
    for (const m of catalog.cloud) {
      status[m.id] = {
        id: m.id,
        ready: keys.has(m.provider) || keyless.has(m.provider),
        active: active === m.id,
        loaded: false,
      };
    }
    for (const m of custom) {
      status[m.id] = {
        id: m.id,
        ready: true,
        active: active === m.id,
        loaded: false,
        custom: true,
      };
    }
    return {
      catalog,
      status,
      keys: Object.fromEntries(
        [...catalog.providers.map((p) => p.id), ...keys].map((id) => [id, keys.has(id)]),
      ),
      custom,
    };
  };

  const emitModels = () => modelsChanged.emit(catalogSnapshot());

  const stopDownload = (id: string) => {
    const timer = timers.get(id);
    if (timer) clearInterval(timer);
    timers.delete(id);
  };

  const startDownload = (id: string, from = 0, bytesPerSecond = 46e6) => {
    const entry = catalog.local.find((m) => m.id === id);
    if (!entry || timers.has(id)) return;
    let received = entry.sizeBytes * from;
    const publish = (state: DownloadProgress['state'], speed: number) => {
      const progress: DownloadProgress = {
        id,
        receivedBytes: Math.min(received, entry.sizeBytes),
        totalBytes: entry.sizeBytes,
        speed,
        state,
      };
      if (state === 'done' || state === 'cancelled') downloads.delete(id);
      else downloads.set(id, progress);
      modelsProgress.emit(progress);
    };
    publish('downloading', 0);
    timers.set(
      id,
      setInterval(() => {
        const speed = bytesPerSecond * (0.8 + Math.random() * 0.4);
        received += speed * 0.2;
        if (received < entry.sizeBytes) {
          publish('downloading', speed);
          return;
        }
        stopDownload(id);
        publish('verifying', 0);
        setTimeout(() => {
          installed.add(id);
          publish('done', 0);
          emitModels();
        }, 900);
      }, 200),
    );
  };

  const downloading = params.get('downloading');
  if (downloading) startDownload(downloading, 0.42, 3e6);

  // ---- Everything else ----------------------------------------------------
  let history = fresh ? [] : sampleHistory(95);
  let nextHistoryId = history.length + 1;
  const benchResults = emitter<BenchResult>();
  let benchRun = 0;
  const captureProgress = emitter<string>();
  let endCapture: ((result: string | null) => void) | null = null;
  const dictationEvents = emitter<DictationEvent>();
  let paused = false;

  const themeParam = params.get('theme');
  const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');
  const accentParam = params.get('accent');
  const theme = (): ThemeInfo => ({
    dark: themeParam ? themeParam === 'dark' : darkQuery.matches,
    accent: accentParam && /^[0-9a-f]{6}$/i.test(accentParam) ? `#${accentParam}` : '#0067c0',
    highContrast: params.get('contrast') === 'high',
    reducedMotion: params.get('motion') === 'reduced',
  });
  const themeChanged = emitter<ThemeInfo>();
  darkQuery.addEventListener('change', () => themeChanged.emit(theme()));

  const info = (): AppInfo => ({
    version: '0.1.0',
    electron: '44.5.1',
    platform: 'win32',
    configPath,
    logsPath: 'C:\\Users\\Ashton\\AppData\\Roaming\\Flow\\logs',
    modelsPath: 'C:\\Users\\Ashton\\AppData\\Roaming\\Flow\\models',
    devices: fresh
      ? []
      : [
          {
            name: 'CPU',
            kind: 'cpu',
            deviceType: 'cpu',
            description: 'AMD Ryzen 7 7800X3D',
            memoryTotal: 0,
          },
          {
            name: 'CUDA0',
            kind: 'cuda',
            deviceType: 'gpu',
            description: 'NVIDIA GeForce RTX 4070',
            memoryTotal: 12 * 1024 ** 3,
          },
        ],
    degraded: params.has('degraded')
      ? [{ module: 'better-sqlite3', effect: 'History is unavailable.' }]
      : [],
    paused,
  });

  return {
    config: {
      async get() {
        await delay(20);
        return configSnapshot();
      },
      async set(patches: ConfigPatch[]) {
        const activeBefore = validateConfig(raw).config.model.active;
        raw = applyPatches(raw, patches);
        await delay(40);
        const snapshot = configSnapshot();
        configChanged.emit(snapshot);
        if (
          patches.some((p) => p.path[0] === 'providers') ||
          snapshot.config.model.active !== activeBefore
        ) {
          emitModels();
        }
        return snapshot;
      },
      onChanged: configChanged.on,
      async openFile() {},
      async exportFile() {
        await delay(150);
        return true;
      },
      async importFile() {
        await delay(150);
        return true;
      },
    },
    models: {
      async list() {
        await delay(30);
        return catalogSnapshot();
      },
      async download(id) {
        startDownload(id);
      },
      async cancelDownload(id) {
        stopDownload(id);
        const current = downloads.get(id);
        downloads.delete(id);
        if (current) modelsProgress.emit({ ...current, state: 'cancelled', speed: 0 });
        emitModels();
      },
      async remove(id) {
        installed.delete(id);
        emitModels();
      },
      async activate(id) {
        raw = applyPatches(raw, [{ path: ['model', 'active'], value: id }]);
        await delay(40);
        configChanged.emit(configSnapshot());
        emitModels();
      },
      async refresh() {
        await delay(300);
        return catalogSnapshot();
      },
      onProgress: modelsProgress.on,
      onChanged: modelsChanged.on,
    },
    secrets: {
      async set(provider, key) {
        await delay(60);
        if (key) keys.add(provider);
        else keys.delete(provider);
        keyValues.set(provider, key);
        emitModels();
      },
      async has(provider) {
        return keys.has(provider);
      },
      async delete(provider) {
        keys.delete(provider);
        emitModels();
      },
      async test(provider) {
        await delay(700);
        if (!keys.has(provider)) return { ok: false, message: 'No key is stored.' };
        // Any key containing "bad" fails, to show the error state.
        if (keyValues.get(provider)?.includes('bad')) {
          return { ok: false, message: '401 Unauthorized: invalid API key.' };
        }
        return { ok: true, message: 'The key works.' };
      },
    },
    bench: {
      async run(audio, modelIds) {
        const runId = ++benchRun;
        const audioMs = (audio.length / SAMPLE_RATE) * 1000;
        let at = 0;
        modelIds.forEach((modelId, i) => {
          const cloud = catalog.cloud.find((m) => m.id === modelId);
          at += cloud ? 260 + Math.random() * 240 : 420 + Math.random() * 700;
          const elapsedMs = Math.round(at / (i + 1) + (cloud ? 180 : 320));
          const variants = [
            'The quick brown fox jumps over the lazy dog, and Priya ships the Tauri build on Friday.',
            'The quick brown fox jumps over the lazy dog and Priya ships the Tory build on Friday.',
            'The quick brown fox jumps over the lazy dog, and Pria ships the Tauri build on Friday.',
          ];
          setTimeout(() => {
            benchResults.emit({
              runId,
              modelId,
              ok: true,
              text: variants[i % variants.length],
              elapsedMs,
              cost: cloud ? cloudCost(cloud, audioMs) : undefined,
              done: i === modelIds.length - 1,
            });
          }, at);
        });
        return runId;
      },
      onResult: benchResults.on,
    },
    hotkeys: {
      capture() {
        endCapture?.(null);
        return new Promise<string | null>((resolve) => {
          const down = new Set<string>();
          const chord = new Set<string>();
          const finish = (result: string | null) => {
            window.removeEventListener('keydown', onDown, true);
            window.removeEventListener('keyup', onUp, true);
            window.removeEventListener('mousedown', onMouseDown, true);
            window.removeEventListener('mouseup', onMouseUp, true);
            endCapture = null;
            resolve(result);
          };
          const press = (key: string | null) => {
            if (!key) return;
            down.add(key);
            chord.add(key);
            captureProgress.emit(formatBinding(bindingFromKeys(chord)));
          };
          const release = (key: string | null) => {
            if (!key) return;
            down.delete(key);
            if (down.size === 0 && chord.size > 0) finish(formatBinding(bindingFromKeys(chord)));
          };
          const onDown = (e: KeyboardEvent) => press(keyIdOf(e.code));
          const onUp = (e: KeyboardEvent) => release(keyIdOf(e.code));
          const onMouseDown = (e: MouseEvent) => press(MOUSE_KEYS[e.button] ?? null);
          const onMouseUp = (e: MouseEvent) => release(MOUSE_KEYS[e.button] ?? null);
          window.addEventListener('keydown', onDown, true);
          window.addEventListener('keyup', onUp, true);
          window.addEventListener('mousedown', onMouseDown, true);
          window.addEventListener('mouseup', onMouseUp, true);
          endCapture = finish;
        });
      },
      async cancelCapture() {
        endCapture?.(null);
      },
      onCaptureProgress: captureProgress.on,
    },
    history: {
      async search(query, limit, offset) {
        await delay(60);
        const needle = query.toLowerCase();
        const matches = needle
          ? history.filter((e) => e.text.toLowerCase().includes(needle))
          : history;
        return matches.slice(offset, offset + limit);
      },
      async delete(id) {
        history = history.filter((e) => e.id !== id);
      },
      async clear() {
        history = [];
      },
      async rerun(id) {
        await delay(500);
        const entry = history.find((e) => e.id === id);
        if (!entry) return null;
        const next = { ...entry, text: entry.text.replace(/\s+/g, ' ').trim() };
        history = history.map((e) => (e.id === id ? next : e));
        return next;
      },
    },
    dictation: {
      onEvent: dictationEvents.on,
      async test(audio) {
        await delay(650);
        const audioMs = (audio.length / SAMPLE_RATE) * 1000;
        const model = validateConfig(raw).config.model.active;
        const text = 'This is a test of Flow. It heard me the first time.';
        history = [
          {
            id: nextHistoryId++,
            createdAt: Date.now(),
            text,
            rawText: text,
            model,
            app: 'Flow',
            audioMs,
            elapsedMs: 640,
          },
          ...history,
        ];
        return { text, model, audioMs, elapsedMs: 640 };
      },
    },
    app: {
      async info() {
        return info();
      },
      async openLogs() {},
      async openModelsFolder() {},
      async openExternal(url) {
        console.info('[mock] openExternal', url);
      },
      async copyText(text) {
        await navigator.clipboard?.writeText(text).catch(() => {});
      },
      async setPaused(next) {
        paused = next;
      },
      async checkForUpdates() {
        await delay(900);
        return 'Flow 0.1.0 is the latest version.';
      },
    },
    theme: {
      async get() {
        return theme();
      },
      onChanged: themeChanged.on,
    },
  };
}

export function installMock(): void {
  document.documentElement.dataset.preview = '';
  window.flow = createMock(new URLSearchParams(window.location.search));
}
