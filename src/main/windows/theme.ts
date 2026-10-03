/** The Windows theme, accent colour, high-contrast and reduced-motion settings. */
import { nativeTheme, systemPreferences } from 'electron';
import type { Config } from '@shared/config';
import type { ThemeInfo } from '@shared/types';

const FALLBACK_ACCENT = '#0067c0';

function accentColor(): string {
  try {
    // `rrggbbaa` on Windows.
    const raw = systemPreferences.getAccentColor();
    return /^[0-9a-f]{6}/i.test(raw) ? `#${raw.slice(0, 6)}` : FALLBACK_ACCENT;
  } catch {
    return FALLBACK_ACCENT;
  }
}

function reducedMotion(): boolean {
  try {
    return !systemPreferences.getAnimationSettings().shouldRenderRichAnimation;
  } catch {
    return false;
  }
}

/** The system theme. The renderer applies `general.theme` on top of this. */
export function systemTheme(): ThemeInfo {
  return {
    dark: nativeTheme.shouldUseDarkColors,
    accent: accentColor(),
    highContrast: nativeTheme.shouldUseHighContrastColors,
    reducedMotion: reducedMotion(),
  };
}

/** Makes native surfaces (title bar, menus, Mica) follow `general.theme`. */
export function applyThemeSource(theme: Config['general']['theme']): void {
  nativeTheme.themeSource = theme;
}

export function onThemeChanged(listener: (theme: ThemeInfo) => void): void {
  const notify = () => listener(systemTheme());
  nativeTheme.on('updated', notify);
  // Accent changes do not trigger nativeTheme's event.
  (systemPreferences as unknown as NodeJS.EventEmitter).on('accent-color-changed', notify);
}
