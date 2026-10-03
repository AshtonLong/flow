/** Applies the Windows theme (or the `general.theme` override) to `<html>`. */
import { useEffect, useState } from 'react';
import type { ThemeInfo } from '@shared/types';

function systemFallback(): ThemeInfo {
  return {
    dark: window.matchMedia('(prefers-color-scheme: dark)').matches,
    accent: '#0067c0',
    highContrast: window.matchMedia('(forced-colors: active)').matches,
    reducedMotion: window.matchMedia('(prefers-reduced-motion: reduce)').matches,
  };
}

export function useSystemTheme(): ThemeInfo {
  const [theme, setTheme] = useState<ThemeInfo>(systemFallback);
  useEffect(() => {
    let alive = true;
    window.flow.theme
      .get()
      .then((t) => {
        if (alive) setTheme(t);
      })
      .catch(() => {
        // Keep the media-query fallback.
      });
    const off = window.flow.theme.onChanged(setTheme);
    return () => {
      alive = false;
      off();
    };
  }, []);
  return theme;
}

/**
 * Sets `data-theme`, `data-contrast` and `data-motion` on the root element. The colours are Flow's
 * own (see `common/brand.css`), so the Windows accent colour is not applied.
 */
export function useApplyTheme(system: ThemeInfo, override: 'system' | 'light' | 'dark'): void {
  const dark = override === 'system' ? system.dark : override === 'dark';
  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = dark ? 'dark' : 'light';
    if (system.highContrast) root.dataset.contrast = 'high';
    else delete root.dataset.contrast;
    if (system.reducedMotion) root.dataset.motion = 'reduced';
    else delete root.dataset.motion;
  }, [dark, system.highContrast, system.reducedMotion]);
}
