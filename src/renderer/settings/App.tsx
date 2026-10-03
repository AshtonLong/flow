import { useEffect, useRef, useState, type ComponentType } from 'react';
import { Tabs, Tooltip } from 'radix-ui';
import {
  AppWindow,
  AudioLines,
  Boxes,
  History as HistoryIcon,
  Keyboard,
  Settings2,
  SpellCheck,
  Wrench,
  X,
  type LucideProps,
} from 'lucide-react';
import { isCloudModelId } from '@shared/catalog';
import { IconButton } from './components/Button';
import { ErrorBoundary } from './components/ErrorBoundary';
import { IssuesBanner } from './components/IssuesBanner';
import { Notice } from './components/Notice';
import { CatalogProvider, useCatalog } from './lib/catalog';
import { ConfigProvider, useConfig } from './lib/config';
import { isPageId, NavContext, useAppInfo, type PageId } from './lib/hooks';
import { modelLabel } from './lib/models';
import { useApplyTheme, useSystemTheme } from './lib/theme';
import { Onboarding } from './onboarding/Onboarding';
import { Advanced } from './pages/Advanced';
import { Cleanup } from './pages/Cleanup';
import { General } from './pages/General';
import { History } from './pages/History';
import { Hotkeys } from './pages/Hotkeys';
import { Models } from './pages/Models';
import { Profiles } from './pages/Profiles';
import type { ThemeInfo } from '@shared/types';

const NAV: { id: PageId; label: string; icon: ComponentType<LucideProps>; page: ComponentType }[] =
  [
    { id: 'general', label: 'General', icon: Settings2, page: General },
    { id: 'hotkeys', label: 'Hotkeys', icon: Keyboard, page: Hotkeys },
    { id: 'models', label: 'Models', icon: Boxes, page: Models },
    { id: 'cleanup', label: 'Cleanup', icon: SpellCheck, page: Cleanup },
    { id: 'profiles', label: 'Profiles', icon: AppWindow, page: Profiles },
    { id: 'history', label: 'History', icon: HistoryIcon, page: History },
    { id: 'advanced', label: 'Advanced', icon: Wrench, page: Advanced },
  ];

/** The main process can open the window on a page with a URL hash, e.g. `#models`. */
function pageFromHash(): PageId | null {
  const hash = window.location.hash.replace(/^#\/?/, '');
  return isPageId(hash) ? hash : null;
}

function StatusFooter() {
  const { config } = useConfig();
  const { snapshot } = useCatalog();
  const { info, refresh } = useAppInfo();
  const paused = info?.paused ?? false;
  const active = config.model.active;

  const togglePaused = async () => {
    try {
      await window.flow.app.setPaused(!paused);
    } finally {
      refresh();
    }
  };

  return (
    <div className="mx-2 border-t border-line px-2 pt-3 pb-3">
      <div className="flex items-center gap-2 text-caption text-fg-2">
        <span
          aria-hidden="true"
          className={`size-2 shrink-0 rounded-full ${paused ? 'bg-warn' : 'bg-ok'}`}
        />
        {paused
          ? 'Dictation paused'
          : isCloudModelId(active)
            ? 'Active model (cloud)'
            : 'Active model'}
      </div>
      <div className="mt-1 truncate" title={modelLabel(snapshot, active)}>
        {modelLabel(snapshot, active)}
      </div>
      <button type="button" className="link mt-1 text-caption" onClick={() => void togglePaused()}>
        {paused ? 'Resume dictation' : 'Pause dictation'}
      </button>
    </div>
  );
}

function Shell() {
  const { error, dismissError } = useConfig();
  const [page, setPage] = useState<PageId>(() => pageFromHash() ?? 'general');
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onHash = () => {
      const next = pageFromHash();
      if (next) setPage(next);
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  useEffect(() => {
    scroller.current?.scrollTo({ top: 0 });
  }, [page]);

  return (
    <NavContext.Provider value={setPage}>
      <Tabs.Root
        orientation="vertical"
        value={page}
        onValueChange={(next) => {
          if (isPageId(next)) setPage(next);
        }}
        className="grid h-full grid-cols-[220px_minmax(0,1fr)]"
      >
        <div className="flex min-h-0 flex-col">
          <div className="drag flex h-10 shrink-0 items-center gap-2 px-4 text-caption">
            <AudioLines size={14} aria-hidden="true" className="text-accent-text" />
            Flow
          </div>
          <Tabs.List
            aria-label="Settings"
            className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto px-2 pt-2"
          >
            {NAV.map(({ id, label, icon: Icon }) => (
              <Tabs.Trigger key={id} value={id} className="nav-item">
                <Icon size={16} strokeWidth={1.75} aria-hidden="true" className="shrink-0" />
                {label}
              </Tabs.Trigger>
            ))}
          </Tabs.List>
          <StatusFooter />
        </div>

        <div className="flex min-h-0 min-w-0 flex-col">
          {/* Title bar strip: draggable, and clear of the native window buttons on the right. */}
          <div className="drag h-10 shrink-0" />
          <div
            ref={scroller}
            className="relative min-h-0 flex-1 overflow-y-auto rounded-tl-lg border-t border-l border-line bg-layer [scrollbar-gutter:stable]"
          >
            <div className="mx-auto max-w-[760px] px-8 pt-6 pb-12">
              {error && (
                <Notice
                  tone="danger"
                  className="mb-3"
                  action={
                    <IconButton
                      size="sm"
                      label="Dismiss"
                      icon={<X size={14} />}
                      onClick={dismissError}
                    />
                  }
                >
                  {error}
                </Notice>
              )}
              <IssuesBanner />
              {NAV.map(({ id, page: Page }) => (
                <Tabs.Content key={id} value={id} tabIndex={-1} className="outline-none">
                  <ErrorBoundary resetKey={id}>
                    <Page />
                  </ErrorBoundary>
                </Tabs.Content>
              ))}
            </div>
          </div>
        </div>
      </Tabs.Root>
    </NavContext.Provider>
  );
}

function Themed({ system }: { system: ThemeInfo }) {
  const { config } = useConfig();
  useApplyTheme(system, config.general.theme);
  return <ErrorBoundary>{config.general.onboarded ? <Shell /> : <Onboarding />}</ErrorBoundary>;
}

/** Before the config arrives: follow the system theme and say what is wrong, if anything. */
function Boot({ system, error }: { system: ThemeInfo; error: string | null }) {
  useApplyTheme(system, 'system');
  return (
    <div className="flex h-full flex-col">
      <div className="drag h-10 shrink-0" />
      {error && (
        <div className="mx-auto mt-16 w-full max-w-[480px] px-6">
          <Notice tone="danger">Flow could not read its settings: {error}</Notice>
        </div>
      )}
    </div>
  );
}

export function App() {
  const system = useSystemTheme();
  return (
    <Tooltip.Provider delayDuration={500}>
      <ConfigProvider fallback={(error) => <Boot system={system} error={error} />}>
        <CatalogProvider>
          <Themed system={system} />
        </CatalogProvider>
      </ConfigProvider>
    </Tooltip.Provider>
  );
}
