/** Resumable, checksum-verified file download. No Electron imports; `fetch` is injectable. */
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, open, rename, rm, stat } from 'node:fs/promises';
import { dirname } from 'node:path';
import { isHttpsUrl } from './catalog';

export interface DownloadOptions {
  url: string;
  destPath: string;
  /** Expected SHA-256 of the complete file, hex. */
  sha256: string;
  /** Expected size of the complete file. */
  sizeBytes: number;
  fetch?: typeof fetch;
  signal?: AbortSignal;
  /** Called as bytes arrive. `receivedBytes === totalBytes` means verification has started. */
  onProgress?(receivedBytes: number, totalBytes: number): void;
  /** Give up when no data arrives for this long. The partial file is kept. Default 60 s. */
  stallTimeoutMs?: number;
}

/** The finished file did not match the expected SHA-256. The partial file has been deleted. */
export class ChecksumError extends Error {
  constructor(
    readonly expected: string,
    readonly actual: string,
  ) {
    super(`Checksum mismatch: expected ${expected}, got ${actual}`);
    this.name = 'ChecksumError';
  }
}

/** True if `error` is the rejection produced by aborting a download. */
export function isAbortError(error: unknown): boolean {
  return error instanceof Error && error.name === 'AbortError';
}

/** The error a cancelled download rejects with. */
export function abortError(): Error {
  const error = new Error('Download cancelled');
  error.name = 'AbortError';
  return error;
}

/** Size of a file in bytes, or 0 if it does not exist. */
export async function fileSize(path: string): Promise<number> {
  try {
    return (await stat(path)).size;
  } catch {
    return 0;
  }
}

/** Streams a file through SHA-256 and returns the lowercase hex digest. */
export async function sha256File(path: string, signal?: AbortSignal): Promise<string> {
  const hash = createHash('sha256');
  const stream = createReadStream(path, { highWaterMark: 1024 * 1024 });
  for await (const chunk of stream) {
    if (signal?.aborted) {
      stream.destroy();
      throw abortError();
    }
    hash.update(chunk as Buffer);
  }
  return hash.digest('hex');
}

/** The total length from a `Content-Range: bytes 100-999/1000` header, and its start offset. */
function parseContentRange(header: string | null): { start: number; total: number | null } | null {
  const match = /^bytes\s+(\d+)-\d+\/(\d+|\*)$/i.exec(header?.trim() ?? '');
  if (!match) return null;
  return { start: Number(match[1]), total: match[2] === '*' ? null : Number(match[2]) };
}

/**
 * Downloads to `<destPath>.part`, resuming with a Range request if a partial file exists, verifies
 * SHA-256 (streamed), then renames into place. A checksum mismatch deletes the partial file and
 * throws `ChecksumError`. Aborting keeps the partial file and rejects with an `AbortError`.
 */
export async function downloadFile(options: DownloadOptions): Promise<void> {
  const { url, destPath, sizeBytes, signal, onProgress } = options;
  if (!isHttpsUrl(url)) throw new Error('Refusing to download from a URL that is not https');
  if (signal?.aborted) throw abortError();

  const partPath = `${destPath}.part`;
  await mkdir(dirname(destPath), { recursive: true });

  let have = await fileSize(partPath);
  if (have > sizeBytes) {
    // Left over from a different file; it cannot be a prefix of this one.
    await rm(partPath, { force: true });
    have = 0;
  }
  if (have < sizeBytes) await fetchRemainder(options, partPath, have);

  if (signal?.aborted) throw abortError();
  onProgress?.(sizeBytes, sizeBytes);
  const expected = options.sha256.toLowerCase();
  const actual = await sha256File(partPath, signal);
  if (actual !== expected) {
    await rm(partPath, { force: true });
    throw new ChecksumError(expected, actual);
  }
  await rename(partPath, destPath);
}

/** Fetches bytes `have..sizeBytes` into the partial file, restarting from zero if the server insists. */
async function fetchRemainder(
  options: DownloadOptions,
  partPath: string,
  have: number,
): Promise<void> {
  const { url, sizeBytes, signal, onProgress } = options;
  const fetchFn = options.fetch ?? globalThis.fetch;
  const stallMs = options.stallTimeoutMs ?? 60_000;

  // One controller serves both the caller's cancel and the stall timeout.
  const controller = new AbortController();
  const onAbort = (): void => controller.abort();
  signal?.addEventListener('abort', onAbort, { once: true });
  let stalled = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const arm = (): void => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      stalled = true;
      controller.abort();
    }, stallMs);
  };

  const request = (from: number): Promise<Response> =>
    fetchFn(url, {
      headers: from > 0 ? { Range: `bytes=${from}-` } : {},
      signal: controller.signal,
      redirect: 'follow',
    });

  try {
    arm();
    let response = await request(have);
    if (response.status === 416 && have > 0) {
      // The server has nothing at that offset, so the partial file is not a prefix of its file.
      await response.body?.cancel().catch(() => undefined);
      have = 0;
      arm();
      response = await request(0);
    }

    let offset: number;
    let declared: number | null = null;
    if (response.status === 206) {
      const range = parseContentRange(response.headers.get('content-range'));
      if (!range || range.start !== have) {
        throw new Error('Download failed: the server sent a different range than requested');
      }
      offset = have;
      declared = range.total;
    } else if (response.status === 200) {
      // No Range support (or a fresh request): the body is the whole file.
      offset = 0;
      const length = response.headers.get('content-length');
      if (length !== null && !response.headers.get('content-encoding')) declared = Number(length);
    } else {
      await response.body?.cancel().catch(() => undefined);
      throw new Error(`Download failed: HTTP ${response.status} ${response.statusText}`.trim());
    }

    if (declared !== null && declared !== sizeBytes) {
      await response.body?.cancel().catch(() => undefined);
      await rm(partPath, { force: true });
      throw new Error(
        `Download failed: the server's file is ${declared} bytes, expected ${sizeBytes}`,
      );
    }
    if (!response.body) throw new Error('Download failed: empty response');

    const reader = response.body.getReader();
    const handle = await open(partPath, offset > 0 ? 'a' : 'w');
    let received = offset;
    let overflow = false;
    try {
      onProgress?.(received, sizeBytes);
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (controller.signal.aborted) break;
        arm();
        received += value.byteLength;
        if (received > sizeBytes) {
          overflow = true;
          break;
        }
        await handle.writeFile(value);
        // The final callback is left to the caller, which uses it to mark verification.
        if (received < sizeBytes) onProgress?.(received, sizeBytes);
      }
    } finally {
      await handle.close();
      await reader.cancel().catch(() => undefined);
    }

    if (overflow) {
      await rm(partPath, { force: true });
      throw new Error(`Download failed: the server sent more than the expected ${sizeBytes} bytes`);
    }
    if (controller.signal.aborted) throw abortError();
    if (received < sizeBytes) {
      throw new Error('Download interrupted: the connection closed early. Try again to resume.');
    }
  } catch (error) {
    if (signal?.aborted) throw abortError();
    if (stalled) {
      throw new Error(
        `Download stalled: no data for ${Math.round(stallMs / 1000)} s. Try again to resume.`,
      );
    }
    throw error;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', onAbort);
  }
}
