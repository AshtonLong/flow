/** Test doubles for the model manager: a fake `fetch` with Range support, and catalog builders. */
import { createHash, generateKeyPairSync, sign, type KeyObject } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Catalog, LocalModelEntry } from '@shared/catalog';

export interface ServedFile {
  bytes: Uint8Array;
  /** Respond with this status instead of serving the file. */
  status?: number;
  /** Ignore Range headers and always send the whole file with 200. */
  ignoreRange?: boolean;
  /** Answer every Range request with 416. */
  rejectRange?: boolean;
  /** Close the connection after this many body bytes of a response. */
  dropAfter?: number;
  /** Open the response but never send a byte. */
  hang?: boolean;
}

export interface FakeServer {
  fetch: typeof fetch;
  files: Record<string, ServedFile>;
  /** Every request made, in order. */
  requests: { url: string; range: string | null }[];
  /** Called after each body chunk is handed to the client. */
  onChunk?: (sentBytes: number) => void;
}

function abortError(): Error {
  const error = new Error('This operation was aborted');
  error.name = 'AbortError';
  return error;
}

/** A `fetch` that serves `files` by URL in small chunks, honouring Range and abort signals. */
export function fakeServer(files: Record<string, ServedFile>, chunkSize = 1024): FakeServer {
  const server: FakeServer = {
    files,
    requests: [],
    fetch: async (input, init) => {
      const url = String(input);
      const signal = init?.signal ?? undefined;
      const range = new Headers(init?.headers).get('range');
      server.requests.push({ url, range });
      if (signal?.aborted) throw abortError();

      const file = server.files[url];
      if (!file) return new Response('not found', { status: 404, statusText: 'Not Found' });
      if (file.status) return new Response('nope', { status: file.status });
      if (range && file.rejectRange) return new Response(null, { status: 416 });

      let start = 0;
      let status = 200;
      const headers: Record<string, string> = {};
      const match = range && !file.ignoreRange ? /^bytes=(\d+)-$/.exec(range) : null;
      if (match) {
        start = Number(match[1]);
        if (start >= file.bytes.length) return new Response(null, { status: 416 });
        status = 206;
        headers['content-range'] = `bytes ${start}-${file.bytes.length - 1}/${file.bytes.length}`;
      }
      headers['content-length'] = String(file.bytes.length - start);

      let offset = start;
      const end = Math.min(file.bytes.length, start + (file.dropAfter ?? Infinity));
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          signal?.addEventListener('abort', () => controller.error(abortError()), { once: true });
        },
        async pull(controller) {
          if (file.hang) return new Promise<void>(() => undefined);
          // Yield to the event loop so aborts and timers can interleave with chunks.
          await new Promise((resolve) => setImmediate(resolve));
          if (signal?.aborted) return;
          if (offset >= end) {
            controller.close();
            return;
          }
          const chunk = file.bytes.subarray(offset, Math.min(end, offset + chunkSize));
          offset += chunk.length;
          controller.enqueue(chunk);
          server.onChunk?.(offset - start);
        },
      });
      return new Response(body, { status, headers });
    },
  };
  return server;
}

/** Deterministic pseudo-random bytes. */
export function randomBytes(length: number, seed = 1): Uint8Array {
  const bytes = new Uint8Array(length);
  let state = seed >>> 0 || 1;
  for (let i = 0; i < length; i++) {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    bytes[i] = state >>> 24;
  }
  return bytes;
}

export function sha256(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex');
}

/** Creates a temp directory and returns it with a cleanup function. */
export async function tempDir(): Promise<{ dir: string; cleanup: () => Promise<void> }> {
  const dir = await mkdtemp(join(tmpdir(), 'flow-models-'));
  return { dir, cleanup: () => rm(dir, { recursive: true, force: true }) };
}

export interface TestKey {
  publicPem: string;
  privateKey: KeyObject;
}

export function testKey(): TestKey {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  return { publicPem: publicKey.export({ type: 'spki', format: 'pem' }).toString(), privateKey };
}

export function signBytes(bytes: Uint8Array, key: TestKey): string {
  return sign(null, bytes, key.privateKey).toString('base64');
}

export function localEntry(id: string, bytes: Uint8Array): LocalModelEntry {
  return {
    kind: 'local',
    id,
    name: `Model ${id}`,
    family: 'test',
    description: 'A test model',
    url: `https://models.test/${id}.gguf`,
    file: `${id}.gguf`,
    sha256: sha256(bytes),
    sizeBytes: bytes.length,
    languages: ['en'],
    licence: 'MIT',
    runsOn: 'cpu',
    tier: 'core',
  };
}

export function testCatalog(local: LocalModelEntry[], asOf = '2026-10-03'): Catalog {
  return {
    version: 1,
    asOf,
    providers: [
      { id: 'groq', name: 'Groq', api: 'openai', baseUrl: 'https://api.groq.com/openai/v1' },
      { id: 'openai', name: 'OpenAI', api: 'openai', baseUrl: 'https://api.openai.com/v1' },
      {
        id: 'ollama',
        name: 'Ollama (local)',
        api: 'openai',
        baseUrl: 'http://localhost:11434/v1',
        keyless: true,
      },
    ],
    local,
    cloud: [
      {
        kind: 'cloud',
        id: 'groq/whisper-large-v3-turbo',
        provider: 'groq',
        model: 'whisper-large-v3-turbo',
        name: 'Whisper large-v3-turbo',
        description: 'Lowest price',
        pricePerHour: 0.04,
      },
      {
        kind: 'cloud',
        id: 'openai/gpt-4o-transcribe',
        provider: 'openai',
        model: 'gpt-4o-transcribe',
        name: 'gpt-4o-transcribe',
        description: 'OpenAI',
        pricePerHour: 0.36,
      },
      {
        kind: 'cloud',
        id: 'ollama/whisper',
        provider: 'ollama',
        model: 'whisper',
        name: 'Local Whisper',
        description: 'Keyless',
        pricePerHour: 0,
      },
    ],
    cleanup: [],
  };
}

export function encode(catalog: Catalog): Buffer {
  return Buffer.from(`${JSON.stringify(catalog, null, 2)}\n`, 'utf8');
}
