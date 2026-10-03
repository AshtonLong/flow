import { defineConfig } from '@playwright/test';

// End-to-end tests launch the built app (`pnpm build` first) with Playwright's Electron driver.
export default defineConfig({
  testDir: 'tests/e2e',
  timeout: 120_000,
  workers: 1,
  reporter: [['list']],
  use: { trace: 'retain-on-failure' },
});
