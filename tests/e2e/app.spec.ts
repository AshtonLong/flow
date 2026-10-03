/**
 * End-to-end tests against the built app (`pnpm build` first). They use the
 * real main process, engine, config file and windows; the microphone is
 * Chromium's fake capture device playing a WAV file, and hotkeys are injected
 * into the session instead of being pressed on the live desktop.
 */
import fs from 'node:fs';
import path from 'node:path';
import { expect, test, type Page } from '@playwright/test';
import { launch, readWav, ROOT, type Launched } from './launch';

const SHOTS = path.join(ROOT, 'screenshots');
const JFK = path.join(ROOT, 'tests/fixtures/jfk.wav');
const ONBOARDED = 'version = 1\n\n[general]\nonboarded = true\n';

const modelsDir = path.join(process.env.LOCALAPPDATA ?? '', 'Flow', 'models');
const hasParakeet = fs.existsSync(path.join(modelsDir, 'parakeet-tdt-0.6b-v2-Q4_K_M.gguf'));

let flow: Launched | null = null;

test.afterEach(async () => {
  await flow?.close();
  flow = null;
});

/** Screenshots capture only web content, so stand in for the Mica backdrop. */
async function shot(page: Page, name: string): Promise<void> {
  fs.mkdirSync(SHOTS, { recursive: true });
  await page.evaluate(() => document.documentElement.setAttribute('data-preview', ''));
  await page.waitForTimeout(250);
  await page.screenshot({ path: path.join(SHOTS, `${name}.png`) });
  await page.evaluate(() => document.documentElement.removeAttribute('data-preview'));
}

const tab = (page: Page, name: string) => page.getByRole('tab', { name, exact: true });

test('first run opens the three-step setup', async () => {
  flow = await launch();
  const settings = await flow.window('settings');
  await expect(settings.getByRole('list', { name: 'Setup steps' })).toBeVisible();
  await expect(settings.getByRole('tab', { name: 'General' })).toHaveCount(0);
  await shot(settings, 'first-run');
  // The config file was created with defaults and a commented header.
  const file = fs.readFileSync(path.join(flow.userData, 'config.toml'), 'utf8');
  expect(file).toContain('version = 1');
  expect(file).toMatch(/^#/);
});

test('starts in the tray with no window once set up', async () => {
  flow = await launch({ config: ONBOARDED });
  await flow.window('overlay');
  await new Promise((resolve) => setTimeout(resolve, 1500));
  const urls = flow.app.windows().map((page) => page.url());
  expect(urls.some((url) => url.includes('/settings/'))).toBe(false);
  // The overlay exists but is hidden while idle.
  const visible = await flow.app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows().map((win) => win.isVisible()),
  );
  expect(visible).toEqual([false]);
});

test('every settings page renders against the real backend', async () => {
  flow = await launch({ config: ONBOARDED, args: ['--settings'] });
  const settings = await flow.window('settings');
  const errors: string[] = [];
  settings.on('pageerror', (err) => errors.push(String(err)));
  settings.on('console', (msg) => {
    if (msg.type() === 'error') errors.push(msg.text());
  });

  const pages = ['General', 'Hotkeys', 'Models', 'Cleanup', 'Profiles', 'History', 'Advanced'];
  for (const name of pages) {
    await tab(settings, name).click();
    await expect(settings.getByRole('heading', { name, level: 1 })).toBeVisible();
    await settings.waitForTimeout(400);
    await shot(settings, `settings-${name.toLowerCase()}`);
  }

  // The real catalog reached the Models page.
  await tab(settings, 'Models').click();
  await expect(settings.getByText('Parakeet TDT 0.6B v2').first()).toBeVisible();
  await expect(settings.getByText('IBM Granite Speech 4.1 2B').first()).toBeVisible();
  await expect(settings.getByText(/Whisper large-v3-turbo/).first()).toBeVisible();

  // Renderers are sandboxed: no Node.js, only the typed preload API.
  const exposure = await settings.evaluate(() => ({
    flow: typeof window.flow,
    overlayApi: typeof (window as unknown as Record<string, unknown>).flowOverlay,
    require: typeof (window as unknown as Record<string, unknown>).require,
    process: typeof (window as unknown as Record<string, unknown>).process,
  }));
  expect(exposure).toEqual({
    flow: 'object',
    overlayApi: 'undefined',
    require: 'undefined',
    process: 'undefined',
  });
  expect(errors).toEqual([]);
});

test('follows the theme set in the config file', async () => {
  flow = await launch({
    config: `${ONBOARDED}theme = "light"
`,
    args: ['--settings'],
  });
  const settings = await flow.window('settings');
  await tab(settings, 'Models').click();
  await expect(settings.getByRole('heading', { name: 'Models', level: 1 })).toBeVisible();
  await expect(settings.locator('html')).toHaveAttribute('data-theme', 'light');
  await settings.waitForTimeout(400);
  await shot(settings, 'settings-models-light');
});

test('settings edits patch config.toml in place and file edits hot-reload', async () => {
  const config = [
    '# my settings',
    'version = 1',
    '',
    '[general]',
    'onboarded = true',
    '',
    '[audio]',
    'sounds = true   # the beeps',
    '',
  ].join('\n');
  flow = await launch({ config, args: ['--settings'] });
  const settings = await flow.window('settings');
  const file = path.join(flow.userData, 'config.toml');
  await expect(tab(settings, 'General')).toBeVisible();

  // UI → file: only the value changes; comments and layout survive.
  const sounds = settings.getByRole('switch', { name: 'Sounds' });
  await expect(sounds).toBeChecked();
  await sounds.click();
  await expect.poll(() => fs.readFileSync(file, 'utf8')).toContain('sounds = false   # the beeps');
  expect(fs.readFileSync(file, 'utf8')).toContain('# my settings');

  // File → UI: an edit in a text editor applies within a second, with no restart.
  fs.writeFileSync(file, fs.readFileSync(file, 'utf8').replace('sounds = false', 'sounds = true'));
  await expect(sounds).toBeChecked({ timeout: 2000 });

  // A bad value falls back to its default and is reported with its line.
  fs.appendFileSync(file, '\n[overlay]\nstyle = "banner"\n');
  await expect(settings.getByText(/config\.toml has 1 problem/)).toBeVisible({ timeout: 2000 });
  const snapshot = await settings.evaluate(() => window.flow.config.get());
  expect(snapshot.config.overlay.style).toBe('pill');
  expect(snapshot.issues[0]).toMatchObject({ path: ['overlay', 'style'], line: 11 });
});

test('API keys are encrypted and never written to the config file', async () => {
  flow = await launch({ config: ONBOARDED, args: ['--settings'] });
  const settings = await flow.window('settings');
  await expect(tab(settings, 'General')).toBeVisible();
  const key = 'gsk_test_not_a_real_key_123456';
  await settings.evaluate((value) => window.flow.secrets.set('groq', value), key);
  expect(await settings.evaluate(() => window.flow.secrets.has('groq'))).toBe(true);

  const file = fs.readFileSync(path.join(flow.userData, 'config.toml'), 'utf8');
  expect(file).toContain('api_key = "secret:groq"');
  expect(file).not.toContain(key);
  const secrets = fs.readFileSync(path.join(flow.userData, 'secrets.bin'));
  expect(secrets.includes(Buffer.from(key))).toBe(false);

  const catalog = await settings.evaluate(() => window.flow.models.list());
  expect(catalog.keys.groq).toBe(true);
  expect(catalog.status['groq/whisper-large-v3-turbo']?.ready).toBe(true);
  expect(JSON.stringify(catalog)).not.toContain(key);

  await settings.evaluate(() => window.flow.secrets.delete('groq'));
  expect(await settings.evaluate(() => window.flow.secrets.has('groq'))).toBe(false);
});

test('the overlay shows each state without taking focus', async () => {
  flow = await launch({ config: ONBOARDED });
  const overlay = await flow.window('overlay');
  await overlay.waitForFunction(() => document.getElementById('stage') !== null);
  const stage = overlay.locator('#stage');
  await expect(stage).toHaveAttribute('data-phase', 'hidden');

  const options = await flow.app.evaluate(({ BrowserWindow }) => {
    const win = BrowserWindow.getAllWindows()[0]!;
    return { focusable: win.isFocusable(), onTop: win.isAlwaysOnTop(), visible: win.isVisible() };
  });
  expect(options).toEqual({ focusable: false, onTop: true, visible: false });

  const show = async (state: Record<string, unknown>) => {
    await flow!.app.evaluate(({ BrowserWindow }, s) => {
      BrowserWindow.getAllWindows()[0]!.webContents.send('overlay:state', s);
    }, state);
    await expect(stage).toHaveAttribute('data-phase', String(state.phase));
  };
  // A desktop-like backdrop so the translucent pill is visible in the screenshot.
  await overlay.addStyleTag({
    content: 'body{background:linear-gradient(135deg,#27405f,#5b3f6b 55%,#94564c)!important}',
  });
  await show({ phase: 'transcribing', cloud: true });
  await expect(overlay.locator('#cloud')).toBeVisible();
  await overlay.screenshot({ path: path.join(SHOTS, 'overlay-transcribing.png') });
  await show({ phase: 'error', cloud: false, message: 'Groq rejected the API key' });
  await expect(overlay.locator('#label')).toHaveText('Groq rejected the API key');
  await overlay.screenshot({ path: path.join(SHOTS, 'overlay-error.png') });
  await show({
    phase: 'notice',
    cloud: false,
    message: 'No text field focused — copied to clipboard',
  });
  await overlay.screenshot({ path: path.join(SHOTS, 'overlay-notice.png') });
  await show({ phase: 'done', cloud: false });
  await expect(overlay.locator('#label')).toHaveText('Typed');
  await overlay.screenshot({ path: path.join(SHOTS, 'overlay-done.png') });
  await show({ phase: 'hidden', cloud: false });
});

test.describe('with the local model installed', () => {
  test.skip(!hasParakeet, 'Parakeet is not downloaded on this machine');

  test('transcribes a recorded phrase with the local engine', async () => {
    flow = await launch({ config: ONBOARDED, args: ['--settings'] });
    const settings = await flow.window('settings');
    await expect(tab(settings, 'General')).toBeVisible();
    const samples = readWav(JFK);
    const transcript = await settings.evaluate(
      (pcm) => window.flow.dictation.test(new Float32Array(pcm)),
      samples,
    );
    expect(transcript.model).toBe('parakeet-tdt-0.6b-v2');
    expect(transcript.text).toMatch(/ask not what your country can do for you/i);
    // The budget is one second for a 10 s clip once the model is loaded; a cold load is included here.
    expect(transcript.elapsedMs).toBeLessThan(15_000);

    const info = await settings.evaluate(() => window.flow.app.info());
    expect(info.devices.length).toBeGreaterThan(0);
    expect(info.degraded).toEqual([]);

    // The test bench runs the same phrase through chosen models side by side.
    const result = await settings.evaluate(
      (pcm) =>
        new Promise<{ ok: boolean; text?: string; elapsedMs?: number }>((resolve) => {
          const off = window.flow.bench.onResult((r) => {
            if (r.done) {
              off();
              resolve(r);
            }
          });
          void window.flow.bench.run(new Float32Array(pcm), ['parakeet-tdt-0.6b-v2']);
        }),
      samples,
    );
    expect(result.ok).toBe(true);
    expect(result.text).toMatch(/fellow Americans/i);
    // The model is already loaded for this second run.
    console.log(`warm Parakeet: ${Math.round(result.elapsedMs ?? 0)} ms for an 11 s clip`);
  });

  test('hold to talk types the transcript into the focused field', async () => {
    flow = await launch({
      config: `${ONBOARDED}\n[history]\nenabled = true\n`,
      args: [
        '--settings',
        '--use-fake-device-for-media-stream',
        `--use-file-for-fake-audio-capture=${JFK}%noloop`,
      ],
      test: true,
    });
    const { app } = flow;
    const settings = await flow.window('settings');
    const overlay = await flow.window('overlay');
    await tab(settings, 'History').click();
    const search = settings
      .getByRole('searchbox', { name: 'Search history' })
      .or(settings.getByLabel('Search history'));
    await search.first().click();

    // The paste goes to whatever window has focus, so make sure it is ours.
    await app.evaluate(({ BrowserWindow }) => {
      const win = BrowserWindow.getAllWindows().find((w) => w.isFocusable());
      win?.show();
      win?.focus();
    });
    await settings.waitForTimeout(300);
    const ours = await app.evaluate(() => {
      const g = globalThis as unknown as {
        __flow: { platform: { getFocusedApp(): { pid: number } | null } };
      };
      return g.__flow.platform.getFocusedApp()?.pid === process.pid;
    });
    test.skip(!ours, 'another window has focus; not pasting into it');

    type Hook = { __flow: { session: { handleHotkey(e: unknown): void; cancel(): void } } };
    await app.evaluate(() =>
      (globalThis as unknown as Hook).__flow.session.handleHotkey({ type: 'hold-start' }),
    );
    // The pill appears once audio is flowing.
    await expect(overlay.locator('#stage')).toHaveAttribute('data-phase', 'listening', {
      timeout: 5000,
    });
    await overlay.waitForTimeout(2500);
    await overlay.addStyleTag({
      content: 'body{background:linear-gradient(135deg,#27405f,#5b3f6b 55%,#94564c)!important}',
    });
    await overlay.screenshot({ path: path.join(SHOTS, 'overlay-listening.png') });
    await overlay.waitForTimeout(9000);

    const stillOurs = await app.evaluate(() => {
      const g = globalThis as unknown as {
        __flow: { platform: { getFocusedApp(): { pid: number } | null } };
      };
      return g.__flow.platform.getFocusedApp()?.pid === process.pid;
    });
    if (!stillOurs) {
      await app.evaluate(() => (globalThis as unknown as Hook).__flow.session.cancel());
      test.skip(true, 'focus moved to another window during the recording');
    }
    await app.evaluate(() =>
      (globalThis as unknown as Hook).__flow.session.handleHotkey({
        type: 'hold-end',
        aborted: false,
      }),
    );

    // Text lands at the cursor, with the trailing space.
    await expect(search.first()).toHaveValue(/ask not what your country can do for you/i, {
      timeout: 20_000,
    });
    // History is on in this config, so the dictation is recorded locally.
    await expect
      .poll(() => settings.evaluate(() => window.flow.history.search('fellow', 10, 0)))
      .toHaveLength(1);
    const [entry] = await settings.evaluate(() => window.flow.history.search('', 10, 0));
    expect(entry).toMatchObject({ model: 'parakeet-tdt-0.6b-v2', app: 'electron.exe' });
    console.log(`key release to text: ${entry!.elapsedMs} ms for ${entry!.audioMs} ms of audio`);
    await search.first().fill('');
    await shot(settings, 'settings-history-entry');
  });
});
