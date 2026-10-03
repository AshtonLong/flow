/** Where the bundled preloads, renderers and resources live, in dev and when packaged. */
import path from 'node:path';
import { app, type BrowserWindow } from 'electron';

const here = import.meta.dirname;

export function preloadPath(): string {
  return path.join(here, '../preload/index.cjs');
}

/** Tells the shared preload which window it is in, so it exposes only that window's API. */
export function preloadArguments(name: 'overlay' | 'settings'): string[] {
  return [`--flow-window=${name}`];
}

/** Loads a renderer from the dev server when one is running, else from the built files. */
export function loadRenderer(win: BrowserWindow, name: 'overlay' | 'settings'): void {
  const devServer = process.env.ELECTRON_RENDERER_URL;
  if (!app.isPackaged && devServer) {
    void win.loadURL(`${devServer}/${name}/index.html`);
  } else {
    void win.loadFile(path.join(here, '../renderer', name, 'index.html'));
  }
}

/** Origin prefixes our own renderers are served from; used to validate IPC senders. */
export function isOwnUrl(url: string): boolean {
  const devServer = process.env.ELECTRON_RENDERER_URL;
  if (!app.isPackaged && devServer && url.startsWith(devServer)) return true;
  if (!url.startsWith('file://')) return false;
  const rendererDir = path.join(here, '../renderer').replace(/\\/g, '/').toLowerCase();
  return decodeURIComponent(url).replace(/\\/g, '/').toLowerCase().includes(rendererDir);
}

/** `resources/` in the repo during development, `<install>/resources/resources` when packaged. */
export function resourcePath(...parts: string[]): string {
  const base = app.isPackaged
    ? path.join(process.resourcesPath, 'resources')
    : path.join(here, '../../resources');
  return path.join(base, ...parts);
}

export function enginePath(): string {
  return path.join(here, 'engine.js');
}

/** The unpacked native speech library in a packaged build; undefined in development. */
export function packagedLibraryPath(): string | undefined {
  if (!app.isPackaged) return undefined;
  return path.join(
    process.resourcesPath,
    'app.asar.unpacked',
    'node_modules',
    '@transcribe-cpp',
    'win32-x64-cpu-vulkan',
    'transcribe.dll',
  );
}
