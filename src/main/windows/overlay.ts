/**
 * The overlay window: frameless, transparent, always on top, never focused.
 * It is created at launch and kept hidden so it appears instantly, and it
 * hosts microphone capture even while invisible.
 */
import { BrowserWindow, screen } from 'electron';
import type { Config } from '@shared/config';
import { IPC } from '@shared/ipc';
import type { CaptureStart, OverlayAppearance, OverlayState, ThemeInfo } from '@shared/types';
import { log } from '../log';
import { loadRenderer, preloadArguments, preloadPath } from './paths';

const WIDTH = 460;
const HEIGHT = 84;
const MARGIN = 28;
/** Lets the fade-out finish before the window is hidden. */
const HIDE_DELAY_MS = 220;

export class OverlayWindow {
  private win: BrowserWindow | null = null;
  private ready = false;
  private readonly pending: Array<() => void> = [];
  private hideTimer: NodeJS.Timeout | null = null;
  private appearance: OverlayAppearance | null = null;
  private position: Config['overlay']['position'] = 'bottom-center';
  private style: Config['overlay']['style'] = 'pill';
  private state: OverlayState = { phase: 'hidden', cloud: false };

  create(): void {
    if (this.win) return;
    const win = new BrowserWindow({
      width: WIDTH,
      height: HEIGHT,
      show: false,
      frame: false,
      transparent: true,
      resizable: false,
      movable: false,
      minimizable: false,
      maximizable: false,
      fullscreenable: false,
      focusable: false,
      skipTaskbar: true,
      hasShadow: false,
      alwaysOnTop: true,
      title: 'Flow overlay',
      webPreferences: {
        preload: preloadPath(),
        additionalArguments: preloadArguments('overlay'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
        // Capture must keep running while the window is hidden.
        backgroundThrottling: false,
      },
    });
    this.win = win;
    // Above full-screen apps, and click-through so it cannot interrupt typing.
    win.setAlwaysOnTop(true, 'screen-saver');
    win.setIgnoreMouseEvents(true);
    win.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
    win.webContents.on('render-process-gone', (_event, details) => {
      log.error('overlay', `renderer gone: ${details.reason}`);
      this.ready = false;
      this.win = null;
      win.destroy();
      // Capture lives here, so the window must come back.
      this.create();
    });
    win.on('closed', () => {
      if (this.win === win) {
        this.win = null;
        this.ready = false;
      }
    });
    loadRenderer(win, 'overlay');
  }

  /** Called when the renderer reports it has mounted. */
  markReady(): void {
    this.ready = true;
    if (this.appearance) this.send(IPC.overlayAppearance, this.appearance);
    for (const run of this.pending.splice(0)) run();
  }

  private send(channel: string, payload?: unknown): void {
    const win = this.win;
    if (!win || win.isDestroyed()) return;
    const run = () => {
      if (!win.isDestroyed()) win.webContents.send(channel, payload);
    };
    if (this.ready) run();
    else this.pending.push(run);
  }

  configure(config: Config, theme: ThemeInfo): void {
    this.position = config.overlay.position;
    this.style = config.overlay.style;
    this.appearance = {
      style: config.overlay.style,
      size: config.overlay.size,
      showWaveform: config.overlay.show_waveform,
      sounds: config.audio.sounds,
      theme,
    };
    this.send(IPC.overlayAppearance, this.appearance);
    if (this.style === 'none') this.hideNow();
  }

  startCapture(options: CaptureStart): void {
    this.send(IPC.captureStart, options);
  }

  stopCapture(): void {
    this.send(IPC.captureStop);
  }

  setState(state: OverlayState): void {
    const changed =
      state.phase !== this.state.phase ||
      state.cloud !== this.state.cloud ||
      state.locked !== this.state.locked ||
      state.message !== this.state.message ||
      state.queued !== this.state.queued;
    if (!changed) return;
    this.state = state;
    this.send(IPC.overlayState, state);
    const win = this.win;
    if (!win || win.isDestroyed()) return;
    if (state.phase === 'hidden') {
      if (!this.hideTimer) this.hideTimer = setTimeout(() => this.hideNow(), HIDE_DELAY_MS);
      return;
    }
    if (this.hideTimer) clearTimeout(this.hideTimer);
    this.hideTimer = null;
    if (this.style === 'none') return;
    if (!win.isVisible()) {
      this.place();
      win.showInactive();
      // Some full-screen windows push other topmost windows down; reassert.
      win.setAlwaysOnTop(true, 'screen-saver');
    }
  }

  private hideNow(): void {
    if (this.hideTimer) clearTimeout(this.hideTimer);
    this.hideTimer = null;
    if (this.win && !this.win.isDestroyed() && this.win.isVisible()) this.win.hide();
  }

  /** Positions the window on the display the user is working on. */
  private place(): void {
    const win = this.win;
    if (!win) return;
    const display = screen.getDisplayNearestPoint(screen.getCursorScreenPoint());
    const area = display.workArea;
    const [vertical, horizontal] = this.position.split('-') as [string, string];
    const x =
      horizontal === 'left'
        ? area.x + MARGIN
        : horizontal === 'right'
          ? area.x + area.width - WIDTH - MARGIN
          : area.x + Math.round((area.width - WIDTH) / 2);
    const y = vertical === 'top' ? area.y + MARGIN : area.y + area.height - HEIGHT - MARGIN;
    win.setBounds({ x, y, width: WIDTH, height: HEIGHT });
  }

  get webContentsId(): number | null {
    return this.win && !this.win.isDestroyed() ? this.win.webContents.id : null;
  }

  get browserWindow(): BrowserWindow | null {
    return this.win;
  }

  destroy(): void {
    this.hideNow();
    this.win?.destroy();
    this.win = null;
  }
}
