/**
 * The config store. `config.toml` is the source of truth: every edit is sent to
 * the main process at once and the window re-renders from the snapshot that
 * comes back, or from `onChanged` when the file is edited elsewhere.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import {
  validateConfig,
  type Config,
  type ConfigPatch,
  type ConfigPath,
  type ConfigSnapshot,
} from '@shared/config';
import { errorMessage } from './format';
import { applyPatches } from './patch';

export interface ConfigStore {
  snapshot: ConfigSnapshot;
  config: Config;
  /** Writes one value. `undefined` removes the key so it falls back to its default. */
  set(path: ConfigPath, value: unknown): void;
  /** Writes several values in one file update. */
  patch(patches: ConfigPatch[]): Promise<void>;
  /** The last write error, cleared by the next successful write. */
  error: string | null;
  dismissError(): void;
}

const ConfigContext = createContext<ConfigStore | null>(null);

export function ConfigProvider({
  children,
  fallback,
}: {
  children: ReactNode;
  /** Shown until the first snapshot arrives. */
  fallback?: (loadError: string | null) => ReactNode;
}) {
  const [snapshot, setSnapshot] = useState<ConfigSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Writes in flight. While any are pending the optimistic state wins, and only
  // the reply to the newest write is applied, so the UI never flickers back.
  const pending = useRef(0);
  const sequence = useRef(0);

  useEffect(() => {
    let alive = true;
    window.flow.config
      .get()
      .then((next) => {
        if (alive && pending.current === 0) setSnapshot(next);
      })
      .catch((err: unknown) => {
        if (alive) setError(errorMessage(err));
      });
    const off = window.flow.config.onChanged((next) => {
      if (pending.current === 0) setSnapshot(next);
    });
    return () => {
      alive = false;
      off();
    };
  }, []);

  const patch = useCallback(async (patches: ConfigPatch[]) => {
    if (patches.length === 0) return;
    const id = ++sequence.current;
    pending.current++;
    // Optimistic: patch the current config and re-validate it, so defaults are
    // filled in exactly as the main process will fill them.
    setSnapshot(
      (prev) =>
        prev && { ...prev, config: validateConfig(applyPatches(prev.config, patches)).config },
    );
    try {
      const next = await window.flow.config.set(patches);
      pending.current--;
      if (id === sequence.current) setSnapshot(next);
      setError(null);
    } catch (err) {
      pending.current--;
      setError(`The change was not saved: ${errorMessage(err)}`);
      if (pending.current === 0) {
        try {
          setSnapshot(await window.flow.config.get());
        } catch {
          // Keep showing the optimistic state; the error banner is already up.
        }
      }
    }
  }, []);

  const set = useCallback(
    (path: ConfigPath, value: unknown) => {
      void patch([{ path, value }]);
    },
    [patch],
  );

  const dismissError = useCallback(() => setError(null), []);

  const store = useMemo<ConfigStore | null>(
    () =>
      snapshot ? { snapshot, config: snapshot.config, set, patch, error, dismissError } : null,
    [snapshot, set, patch, error, dismissError],
  );

  if (!store) return <>{fallback?.(error) ?? null}</>;
  return <ConfigContext.Provider value={store}>{children}</ConfigContext.Provider>;
}

export function useConfig(): ConfigStore {
  const store = useContext(ConfigContext);
  if (!store) throw new Error('useConfig must be used inside ConfigProvider');
  return store;
}
