/**
 * Serves the settings window in a plain browser against the in-memory mock
 * backend (`src/renderer/settings/mock.ts`):
 *
 *   pnpm exec vite --config scripts/preview-settings.vite.config.ts --port 5199
 *   → http://localhost:5199/settings/index.html
 *
 * See the top of mock.ts for query parameters (?theme=dark, ?onboarding, …).
 */
import { resolve } from 'node:path';
import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const root = resolve(__dirname, '../src/renderer');

/**
 * The dev server injects an inline React-refresh script and uses a websocket,
 * both of which the window's strict policy forbids. Relax it while serving
 * only; `vite build` keeps the policy exactly as written in index.html.
 */
function relaxCspInDev(): Plugin {
  return {
    name: 'flow-preview-relax-csp',
    apply: 'serve',
    transformIndexHtml: {
      order: 'pre',
      handler: (html) =>
        html
          .replace("script-src 'self'", "script-src 'self' 'unsafe-inline'")
          .replace("connect-src 'self'", "connect-src 'self' ws:"),
    },
  };
}

export default defineConfig({
  root,
  resolve: { alias: { '@shared': resolve(__dirname, '../src/shared') } },
  plugins: [react(), tailwindcss(), relaxCspInDev()],
  // Lets a production build of the preview load the mock too. The app's own
  // build never sets this, so the mock is not bundled there. Set
  // FLOW_PREVIEW_NO_MOCK=1 to see what the app shows when the preload is missing.
  define: process.env.FLOW_PREVIEW_NO_MOCK
    ? {}
    : { 'import.meta.env.VITE_FLOW_MOCK': JSON.stringify('1') },
  server: { fs: { allow: [resolve(__dirname, '..')] }, strictPort: true },
  build: {
    outDir: resolve(__dirname, '../screenshots/tmp/preview-build'),
    emptyOutDir: true,
    // The capture worklet is under Vite's 4 kB inline limit, and a `data:` URL
    // is refused by `script-src 'self'`. Emit it as a file instead.
    assetsInlineLimit: 0,
    rollupOptions: { input: { settings: resolve(root, 'settings/index.html') } },
  },
});
