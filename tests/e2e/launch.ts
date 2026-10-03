import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { _electron as electron, type ElectronApplication, type Page } from '@playwright/test';

export const ROOT = path.resolve(import.meta.dirname, '../..');

export interface Launched {
  app: ElectronApplication;
  userData: string;
  /** Waits for a window whose URL contains `name` (`settings` or `overlay`). */
  window(name: 'settings' | 'overlay'): Promise<Page>;
  close(): Promise<void>;
}

/** Launches the built app against a throwaway user-data directory. */
export async function launch(
  options: { config?: string; args?: string[]; modelsDir?: string; test?: boolean } = {},
): Promise<Launched> {
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), 'flow-e2e-'));
  if (options.config !== undefined) {
    fs.writeFileSync(path.join(userData, 'config.toml'), options.config);
  }
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    // Inherited when the tests run from inside another Electron app.
    if (value !== undefined && key !== 'ELECTRON_RUN_AS_NODE') env[key] = value;
  }
  env.FLOW_USER_DATA = userData;
  if (options.modelsDir) env.FLOW_MODELS_DIR = options.modelsDir;
  // Exposes the session to `app.evaluate` so tests can inject hotkey events.
  if (options.test) env.FLOW_TEST = '1';
  const app = await electron.launch({ args: [ROOT, ...(options.args ?? [])], env });

  const find = (name: string) => app.windows().find((page) => page.url().includes(`/${name}/`));
  return {
    app,
    userData,
    async window(name) {
      const deadline = Date.now() + 30_000;
      while (Date.now() < deadline) {
        const page = find(name);
        if (page) {
          await page.waitForLoadState('domcontentloaded');
          return page;
        }
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      throw new Error(`the ${name} window did not open`);
    },
    async close() {
      await app.close().catch(() => undefined);
      fs.rmSync(userData, { recursive: true, force: true });
    },
  };
}

/** Reads a 16-bit PCM mono WAV into float samples. */
export function readWav(file: string): number[] {
  const bytes = fs.readFileSync(file);
  let offset = 12;
  while (offset < bytes.length) {
    const id = bytes.toString('ascii', offset, offset + 4);
    const size = bytes.readUInt32LE(offset + 4);
    if (id === 'data') {
      const samples: number[] = new Array(size / 2);
      for (let i = 0; i < samples.length; i++) {
        samples[i] = bytes.readInt16LE(offset + 8 + i * 2) / 32768;
      }
      return samples;
    }
    offset += 8 + size + (size & 1);
  }
  throw new Error('no data chunk');
}
