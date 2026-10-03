/** App startup: single-instance lock, services, tray, overlay, hotkeys. */
import fs from 'node:fs';
import path from 'node:path';
import {
  app,
  clipboard,
  ClipboardItem,
  powerMonitor,
  safeStorage,
  session as electronSession,
} from 'electron';
import type { Config } from '@shared/config';
import { IPC } from '@shared/ipc';
import type { AppInfo } from '@shared/types';
import { ConfigStore } from './config';
import { HistoryStore } from './history/store';
import { InputHook } from './input/hook';
import { Inserter } from './insert/inserter';
import { registerIpc } from './ipc';
import { log } from './log';
import { ModelManager } from './models';
import { createPlatform } from './platform';
import { SecretStore } from './secrets/store';
import { DictationSession } from './session/session';
import { EngineClient } from './transcribe/engine-client';
import { TranscriberRouter } from './transcribe/router';
import { scheduleUpdateCheck } from './updater';
import { OverlayWindow } from './windows/overlay';
import { enginePath, isOwnUrl, packagedLibraryPath, resourcePath } from './windows/paths';
import { SettingsWindow } from './windows/settings';
import { applyThemeSource, onThemeChanged, systemTheme } from './windows/theme';
import { AppTray, type TrayModel } from './windows/tray';

// Tests point these at temporary directories.
if (process.env.FLOW_USER_DATA) app.setPath('userData', process.env.FLOW_USER_DATA);

// A pill and a settings form do not need the GPU, and software rendering keeps
// idle memory inside the 200 MB budget. Local models reach the GPU on their own.
app.disableHardwareAcceleration();

// The overlay starts its AudioContext from a hotkey, not a click inside the page.
app.commandLine.appendSwitch('autoplay-policy', 'no-user-gesture-required');

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  void main();
}

async function main(): Promise<void> {
  await app.whenReady();
  app.setAppUserModelId('computer.flow.dictation');

  const userData = app.getPath('userData');
  const logsDir = path.join(userData, 'logs');
  const modelsDir =
    process.env.FLOW_MODELS_DIR ??
    path.join(process.env.LOCALAPPDATA ?? userData, 'Flow', 'models');
  log.init(logsDir);
  log.info('app', `Flow ${app.getVersion()} starting (Electron ${process.versions.electron})`);

  const degraded: AppInfo['degraded'] = [];

  // ── Services ─────────────────────────────────────────────────────────────
  const config = new ConfigStore(path.join(userData, 'config.toml'));
  await config.load();
  config.watch();

  const secrets = new SecretStore(path.join(userData, 'secrets.bin'), {
    isAvailable: () => safeStorage.isEncryptionAvailable(),
    encrypt: (plain) => safeStorage.encryptString(plain),
    decrypt: (data) => safeStorage.decryptString(data),
  });
  await secrets.load();

  let history: HistoryStore | null = null;
  try {
    history = new HistoryStore(path.join(userData, 'history.db'));
  } catch (err) {
    log.error('history', `unavailable: ${String(err)}`);
    degraded.push({ module: 'better-sqlite3', effect: 'History is disabled' });
  }

  // Optional GPU pack: a CUDA build of the speech library plus the CUDA runtime,
  // dropped into this folder. Without it, GPUs are used through Vulkan.
  const gpuPack = path.join(process.env.LOCALAPPDATA ?? userData, 'Flow', 'gpu-pack');
  const gpuLibrary = path.join(gpuPack, 'transcribe.dll');
  const hasGpuPack = fs.existsSync(gpuLibrary);
  if (hasGpuPack) log.info('engine', `using the GPU pack at ${gpuPack}`);

  const engine = new EngineClient({
    entry: enginePath(),
    vadModelPath: resourcePath('silero_vad.onnx'),
    libraryPath: hasGpuPack ? gpuLibrary : packagedLibraryPath(),
    cudaRuntimeDir: hasGpuPack ? gpuPack : undefined,
    debug: config.get().general.debug_logging,
  });

  const models = new ModelManager({
    modelsDir,
    userDataDir: userData,
    bundledCatalogPath: resourcePath('catalog.json'),
    getConfig: () => config.get(),
    hasSecret: (id) => secrets.has(id),
    loadedModelId: () => engine.loadedModelId,
  });
  await models.init();

  const platform = createPlatform();
  if (!platform.available) {
    degraded.push({ module: 'koffi', effect: 'Typing method and per-app profiles are disabled' });
  }

  const hook = new InputHook({
    maskStartMenu: () => platform.maskStartMenu(),
    // Key-ups that go to an elevated window are never seen by the hook; this lets it notice.
    isKeyDown: (key) => platform.isKeyDown(key),
  });
  const inserter = new Inserter({
    platform,
    clipboard,
    makeItem: (data) => new ClipboardItem(data),
    tapShortcut: (binding) => hook.tapShortcut(binding),
  });
  const router = new TranscriberRouter({ engine, models, getSecret: (id) => secrets.get(id) });

  // ── Windows ──────────────────────────────────────────────────────────────
  const overlay = new OverlayWindow();
  const settings = new SettingsWindow(() => {
    const theme = systemTheme();
    return { ...theme, effectiveDark: theme.dark };
  });

  const session = new DictationSession({
    getConfig: () => config.get(),
    getCatalog: () => models.catalog(),
    getSecret: (id) => secrets.get(id),
    getFocusedApp: () => platform.getFocusedApp(),
    insert: (text, options) => inserter.insert(text, options),
    copy: (text) => inserter.copy(text),
    router,
    engine,
    overlay,
    setCancelArmed: (armed) => hook.setCancelArmed(armed),
    addHistory: (entry) => {
      history?.add(entry);
    },
  });

  const tray = new AppTray({
    activateModel: (id) => void config.set([{ path: ['model', 'active'], value: id }]),
    setPaused: (paused) => session.setPaused(paused),
    copyLast: () => session.copyLast(),
    retryLast: () => session.retryLast(),
    openSettings: () => settings.open(),
    quit: () => app.quit(),
  });

  let hookStarted = false;

  function trayModels(): TrayModel[] {
    const snapshot = models.snapshot();
    const entries = [...snapshot.catalog.local, ...snapshot.catalog.cloud, ...snapshot.custom];
    return entries
      .filter((entry) => snapshot.status[entry.id]?.ready)
      .map((entry) => ({ id: entry.id, name: entry.name, cloud: entry.kind === 'cloud' }));
  }

  function updateTray(): void {
    tray.update({
      activeModelId: config.get().model.active,
      models: trayModels(),
      paused: session.isPaused,
      hasLast: session.lastText !== null,
      canRetry: session.canRetry,
      hookFailed: !hookStarted,
    });
  }

  function applyConfig(cfg: Config): void {
    log.setDebug(cfg.general.debug_logging);
    applyThemeSource(cfg.general.theme);
    hook.setBindings(cfg.hotkeys);
    engine.setKeepLoadedMinutes(cfg.model.keep_loaded_minutes);
    overlay.configure(cfg, systemTheme());
    settings.refreshTheme();
    if (app.isPackaged) {
      app.setLoginItemSettings({ openAtLogin: cfg.general.launch_at_login, args: ['--hidden'] });
    }
    if (history && cfg.history.enabled) {
      try {
        history.prune(cfg.history.retention_days);
      } catch (err) {
        log.warn('history', `prune failed: ${String(err)}`);
      }
    }
    // "Always loaded" keeps the model in memory from launch.
    if (cfg.model.keep_loaded_minutes < 0) router.warmUp(cfg);
    updateTray();
  }

  // ── Security: our windows only, microphone only ──────────────────────────
  electronSession.defaultSession.setPermissionRequestHandler((contents, permission, callback) => {
    callback(permission === 'media' && isOwnUrl(contents.getURL()));
  });
  electronSession.defaultSession.setPermissionCheckHandler((contents, permission) => {
    return permission === 'media' && isOwnUrl(contents?.getURL() ?? '');
  });
  app.on('web-contents-created', (_event, contents) => {
    contents.on('will-attach-webview', (event) => event.preventDefault());
  });

  registerIpc({
    config,
    secrets,
    history,
    models,
    engine,
    router,
    session,
    hook,
    overlay,
    settings,
    paths: { logs: logsDir, models: modelsDir },
    degraded,
  });

  // ── Wiring ───────────────────────────────────────────────────────────────
  config.on('changed', (snapshot) => {
    applyConfig(snapshot.config);
    settings.send(IPC.configChanged, snapshot);
    settings.send(IPC.modelsChanged, models.snapshot());
  });
  models.on('progress', (progress) => settings.send(IPC.modelsProgress, progress));
  models.on('changed', () => {
    settings.send(IPC.modelsChanged, models.snapshot());
    updateTray();
  });
  engine.on('loaded', () => settings.send(IPC.modelsChanged, models.snapshot()));
  session.on('dictation', (event) => {
    settings.send(IPC.dictationEvent, event);
    updateTray();
  });
  session.on('paused', () => updateTray());
  onThemeChanged((theme) => {
    overlay.configure(config.get(), theme);
    settings.refreshTheme();
    settings.send(IPC.themeChanged, theme);
  });
  hook.on('hotkey', (event) => session.handleHotkey(event));

  // End-to-end tests drive the session directly instead of pressing real keys.
  if (process.env.FLOW_TEST === '1') {
    (globalThis as Record<string, unknown>).__flow = {
      session,
      platform,
      engine,
      overlay,
      inserter,
    };
  }

  overlay.create();
  tray.create();
  applyConfig(config.get());

  hookStarted = hook.start();
  if (!hookStarted) {
    log.error('input', 'the global input hook failed to start; hotkeys are unavailable');
    degraded.push({ module: 'uiohook-napi', effect: 'Hotkeys are unavailable' });
  }
  updateTray();

  // Keys released while asleep never produce a key-up, so forget what was held.
  powerMonitor.on('resume', () => {
    hook.reset();
    session.cancel();
  });
  powerMonitor.on('lock-screen', () => session.cancel());

  app.on('second-instance', () => settings.open());
  // A tray app: closing the settings window must not quit.
  app.on('window-all-closed', () => undefined);
  app.on('before-quit', () => {
    hook.stop();
    engine.shutdown();
    overlay.destroy();
    tray.destroy();
    history?.close();
    void config.close();
  });

  const hidden = process.argv.includes('--hidden');
  if (process.argv.includes('--settings') || (!config.get().general.onboarded && !hidden)) {
    settings.open();
  }

  // Release CI launches the packaged app with this flag to prove the native modules load.
  if (process.argv.includes('--smoke-test')) {
    try {
      const probe = await engine.probe();
      const ok = hookStarted && platform.available && history !== null;
      log.info('smoke', `engine ${probe.version}, vad ${probe.vad}, ok=${ok}`);
      app.exit(ok ? 0 : 1);
    } catch (err) {
      log.error('smoke', String(err));
      app.exit(1);
    }
    return;
  }

  setTimeout(() => void models.refreshCatalog(), 15_000).unref();
  scheduleUpdateCheck();
  log.info('app', 'tray ready');
}
