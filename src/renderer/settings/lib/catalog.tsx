/** The model catalog store: one snapshot, live download progress, shared by every page. */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import type { CatalogSnapshot, DownloadProgress } from '@shared/catalog';
import { errorMessage } from './format';

export interface CatalogStore {
  /** Null until the first `models.list()` resolves. */
  snapshot: CatalogSnapshot | null;
  /** Latest progress per model id, including finished `error` states until retried. */
  progress: Record<string, DownloadProgress>;
  error: string | null;
  /** Re-reads the catalog, e.g. after a key was stored. */
  reload(): Promise<void>;
  clearProgress(id: string): void;
}

const CatalogContext = createContext<CatalogStore | null>(null);

export function CatalogProvider({ children }: { children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<CatalogSnapshot | null>(null);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setSnapshot(await window.flow.models.list());
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const clearProgress = useCallback((id: string) => {
    setProgress((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, []);

  useEffect(() => {
    void reload();
    const offChanged = window.flow.models.onChanged((next) => {
      setSnapshot(next);
      // A finished download shows up as `ready`; drop its progress entry.
      setProgress((prev) => {
        const kept = Object.entries(prev).filter(
          ([id, p]) => p.state === 'error' && !next.status[id]?.ready,
        );
        return kept.length === Object.keys(prev).length ? prev : Object.fromEntries(kept);
      });
    });
    const offProgress = window.flow.models.onProgress((p) => {
      setProgress((prev) => {
        if (p.state === 'done' || p.state === 'cancelled') {
          if (!(p.id in prev)) return prev;
          const next = { ...prev };
          delete next[p.id];
          return next;
        }
        return { ...prev, [p.id]: p };
      });
    });
    return () => {
      offChanged();
      offProgress();
    };
  }, [reload]);

  const store = useMemo(
    () => ({ snapshot, progress, error, reload, clearProgress }),
    [snapshot, progress, error, reload, clearProgress],
  );
  return <CatalogContext.Provider value={store}>{children}</CatalogContext.Provider>;
}

export function useCatalog(): CatalogStore {
  const store = useContext(CatalogContext);
  if (!store) throw new Error('useCatalog must be used inside CatalogProvider');
  return store;
}

/** The download in flight for a model, from live progress or the snapshot. */
export function downloadOf(store: CatalogStore, id: string): DownloadProgress | undefined {
  const live = store.progress[id];
  if (live) return live;
  const fromSnapshot = store.snapshot?.status[id]?.download;
  if (
    fromSnapshot &&
    (fromSnapshot.state === 'downloading' || fromSnapshot.state === 'verifying')
  ) {
    return fromSnapshot;
  }
  return undefined;
}
