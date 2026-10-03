/** What every cloud adapter shares: WAV upload, timing, cancellation and error mapping. */
import type { TranscribeOptions, Transcriber, Transcript } from '@shared/types';
import { SAMPLE_RATE } from '@shared/types';
import { TranscribeError } from '../errors';
import { encodeWav } from '../wav';
import { requestJson } from './http';
import { authHeaders, type ResolvedProvider } from './provider';

export interface AdapterOptions {
  /** Flow model id, e.g. `groq/whisper-large-v3-turbo`. */
  id: string;
  /** Model name the provider knows. */
  model: string;
  provider: ResolvedProvider;
  fetch: typeof fetch;
  /** Send dictionary terms as a hint. */
  useHints: boolean;
}

export interface CloudRequest {
  url: string;
  headers?: Record<string, string>;
  body: NonNullable<RequestInit['body']>;
}

/** A provider that hasn't answered this long after the upload began is treated as unreachable. */
const BASE_TIMEOUT_MS = 30_000;

/**
 * Builds a Transcriber from the two things that differ per provider: how the
 * request is laid out and where the text sits in the response.
 */
export function cloudTranscriber(
  options: AdapterOptions,
  build: (wav: Uint8Array<ArrayBuffer>, opts: TranscribeOptions) => CloudRequest,
  read: (json: unknown) => unknown,
): Transcriber {
  const { id, provider } = options;
  return {
    id,
    cloud: true,
    async transcribe(audio: Float32Array, opts: TranscribeOptions): Promise<Transcript> {
      const started = performance.now();
      const audioMs = (audio.length / SAMPLE_RATE) * 1000;
      if (opts.signal.aborted) throw new TranscribeError('aborted', 'Cancelled');
      // Nothing to send; providers reject an empty file.
      if (audio.length === 0) return { text: '', model: id, audioMs: 0, elapsedMs: 0 };
      const hinted = options.useHints ? opts : { ...opts, hints: [] };
      const request = build(encodeWav(audio, SAMPLE_RATE), hinted);
      const json = await requestJson(
        {
          provider: provider.name,
          fetch: options.fetch,
          signal: opts.signal,
          // Long recordings take longer to upload and to transcribe.
          timeoutMs: BASE_TIMEOUT_MS + audioMs,
        },
        request.url,
        {
          method: 'POST',
          headers: { ...authHeaders(provider), ...request.headers },
          body: request.body,
        },
      );
      const text = read(json);
      if (typeof text !== 'string') {
        throw new TranscribeError('bad-response', `${provider.name} response has no transcript`);
      }
      return { text: text.trim(), model: id, audioMs, elapsedMs: performance.now() - started };
    },
  };
}

/** The WAV as a multipart file part. */
export function wavFile(wav: Uint8Array<ArrayBuffer>): Blob {
  return new Blob([wav], { type: 'audio/wav' });
}

/** Safe property read on an unknown JSON value. */
export function field(value: unknown, key: string | number): unknown {
  if (value === null || typeof value !== 'object') return undefined;
  return (value as Record<string | number, unknown>)[key];
}
