// Keep this import first: it configures zod before any schema module loads.
import './lib/zod-setup';
import './styles.css';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';

// In the app the preload defines `window.flow`, and it always wins. Without it
// (a plain browser on the preview server) an in-memory backend stands in. The
// mock is never bundled into a production build of the app, so a broken
// preload can not show made-up settings: `VITE_FLOW_MOCK` is set only by
// `scripts/preview-settings.vite.config.ts`.
const hasBackend = (): boolean => (window as { flow?: unknown }).flow !== undefined;

if (!hasBackend() && (import.meta.env.DEV || import.meta.env.VITE_FLOW_MOCK === '1')) {
  const { installMock } = await import('./mock');
  installMock();
}

const root = createRoot(document.getElementById('root')!);

if (hasBackend()) {
  root.render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
} else {
  document.documentElement.dataset.theme = window.matchMedia('(prefers-color-scheme: dark)').matches
    ? 'dark'
    : 'light';
  root.render(
    <div className="flex h-full flex-col">
      <div className="drag h-10 shrink-0" />
      <p role="alert" className="mx-auto mt-16 max-w-[420px] px-6 text-center text-fg-2">
        Settings could not connect to Flow. Close this window and open Settings again from the tray
        icon.
      </p>
    </div>,
  );
}
