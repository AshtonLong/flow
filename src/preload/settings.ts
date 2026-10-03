/** Preload for the settings window: the typed `window.flow` API. */
import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';
import { IPC, type SettingsApi } from '@shared/ipc';

function subscribe<T>(channel: string, cb: (payload: T) => void): () => void {
  const listener = (_event: IpcRendererEvent, payload: T) => cb(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

const invoke = ipcRenderer.invoke.bind(ipcRenderer);

const api: SettingsApi = {
  config: {
    get: () => invoke(IPC.configGet),
    set: (patches) => invoke(IPC.configSet, patches),
    onChanged: (cb) => subscribe(IPC.configChanged, cb),
    openFile: () => invoke(IPC.configOpenFile),
    exportFile: () => invoke(IPC.configExport),
    importFile: () => invoke(IPC.configImport),
  },
  models: {
    list: () => invoke(IPC.modelsList),
    download: (id) => invoke(IPC.modelsDownload, id),
    cancelDownload: (id) => invoke(IPC.modelsCancel, id),
    remove: (id) => invoke(IPC.modelsRemove, id),
    activate: (id) => invoke(IPC.modelsActivate, id),
    refresh: () => invoke(IPC.modelsRefresh),
    onProgress: (cb) => subscribe(IPC.modelsProgress, cb),
    onChanged: (cb) => subscribe(IPC.modelsChanged, cb),
  },
  secrets: {
    set: (provider, key) => invoke(IPC.secretsSet, { provider, key }),
    has: (provider) => invoke(IPC.secretsHas, provider),
    delete: (provider) => invoke(IPC.secretsDelete, provider),
    test: (provider) => invoke(IPC.secretsTest, provider),
  },
  bench: {
    run: (audio, modelIds) => invoke(IPC.benchRun, { audio, modelIds }),
    onResult: (cb) => subscribe(IPC.benchResult, cb),
  },
  hotkeys: {
    capture: () => invoke(IPC.hotkeysCapture),
    cancelCapture: () => invoke(IPC.hotkeysCaptureCancel),
    onCaptureProgress: (cb) => subscribe(IPC.hotkeysCaptureProgress, cb),
  },
  history: {
    search: (query, limit, offset) => invoke(IPC.historySearch, { query, limit, offset }),
    delete: (id) => invoke(IPC.historyDelete, id),
    clear: () => invoke(IPC.historyClear),
    rerun: (id) => invoke(IPC.historyRerun, id),
  },
  dictation: {
    onEvent: (cb) => subscribe(IPC.dictationEvent, cb),
    test: (audio) => invoke(IPC.dictationTest, audio),
  },
  app: {
    info: () => invoke(IPC.appInfo),
    openLogs: () => invoke(IPC.appOpenLogs),
    openModelsFolder: () => invoke(IPC.appOpenModels),
    openExternal: (url) => invoke(IPC.appOpenExternal, url),
    copyText: (text) => invoke(IPC.appCopy, text),
    setPaused: (paused) => invoke(IPC.appSetPaused, paused),
    checkForUpdates: () => invoke(IPC.appCheckUpdates),
  },
  theme: {
    get: () => invoke(IPC.themeGet),
    onChanged: (cb) => subscribe(IPC.themeChanged, cb),
  },
};

export function exposeSettingsApi(): void {
  contextBridge.exposeInMainWorld('flow', api);
}
