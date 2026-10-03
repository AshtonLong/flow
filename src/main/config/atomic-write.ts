import { mkdir, rename, rm, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

const RETRYABLE = new Set(['EPERM', 'EBUSY', 'EACCES']);

/**
 * Writes `data` to `<path>.tmp` and renames it over `path`, so a reader never
 * sees a half-written file and a crash never leaves a truncated one.
 */
export async function writeFileAtomic(path: string, data: string | Uint8Array): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  const tmp = `${path}.tmp`;
  await writeFile(tmp, data);
  // On Windows the rename fails while an editor, indexer or virus scanner has
  // the target open. Those handles are short-lived, so retry briefly.
  for (let attempt = 0; ; attempt++) {
    try {
      await rename(tmp, path);
      return;
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (attempt >= 5 || code === undefined || !RETRYABLE.has(code)) {
        await rm(tmp, { force: true }).catch(() => undefined);
        throw error;
      }
      await new Promise((resolve) => setTimeout(resolve, 20 * (attempt + 1)));
    }
  }
}
