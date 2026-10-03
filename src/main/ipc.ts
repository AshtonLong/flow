/**
 * IPC handlers. Every message from a renderer is checked to come from one of
 * our own windows and is schema-validated before it reaches any service.
 */
import { app, clipboard, dialog, ipcMain, shell, type IpcMainInvokeEvent } from 'electron';
import type { z } from 'zod';
import { cloudCost, isCloudModelId } from '@shared/catalog';
import { IPC, type BenchResult, type KeyTestResult } from '@shared/ipc';
import {
  BenchRunSchema,
  CaptureErrorSchema,
  ConfigPatchesSchema,
  ExternalUrlSchema,
  HistorySearchSchema,
  ModelIdSchema,
  PcmSchema,
  ProviderIdSchema,
  SecretSetSchema,
} from '@shared/ipc-schemas';
import type { AppInfo } from '@shared/types';
import { SAMPLE_RATE } from '@shared/types';
import { z as zod } from 'zod';
import { dictionaryHints } from './cleanup';
import type { ConfigStore } from './config';
import type { HistoryStore } from './history/store';
import type { InputHook } from './input/hook';
import { log } from './log';
import type { ModelManager } from './models';
import type { SecretStore } from './secrets/store';
import type { DictationSession } from './session/session';
import { testProviderKey } from './transcribe/cloud';
import type { EngineClient } from './transcribe/engine-client';
import { toTranscribeError, type TranscriberRouter } from './transcribe/router';
import { checkForUpdates } from './updater';
import type { OverlayWindow } from './windows/overlay';
import { isOwnUrl } from './windows/paths';
import type { SettingsWindow } from './windows/settings';
import { systemTheme } from './windows/theme';

export interface IpcDeps {
  config: ConfigStore;
  secrets: SecretStore;
  history: HistoryStore | null;
  models: ModelManager;
  engine: EngineClient;
  router: TranscriberRouter;
  session: DictationSession;
  hook: InputHook;
  overlay: OverlayWindow;
  settings: SettingsWindow;
  paths: { logs: string; models: string };
  degraded: AppInfo['degraded'];
}

function fromOwnWindow(event: IpcMainInvokeEvent): boolean {
  const url = event.senderFrame?.url ?? '';
  return isOwnUrl(url);
}

/** Registers an invoke handler that validates the sender and the single argument. */
function handle<S extends z.ZodType, R>(
  channel: string,
  schema: S | null,
  fn: (arg: z.infer<S>, event: IpcMainInvokeEvent) => R | Promise<R>,
): void {
  ipcMain.handle(channel, async (event, raw) => {
    if (!fromOwnWindow(event)) throw new Error('Rejected: unknown sender');
    const arg = schema ? schema.parse(raw) : (undefined as z.infer<S>);
    return fn(arg, event);
  });
}

export function registerIpc(deps: IpcDeps): void {
  const { config, secrets, history, models, engine, router, session, hook, overlay, settings } =
    deps;

  // ── Overlay → Main ───────────────────────────────────────────────────────
  const fromOverlay = (event: Electron.IpcMainEvent) => event.sender.id === overlay.webContentsId;

  ipcMain.on(IPC.overlayReady, (event) => {
    if (fromOverlay(event)) overlay.markReady();
  });
  ipcMain.on(IPC.captureStarted, (event) => {
    if (fromOverlay(event)) session.onCaptureStarted();
  });
  ipcMain.on(IPC.captureChunk, (event, pcm: unknown) => {
    if (!fromOverlay(event) || !(pcm instanceof Float32Array) || pcm.length > SAMPLE_RATE) return;
    session.onCaptureChunk(pcm);
  });
  ipcMain.on(IPC.captureStopped, (event) => {
    if (fromOverlay(event)) session.onCaptureStopped();
  });
  ipcMain.on(IPC.captureError, (event, raw: unknown) => {
    if (!fromOverlay(event)) return;
    const parsed = CaptureErrorSchema.safeParse(raw);
    if (parsed.success) session.onCaptureError(parsed.data.reason, parsed.data.message);
  });

  // ── Config ───────────────────────────────────────────────────────────────
  handle(IPC.configGet, null, () => config.snapshot());
  handle(IPC.configSet, ConfigPatchesSchema, (patches) => config.set(patches));
  handle(IPC.configOpenFile, null, async () => {
    await shell.openPath(config.path);
  });
  handle(IPC.configExport, null, async () => {
    const win = settings.browserWindow;
    const options = {
      title: 'Export Flow settings',
      defaultPath: 'flow-config.toml',
      filters: [{ name: 'TOML', extensions: ['toml'] }],
    };
    const result = win
      ? await dialog.showSaveDialog(win, options)
      : await dialog.showSaveDialog(options);
    if (result.canceled || !result.filePath) return false;
    await config.exportTo(result.filePath);
    return true;
  });
  handle(IPC.configImport, null, async () => {
    const win = settings.browserWindow;
    const options = {
      title: 'Import Flow settings',
      properties: ['openFile' as const],
      filters: [{ name: 'TOML', extensions: ['toml'] }],
    };
    const result = win
      ? await dialog.showOpenDialog(win, options)
      : await dialog.showOpenDialog(options);
    const file = result.filePaths[0];
    if (result.canceled || !file) return false;
    await config.importFrom(file);
    return true;
  });

  // ── Models ───────────────────────────────────────────────────────────────
  handle(IPC.modelsList, null, () => models.snapshot());
  handle(IPC.modelsDownload, ModelIdSchema, async (id) => {
    // Failures are reported through progress events, which the UI already shows.
    await models.download(id).catch((err) => log.warn('models', `download ${id}: ${String(err)}`));
  });
  handle(IPC.modelsCancel, ModelIdSchema, (id) => models.cancelDownload(id));
  handle(IPC.modelsRemove, ModelIdSchema, async (id) => {
    if (engine.loadedModelId === id) await engine.unload().catch(() => undefined);
    await models.remove(id);
  });
  handle(IPC.modelsActivate, ModelIdSchema, async (id) => {
    await config.set([{ path: ['model', 'active'], value: id }]);
  });
  handle(IPC.modelsRefresh, null, async () => {
    await models.refreshCatalog();
    return models.snapshot();
  });

  // ── Secrets: keys flow one way, into the main process ────────────────────
  handle(IPC.secretsSet, SecretSetSchema, async ({ provider, key }) => {
    await secrets.set(provider, key.trim());
    // The file holds only a reference to the key.
    const current = config.get().providers[provider]?.api_key;
    if (key.trim() && current !== `secret:${provider}`) {
      await config.set([{ path: ['providers', provider, 'api_key'], value: `secret:${provider}` }]);
    }
    models.emit('changed');
  });
  handle(IPC.secretsHas, ProviderIdSchema, (provider) => secrets.has(provider));
  handle(IPC.secretsDelete, ProviderIdSchema, async (provider) => {
    await secrets.delete(provider);
    if (config.get().providers[provider]?.api_key) {
      await config.set([{ path: ['providers', provider, 'api_key'], value: undefined }]);
    }
    models.emit('changed');
  });
  handle(IPC.secretsTest, ProviderIdSchema, (provider): Promise<KeyTestResult> =>
    testProviderKey(provider, {
      catalog: models.catalog(),
      config: config.get(),
      getSecret: (id) => secrets.get(id),
    }),
  );

  // ── Test bench ───────────────────────────────────────────────────────────
  let benchRun = 0;
  handle(IPC.benchRun, BenchRunSchema, async ({ audio, modelIds }) => {
    const runId = ++benchRun;
    void (async () => {
      const cfg = config.get();
      const speech = await engine
        .trim(audio)
        .then((r) => r.audio ?? audio)
        .catch(() => audio);
      const audioMs = (speech.length / SAMPLE_RATE) * 1000;
      for (let i = 0; i < modelIds.length; i++) {
        const modelId = modelIds[i]!;
        const done = i === modelIds.length - 1;
        let result: BenchResult;
        const started = performance.now();
        try {
          const transcript = await router.transcribeWith(modelId, speech, cfg, {
            language: cfg.general.language,
            hints: dictionaryHints(cfg),
            signal: new AbortController().signal,
          });
          const entry = isCloudModelId(modelId) ? models.cloudEntry(modelId) : undefined;
          result = {
            runId,
            modelId,
            ok: true,
            text: transcript.text,
            elapsedMs: performance.now() - started,
            cost: entry ? cloudCost(entry, audioMs) : undefined,
            done,
          };
        } catch (err) {
          result = { runId, modelId, ok: false, error: toTranscribeError(err).userMessage, done };
        }
        settings.send(IPC.benchResult, result);
      }
    })();
    return runId;
  });

  // ── Hotkey recorder ──────────────────────────────────────────────────────
  handle(IPC.hotkeysCapture, null, () =>
    hook.captureBinding((partial) => settings.send(IPC.hotkeysCaptureProgress, partial)),
  );
  handle(IPC.hotkeysCaptureCancel, null, () => hook.cancelCapture());

  // ── History ──────────────────────────────────────────────────────────────
  handle(IPC.historySearch, HistorySearchSchema, ({ query, limit, offset }) =>
    history ? history.search(query, limit, offset) : [],
  );
  handle(IPC.historyDelete, zod.number().int(), (id) => history?.delete(id));
  handle(IPC.historyClear, null, () => history?.clear());
  handle(IPC.historyRerun, zod.number().int(), async (id) => {
    const entry = history?.get(id);
    if (!history || !entry) return null;
    const text = await session.cleanUp(entry.rawText, config.get());
    return history.update(id, { text }) ?? null;
  });

  // ── Dictation ────────────────────────────────────────────────────────────
  handle(IPC.dictationTest, PcmSchema, async (audio) => {
    try {
      return await session.test(audio);
    } catch (err) {
      throw new Error(toTranscribeError(err).userMessage);
    }
  });

  // ── App ──────────────────────────────────────────────────────────────────
  handle(IPC.appInfo, null, async (): Promise<AppInfo> => {
    if (engine.devices.length === 0) {
      // First ask: probe the engine, but do not hold the window up for long.
      await Promise.race([
        engine.probe().catch((err) => log.warn('engine', `probe failed: ${String(err)}`)),
        new Promise((resolve) => setTimeout(resolve, 4000)),
      ]);
    }
    return {
      version: app.getVersion(),
      electron: process.versions.electron ?? '',
      platform: `${process.platform} ${process.arch}`,
      configPath: config.path,
      logsPath: deps.paths.logs,
      modelsPath: deps.paths.models,
      devices: engine.devices,
      degraded: deps.degraded,
      paused: session.isPaused,
    };
  });
  handle(IPC.appOpenLogs, null, async () => {
    await shell.openPath(deps.paths.logs);
  });
  handle(IPC.appOpenModels, null, async () => {
    await shell.openPath(deps.paths.models);
  });
  handle(IPC.appOpenExternal, ExternalUrlSchema, async (url) => {
    await shell.openExternal(url);
  });
  handle(IPC.appCopy, zod.string().max(100_000), (text) => clipboard.writeText(text));
  handle(IPC.appSetPaused, zod.boolean(), (paused) => session.setPaused(paused));
  handle(IPC.appCheckUpdates, null, () => checkForUpdates());
  handle(IPC.themeGet, null, () => systemTheme());
}
