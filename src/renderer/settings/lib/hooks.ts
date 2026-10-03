/** Small shared hooks. */
import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react';
import type { AppInfo } from '@shared/types';
import { listMicrophones, type Microphone } from '../../common/capture';

export function useDebounced<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return debounced;
}

/** `app.info()`, refreshed when the window regains focus (paused can change from the tray). */
export function useAppInfo(): { info: AppInfo | null; refresh: () => void } {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const refresh = useCallback(() => {
    window.flow.app
      .info()
      .then(setInfo)
      .catch(() => {
        // The pages degrade to "unknown" rather than failing.
      });
  }, []);
  useEffect(() => {
    refresh();
    window.addEventListener('focus', refresh);
    return () => window.removeEventListener('focus', refresh);
  }, [refresh]);
  return { info, refresh };
}

/** Connected microphones, refreshed on hot-plug and on demand (labels need permission first). */
export function useMicrophones(): { mics: Microphone[]; refresh: () => void } {
  const [mics, setMics] = useState<Microphone[]>([]);
  const refresh = useCallback(() => {
    listMicrophones()
      .then(setMics)
      .catch(() => setMics([]));
  }, []);
  useEffect(() => {
    refresh();
    const devices = navigator.mediaDevices;
    devices?.addEventListener('devicechange', refresh);
    return () => devices?.removeEventListener('devicechange', refresh);
  }, [refresh]);
  return { mics, refresh };
}

/** A flag that turns itself off, for "Copied" style confirmations. */
export function useFlash(durationMs = 1600): [boolean, () => void] {
  const [on, setOn] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  const flash = useCallback(() => {
    setOn(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setOn(false), durationMs);
  }, [durationMs]);
  return [on, flash];
}

export const PAGES = [
  'general',
  'hotkeys',
  'models',
  'cleanup',
  'profiles',
  'history',
  'advanced',
] as const;
export type PageId = (typeof PAGES)[number];

export function isPageId(value: string): value is PageId {
  return (PAGES as readonly string[]).includes(value);
}

/** Lets a page send the user to another page ("Add a key on the Models page"). */
export const NavContext = createContext<(page: PageId) => void>(() => {});

export function useGoTo(): (page: PageId) => void {
  return useContext(NavContext);
}
