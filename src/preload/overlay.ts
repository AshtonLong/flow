/** Preload for the overlay window: capture control in, PCM audio out. */
import { contextBridge, ipcRenderer, type IpcRendererEvent } from 'electron';
import { IPC, type OverlayApi } from '@shared/ipc';

function subscribe<T>(channel: string, cb: (payload: T) => void): () => void {
  const listener = (_event: IpcRendererEvent, payload: T) => cb(payload);
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

const api: OverlayApi = {
  onCaptureStart: (cb) => subscribe(IPC.captureStart, cb),
  onCaptureStop: (cb) => subscribe<void>(IPC.captureStop, () => cb()),
  onState: (cb) => subscribe(IPC.overlayState, cb),
  onAppearance: (cb) => subscribe(IPC.overlayAppearance, cb),
  captureStarted: () => ipcRenderer.send(IPC.captureStarted),
  captureChunk: (pcm) => ipcRenderer.send(IPC.captureChunk, pcm),
  captureError: (reason, message) => ipcRenderer.send(IPC.captureError, { reason, message }),
  captureStopped: () => ipcRenderer.send(IPC.captureStopped),
  ready: () => ipcRenderer.send(IPC.overlayReady),
};

export function exposeOverlayApi(): void {
  contextBridge.exposeInMainWorld('flowOverlay', api);
}
