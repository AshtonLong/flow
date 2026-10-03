/** The tray icon and its menu: model switcher, pause, copy last, settings, quit. */
import { Menu, Tray, nativeImage, type MenuItemConstructorOptions } from 'electron';
import { resourcePath } from './paths';

export interface TrayModel {
  id: string;
  name: string;
  cloud: boolean;
}

export interface TrayState {
  activeModelId: string;
  /** Models that can be switched to right now (installed, or connected). */
  models: TrayModel[];
  paused: boolean;
  hasLast: boolean;
  canRetry: boolean;
  /** Set when the global hook could not start, so hotkeys do nothing. */
  hookFailed: boolean;
}

export interface TrayActions {
  activateModel(id: string): void;
  setPaused(paused: boolean): void;
  copyLast(): void;
  retryLast(): void;
  openSettings(): void;
  quit(): void;
}

export class AppTray {
  private tray: Tray | null = null;

  constructor(private readonly actions: TrayActions) {}

  create(): void {
    if (this.tray) return;
    this.tray = new Tray(this.icon(false));
    this.tray.on('double-click', () => this.actions.openSettings());
    // A left click opens the same menu as a right click.
    this.tray.on('click', () => this.tray?.popUpContextMenu());
  }

  private icon(paused: boolean) {
    const image = nativeImage.createFromPath(
      resourcePath('icons', paused ? 'tray-paused.png' : 'tray.png'),
    );
    return image.isEmpty() ? nativeImage.createEmpty() : image;
  }

  update(state: TrayState): void {
    const tray = this.tray;
    if (!tray) return;
    const active = state.models.find((m) => m.id === state.activeModelId);
    const activeName = active?.name ?? state.activeModelId;
    const local = state.models.filter((m) => !m.cloud);
    const cloud = state.models.filter((m) => m.cloud);
    const item = (m: TrayModel): MenuItemConstructorOptions => ({
      label: m.name,
      type: 'radio',
      checked: m.id === state.activeModelId,
      click: () => this.actions.activateModel(m.id),
    });
    const switcher: MenuItemConstructorOptions[] = [
      ...local.map(item),
      ...(local.length > 0 && cloud.length > 0 ? [{ type: 'separator' as const }] : []),
      ...cloud.map(item),
    ];
    if (switcher.length === 0) {
      switcher.push({ label: 'No models are ready — open Settings', enabled: false });
    }

    const menu = Menu.buildFromTemplate([
      { label: `Model: ${activeName}`, submenu: switcher },
      { type: 'separator' },
      {
        label: 'Pause dictation',
        type: 'checkbox',
        checked: state.paused,
        click: (menuItem) => this.actions.setPaused(menuItem.checked),
      },
      {
        label: 'Copy last transcript',
        enabled: state.hasLast,
        click: () => this.actions.copyLast(),
      },
      {
        label: 'Retry last dictation',
        enabled: state.canRetry,
        click: () => this.actions.retryLast(),
      },
      { type: 'separator' },
      ...(state.hookFailed
        ? [{ label: 'Hotkeys are unavailable (input hook failed)', enabled: false }]
        : []),
      { label: 'Settings…', click: () => this.actions.openSettings() },
      { label: 'Quit Flow', click: () => this.actions.quit() },
    ]);
    tray.setContextMenu(menu);
    tray.setImage(this.icon(state.paused));
    tray.setToolTip(state.paused ? 'Flow — paused' : `Flow — ${activeName}`);
  }

  destroy(): void {
    this.tray?.destroy();
    this.tray = null;
  }
}
