import { existsSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  ChecksumError,
  downloadFile,
  isAbortError,
  sha256File,
} from '../../../src/main/models/downloader';
import { fakeServer, randomBytes, sha256, tempDir } from './helpers';

const URL_A = 'https://models.test/a.gguf';
const bytes = randomBytes(50_000);

let dir: string;
let cleanup: () => Promise<void>;
let dest: string;
let part: string;

beforeEach(async () => {
  ({ dir, cleanup } = await tempDir());
  dest = join(dir, 'nested', 'a.gguf');
  part = `${dest}.part`;
});
afterEach(() => cleanup());

/** Leaves a partial download on disk, as an interrupted earlier attempt would. */
async function seedPart(data: Uint8Array): Promise<void> {
  await mkdir(dirname(part), { recursive: true });
  await writeFile(part, data);
}

function options(server: ReturnType<typeof fakeServer>) {
  return {
    url: URL_A,
    destPath: dest,
    sha256: sha256(bytes),
    sizeBytes: bytes.length,
    fetch: server.fetch,
  };
}

describe('downloadFile', () => {
  it('downloads a fresh file, verifies it and moves it into place', async () => {
    const server = fakeServer({ [URL_A]: { bytes } });
    const progress: number[] = [];
    await downloadFile({
      ...options(server),
      onProgress: (received, total) => {
        expect(total).toBe(bytes.length);
        progress.push(received);
      },
    });

    expect(Buffer.from(await readFile(dest)).equals(Buffer.from(bytes))).toBe(true);
    expect(existsSync(part)).toBe(false);
    expect(server.requests).toEqual([{ url: URL_A, range: null }]);
    expect(progress[0]).toBe(0);
    expect(progress.at(-1)).toBe(bytes.length);
    expect(progress).toEqual([...progress].sort((a, b) => a - b));
    // Completion is reported exactly once, as the hand-off to verification.
    expect(progress.filter((p) => p === bytes.length)).toHaveLength(1);
  });

  it('resumes a partial file with a Range request', async () => {
    const server = fakeServer({ [URL_A]: { bytes } });
    await seedPart(bytes.subarray(0, 20_000));

    const progress: number[] = [];
    await downloadFile({ ...options(server), onProgress: (received) => progress.push(received) });

    expect(server.requests).toEqual([{ url: URL_A, range: 'bytes=20000-' }]);
    expect(progress[0]).toBe(20_000);
    expect(await sha256File(dest)).toBe(sha256(bytes));
    expect(existsSync(part)).toBe(false);
  });

  it('verifies without a request when the partial file is already complete', async () => {
    const server = fakeServer({ [URL_A]: { bytes } });
    await seedPart(bytes);

    await downloadFile(options(server));
    expect(server.requests).toHaveLength(0);
    expect(await sha256File(dest)).toBe(sha256(bytes));
  });

  it('starts over when the server ignores the Range header', async () => {
    const server = fakeServer({ [URL_A]: { bytes, ignoreRange: true } });
    await seedPart(bytes.subarray(0, 20_000));

    await downloadFile(options(server));
    expect(await sha256File(dest)).toBe(sha256(bytes));
  });

  it('starts over when the server answers 416', async () => {
    const server = fakeServer({ [URL_A]: { bytes, rejectRange: true } });
    await seedPart(bytes.subarray(0, 20_000));

    await downloadFile(options(server));
    expect(server.requests.map((r) => r.range)).toEqual(['bytes=20000-', null]);
    expect(await sha256File(dest)).toBe(sha256(bytes));
  });

  it('deletes the partial file and throws on a checksum mismatch', async () => {
    const wrong = randomBytes(bytes.length, 99);
    const server = fakeServer({ [URL_A]: { bytes: wrong } });

    await expect(downloadFile(options(server))).rejects.toBeInstanceOf(ChecksumError);
    expect(existsSync(part)).toBe(false);
    expect(existsSync(dest)).toBe(false);
  });

  it('recovers from a corrupt partial file on the next attempt', async () => {
    const server = fakeServer({ [URL_A]: { bytes } });
    await seedPart(randomBytes(20_000, 7));

    await expect(downloadFile(options(server))).rejects.toBeInstanceOf(ChecksumError);
    expect(existsSync(part)).toBe(false);

    await downloadFile(options(server));
    expect(await sha256File(dest)).toBe(sha256(bytes));
  });

  it('keeps the partial file when cancelled, then resumes from it', async () => {
    const server = fakeServer({ [URL_A]: { bytes } });
    const controller = new AbortController();
    server.onChunk = (sent) => {
      if (sent >= 10_000) controller.abort();
    };

    const error = await downloadFile({ ...options(server), signal: controller.signal }).catch(
      (e: unknown) => e,
    );
    expect(isAbortError(error)).toBe(true);
    expect(existsSync(dest)).toBe(false);
    const kept = (await readFile(part)).length;
    expect(kept).toBeGreaterThan(0);
    expect(kept).toBeLessThan(bytes.length);

    server.onChunk = undefined;
    await downloadFile(options(server));
    expect(server.requests.at(-1)).toEqual({ url: URL_A, range: `bytes=${kept}-` });
    expect(await sha256File(dest)).toBe(sha256(bytes));
    expect(existsSync(part)).toBe(false);
  });

  it('rejects immediately when the signal is already aborted', async () => {
    const server = fakeServer({ [URL_A]: { bytes } });
    const controller = new AbortController();
    controller.abort();
    const error = await downloadFile({ ...options(server), signal: controller.signal }).catch(
      (e: unknown) => e,
    );
    expect(isAbortError(error)).toBe(true);
    expect(server.requests).toHaveLength(0);
  });

  it('throws on a non-200 response and leaves nothing behind', async () => {
    const missing = fakeServer({});
    await expect(downloadFile(options(missing))).rejects.toThrow(/HTTP 404/);
    const failing = fakeServer({ [URL_A]: { bytes, status: 503 } });
    await expect(downloadFile(options(failing))).rejects.toThrow(/HTTP 503/);
    expect(existsSync(part)).toBe(false);
    expect(existsSync(dest)).toBe(false);
  });

  it('keeps the partial file when the connection drops, then resumes', async () => {
    const server = fakeServer({ [URL_A]: { bytes, dropAfter: 12_000 } });
    await expect(downloadFile(options(server))).rejects.toThrow(/interrupted/);
    expect((await readFile(part)).length).toBe(12_000);

    server.files[URL_A] = { bytes };
    await downloadFile(options(server));
    expect(server.requests.at(-1)?.range).toBe('bytes=12000-');
    expect(await sha256File(dest)).toBe(sha256(bytes));
  });

  it('refuses a file whose size differs from the manifest', async () => {
    const server = fakeServer({ [URL_A]: { bytes: randomBytes(60_000) } });
    await expect(downloadFile(options(server))).rejects.toThrow(/60000 bytes, expected 50000/);
    expect(existsSync(part)).toBe(false);
  });

  it('gives up when the server stops sending data', async () => {
    const server = fakeServer({ [URL_A]: { bytes, hang: true } });
    await expect(downloadFile({ ...options(server), stallTimeoutMs: 40 })).rejects.toThrow(
      /stalled/,
    );
  });

  it('refuses URLs that are not https', async () => {
    const server = fakeServer({});
    for (const url of ['http://models.test/a.gguf', 'file:///c:/a.gguf', 'not a url']) {
      await expect(downloadFile({ ...options(server), url })).rejects.toThrow(/not https/);
    }
    expect(server.requests).toHaveLength(0);
  });
});
