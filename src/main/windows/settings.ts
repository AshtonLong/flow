/** The settings window. It exists only while open: created on demand, destroyed on close. */
import { BrowserWindow, shell } from 'electron';
import type { ThemeInfo } from '@shared/types';
import { isOwnUrl, loadRenderer, preloadArguments, preloadPath, resourcePath } from './paths';

export class SettingsWindow {
  private win: BrowserWindow | null = null;

  constructor(private readonly getTheme: () => ThemeInfo & { effectiveDark: boolean }) {}

  get isOpen(): boolean {
    return this.win !== null && !this.win.isDestroyed();
  }

  get webContentsId(): number | null {
    return this.isOpen ? this.win!.webContents.id : null;
  }

  get browserWindow(): BrowserWindow | null {
    return this.isOpen ? this.win : null;
  }

  private overlayColors(): { color: string; symbolColor: string } {
    const dark = this.getTheme().effectiveDark;
    return { color: '#00000000', symbolColor: dark ? '#ffffff' : '#1b1b1b' };
  }

  open(): void {
    if (this.isOpen) {
      const win = this.win!;
      if (win.isMinimized()) win.restore();
      win.show();
      win.focus();
      return;
    }
    const theme = this.getTheme();
    const win = new BrowserWindow({
      width: 1000,
      height: 700,
      minWidth: 860,
      minHeight: 560,
      show: false,
      title: 'Flow',
      icon: resourcePath('icons', 'icon.png'),
      autoHideMenuBar: true,
      titleBarStyle: 'hidden',
      titleBarOverlay: { ...this.overlayColors(), height: 40 },
      // Mica needs a see-through client area; high contrast gets a solid one.
      backgroundMaterial: theme.highContrast ? 'none' : 'mica',
      backgroundColor: theme.highContrast
        ? theme.effectiveDark
          ? '#000000'
          : '#ffffff'
        : '#00000000',
      webPreferences: {
        preload: preloadPath(),
        additionalArguments: preloadArguments('settings'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
        spellcheck: false,
      },
    });
    this.win = win;
    win.removeMenu();
    win.once('ready-to-show', () => win.show());
    win.on('closed', () => {
      if (this.win === win) this.win = null;
    });
    // No remote content, ever: outside links open in the browser, nothing navigates away.
    win.webContents.setWindowOpenHandler(({ url }) => {
      if (url.startsWith('https://')) void shell.openExternal(url);
      return { action: 'deny' };
    });
    win.webContents.on('will-navigate', (event, url) => {
      if (!isOwnUrl(url)) event.preventDefault();
    });
    loadRenderer(win, 'settings');
  }

  /** Keeps the native caption buttons readable when the theme changes. */
  refreshTheme(): void {
    if (!this.isOpen) return;
    try {
      this.win!.setTitleBarOverlay(this.overlayColors());
    } catch {
      // Not supported on this platform.
    }
  }

  send(channel: string, payload?: unknown): void {
    if (this.isOpen) this.win!.webContents.send(channel, payload);
  }

  close(): void {
    if (this.isOpen) this.win!.close();
  }
}
