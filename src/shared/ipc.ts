/**
 * IPC contracts. Channel names, payload validators, and the typed APIs that the
 * preload scripts expose to each renderer. Renderers never get Node.js access;
 * every message they send is validated here on the main side.
 */
import type { CatalogSnapshot, DownloadProgress } from './catalog';
import type { ConfigPatch, ConfigSnapshot } from './config';
import type {
  AppInfo,
  CaptureErrorReason,
  CaptureStart,
  DictationEvent,
  HistoryEntry,
  OverlayAppearance,
  OverlayState,
  ThemeInfo,
  Transcript,
} from './types';

export const IPC = {
  // Main → Overlay
  captureStart: 'capture:start',
  captureStop: 'capture:stop',
  overlayState: 'overlay:state',
  overlayAppearance: 'overlay:appearance',
  // Overlay → Main
  captureStarted: 'capture:started',
  captureChunk: 'capture:chunk',
  captureError: 'capture:error',
  captureStopped: 'capture:stopped',
  overlayReady: 'overlay:ready',
  // Settings ↔ Main
  configGet: 'config:get',
  configSet: 'config:set',
  configChanged: 'config:changed',
  configOpenFile: 'config:open-file',
  configExport: 'config:export',
  configImport: 'config:import',
  modelsList: 'models:list',
  modelsDownload: 'models:download',
  modelsCancel: 'models:cancel',
  modelsRemove: 'models:remove',
  modelsActivate: 'models:activate',
  modelsProgress: 'models:progress',
  modelsChanged: 'models:changed',
  modelsRefresh: 'models:refresh',
  secretsSet: 'secrets:set',
  secretsHas: 'secrets:has',
  secretsDelete: 'secrets:delete',
  secretsTest: 'secrets:test',
  benchRun: 'bench:run',
  benchResult: 'bench:result',
  hotkeysCapture: 'hotkeys:capture',
  hotkeysCaptureCancel: 'hotkeys:capture-cancel',
  hotkeysCaptureProgress: 'hotkeys:capture-progress',
  historySearch: 'history:search',
  historyDelete: 'history:delete',
  historyClear: 'history:clear',
  historyRerun: 'history:rerun',
  dictationEvent: 'dictation:event',
  dictationTest: 'dictation:test',
  appInfo: 'app:info',
  appOpenLogs: 'app:open-logs',
  appOpenModels: 'app:open-models',
  appOpenExternal: 'app:open-external',
  appCopy: 'app:copy',
  appSetPaused: 'app:set-paused',
  appCheckUpdates: 'app:check-updates',
  themeGet: 'theme:get',
  themeChanged: 'theme:changed',
} as const;

export interface BenchResult {
  /** Identifies the run so stale results can be ignored. */
  runId: number;
  modelId: string;
  ok: boolean;
  text?: string;
  /** Wall-clock milliseconds for this model, including load. */
  elapsedMs?: number;
  /** USD, cloud models only. */
  cost?: number;
  error?: string;
  /** True on the last result of a run. */
  done: boolean;
}

export interface KeyTestResult {
  ok: boolean;
  message: string;
}

type Unsubscribe = () => void;

/** Exposed to the overlay window as `window.flowOverlay`. */
export interface OverlayApi {
  onCaptureStart(cb: (opts: CaptureStart) => void): Unsubscribe;
  onCaptureStop(cb: () => void): Unsubscribe;
  onState(cb: (state: OverlayState) => void): Unsubscribe;
  onAppearance(cb: (appearance: OverlayAppearance) => void): Unsubscribe;
  /** Audio is flowing: the pill may appear. */
  captureStarted(): void;
  captureChunk(pcm: Float32Array): void;
  captureError(reason: CaptureErrorReason, message?: string): void;
  /** All chunks for the recording have been sent. */
  captureStopped(): void;
  /** The renderer has mounted and subscribed. */
  ready(): void;
}

/** Exposed to the settings window as `window.flow`. */
export interface SettingsApi {
  config: {
    get(): Promise<ConfigSnapshot>;
    /** Patches values in place in `config.toml` and returns the reloaded config. */
    set(patches: ConfigPatch[]): Promise<ConfigSnapshot>;
    onChanged(cb: (snapshot: ConfigSnapshot) => void): Unsubscribe;
    /** Opens `config.toml` in the default editor. */
    openFile(): Promise<void>;
    /** Save / open dialogs. Resolve false when the user cancels. API keys are excluded. */
    exportFile(): Promise<boolean>;
    importFile(): Promise<boolean>;
  };
  models: {
    list(): Promise<CatalogSnapshot>;
    download(id: string): Promise<void>;
    cancelDownload(id: string): Promise<void>;
    remove(id: string): Promise<void>;
    activate(id: string): Promise<void>;
    /** Re-fetches the signed remote catalog. */
    refresh(): Promise<CatalogSnapshot>;
    onProgress(cb: (progress: DownloadProgress) => void): Unsubscribe;
    onChanged(cb: (snapshot: CatalogSnapshot) => void): Unsubscribe;
  };
  secrets: {
    /** Stores a key encrypted with DPAPI. Keys are never sent back to a renderer. */
    set(provider: string, key: string): Promise<void>;
    has(provider: string): Promise<boolean>;
    delete(provider: string): Promise<void>;
    /** Makes one cheap authenticated request to check the stored key. */
    test(provider: string): Promise<KeyTestResult>;
  };
  bench: {
    /** Runs one recorded phrase through each model; results arrive via `onResult`. */
    run(audio: Float32Array, modelIds: string[]): Promise<number>;
    onResult(cb: (result: BenchResult) => void): Unsubscribe;
  };
  hotkeys: {
    /**
     * Records the next binding with the global hook (so `Win` and mouse buttons
     * work). Resolves the binding string, or null if cancelled. Dictation
     * hotkeys are suspended while recording.
     */
    capture(): Promise<string | null>;
    cancelCapture(): Promise<void>;
    /** Keys held so far during a capture, as a binding string. */
    onCaptureProgress(cb: (partial: string) => void): Unsubscribe;
  };
  history: {
    search(query: string, limit: number, offset: number): Promise<HistoryEntry[]>;
    delete(id: number): Promise<void>;
    clear(): Promise<void>;
    /** Re-applies the current cleanup settings to the stored raw transcript. */
    rerun(id: number): Promise<HistoryEntry | null>;
  };
  dictation: {
    /** Fires after every dictation, so first-run can show a live test. */
    onEvent(cb: (event: DictationEvent) => void): Unsubscribe;
    /** Transcribes audio recorded in the settings window with the active model. */
    test(audio: Float32Array): Promise<Transcript>;
  };
  app: {
    info(): Promise<AppInfo>;
    openLogs(): Promise<void>;
    openModelsFolder(): Promise<void>;
    openExternal(url: string): Promise<void>;
    copyText(text: string): Promise<void>;
    setPaused(paused: boolean): Promise<void>;
    checkForUpdates(): Promise<string>;
  };
  theme: {
    get(): Promise<ThemeInfo>;
    onChanged(cb: (theme: ThemeInfo) => void): Unsubscribe;
  };
}

declare global {
  interface Window {
    flow: SettingsApi;
    flowOverlay: OverlayApi;
  }
}
