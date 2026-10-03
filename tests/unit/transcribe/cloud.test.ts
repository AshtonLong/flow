import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Catalog, CloudModelEntry } from '@shared/catalog';
import { type Config, ConfigSchema } from '@shared/config';
import type { TranscribeOptions } from '@shared/types';
import {
  type CloudDeps,
  createCloudTranscriber,
  testProviderKey,
} from '../../../src/main/transcribe/cloud';
import { TranscribeError } from '../../../src/main/transcribe/errors';

function cloud(id: string, provider: string, model: string, extra = {}): CloudModelEntry {
  return {
    kind: 'cloud',
    id,
    provider,
    model,
    name: id,
    description: '',
    pricePerHour: 0,
    ...extra,
  };
}

const catalog: Catalog = {
  version: 1,
  asOf: '2026-10-03',
  providers: [
    { id: 'groq', name: 'Groq', api: 'openai', baseUrl: 'https://api.groq.com/openai/v1' },
    { id: 'openai', name: 'OpenAI', api: 'openai', baseUrl: 'https://api.openai.com/v1' },
    { id: 'mistral', name: 'Mistral', api: 'mistral', baseUrl: 'https://api.mistral.ai/v1' },
    {
      id: 'elevenlabs',
      name: 'ElevenLabs',
      api: 'elevenlabs',
      baseUrl: 'https://api.elevenlabs.io/v1',
    },
    { id: 'deepgram', name: 'Deepgram', api: 'deepgram', baseUrl: 'https://api.deepgram.com/v1' },
    {
      id: 'anthropic',
      name: 'Anthropic',
      api: 'anthropic',
      baseUrl: 'https://api.anthropic.com/v1',
    },
    {
      id: 'ollama',
      name: 'Ollama',
      api: 'openai',
      baseUrl: 'http://localhost:11434/v1',
      keyless: true,
    },
  ],
  local: [],
  cloud: [
    cloud('groq/whisper-large-v3-turbo', 'groq', 'whisper-large-v3-turbo', { hints: true }),
    cloud('groq/whisper-large-v3', 'groq', 'whisper-large-v3', { hints: false }),
    cloud('openai/gpt-4o-mini-transcribe', 'openai', 'gpt-4o-mini-transcribe'),
    cloud('openai/gpt-transcribe', 'openai', 'gpt-transcribe'),
    cloud('mistral/voxtral-mini-transcribe-2', 'mistral', 'voxtral-mini-2602'),
    cloud('elevenlabs/scribe_v2', 'elevenlabs', 'scribe_v2'),
    cloud('deepgram/nova-3', 'deepgram', 'nova-3'),
    cloud('deepgram/nova-2', 'deepgram', 'nova-2'),
    cloud('anthropic/claude-haiku-4-5', 'anthropic', 'claude-haiku-4-5'),
    cloud('nowhere/model', 'nowhere', 'model'),
  ],
  cleanup: [],
};

const SECRETS: Record<string, string> = {
  groq: 'gsk-test',
  openai: 'sk-test',
  mistral: 'mistral-test',
  elevenlabs: 'xi-test',
  deepgram: 'dg-test',
  anthropic: 'sk-ant-test',
  'lan-key': 'lan-secret',
};

interface Call {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
  signal: AbortSignal;
}

type Reply = { status?: number; json?: unknown; text?: string } | Error | 'hang';

function fakeFetch(...replies: Reply[]) {
  const calls: Call[] = [];
  const fn = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
    const signal = init!.signal as AbortSignal;
    calls.push({
      url: String(url),
      method: init!.method ?? 'GET',
      headers: (init!.headers ?? {}) as Record<string, string>,
      body: init!.body,
      signal,
    });
    const reply = replies[Math.min(calls.length - 1, replies.length - 1)]!;
    if (reply === 'hang') {
      return new Promise<Response>((_resolve, reject) => {
        signal.addEventListener('abort', () =>
          reject(new DOMException('The operation was aborted', 'AbortError')),
        );
      });
    }
    if (reply instanceof Error) throw reply;
    return new Response(reply.text ?? JSON.stringify(reply.json ?? {}), {
      status: reply.status ?? 200,
    });
  });
  return { fn: fn as unknown as typeof fetch, calls };
}

function deps(fetchFn: typeof fetch, overrides: Record<string, unknown> = {}): CloudDeps {
  const config: Config = ConfigSchema.parse(overrides);
  return { catalog, config, getSecret: (id) => SECRETS[id], fetch: fetchFn };
}

function options(overrides: Partial<TranscribeOptions> = {}): TranscribeOptions {
  return { language: 'en', hints: [], signal: new AbortController().signal, ...overrides };
}

/** Half a second of audio: 8000 samples, a 16 044-byte WAV. */
const AUDIO = new Float32Array(8000).fill(0.1);
const WAV_BYTES = 44 + AUDIO.length * 2;

function form(call: Call): FormData {
  expect(call.body).toBeInstanceOf(FormData);
  return call.body as FormData;
}

function expectWavFile(data: FormData): void {
  const file = data.get('file') as File;
  expect(file).toBeInstanceOf(Blob);
  expect(file.size).toBe(WAV_BYTES);
  expect(file.type).toBe('audio/wav');
  expect(file.name).toBe('audio.wav');
}

afterEach(() => {
  vi.useRealTimers();
});

describe('createCloudTranscriber', () => {
  describe('OpenAI-compatible (Groq, OpenAI, custom)', () => {
    it('Groq: multipart upload with model, language, prompt and JSON format', async () => {
      const { fn, calls } = fakeFetch({ json: { text: ' Hello from Tauri. ' } });
      const transcriber = createCloudTranscriber('groq/whisper-large-v3-turbo', deps(fn));
      expect(transcriber.id).toBe('groq/whisper-large-v3-turbo');
      expect(transcriber.cloud).toBe(true);
      expect(transcriber.load).toBeUndefined();

      const result = await transcriber.transcribe(AUDIO, options({ hints: ['Tauri', 'Parakeet'] }));
      expect(result.text).toBe('Hello from Tauri.');
      expect(result.model).toBe('groq/whisper-large-v3-turbo');
      expect(result.audioMs).toBe(500);
      expect(result.elapsedMs).toBeGreaterThanOrEqual(0);

      expect(calls).toHaveLength(1);
      const call = calls[0]!;
      expect(call.url).toBe('https://api.groq.com/openai/v1/audio/transcriptions');
      expect(call.method).toBe('POST');
      expect(call.headers).toEqual({ Authorization: 'Bearer gsk-test' });
      const data = form(call);
      expectWavFile(data);
      expect(data.get('model')).toBe('whisper-large-v3-turbo');
      expect(data.get('language')).toBe('en');
      expect(data.get('response_format')).toBe('json');
      expect(data.get('prompt')).toBe('Tauri, Parakeet');
    });

    it('uploads a real WAV of the audio', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'x' } });
      await createCloudTranscriber('groq/whisper-large-v3-turbo', deps(fn)).transcribe(
        AUDIO,
        options(),
      );
      const bytes = new Uint8Array(await (form(calls[0]!).get('file') as File).arrayBuffer());
      expect(String.fromCharCode(...bytes.subarray(0, 4))).toBe('RIFF');
      const view = new DataView(bytes.buffer);
      expect(view.getUint32(24, true)).toBe(16000);
      expect(view.getInt16(44, true)).toBe(Math.round(0.1 * 0x7fff));
    });

    it('sends no prompt without hints', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'x' } });
      await createCloudTranscriber('groq/whisper-large-v3-turbo', deps(fn)).transcribe(
        AUDIO,
        options(),
      );
      expect(form(calls[0]!).has('prompt')).toBe(false);
    });

    it('sends no hints to a model the catalog marks hints: false', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'x' } });
      await createCloudTranscriber('groq/whisper-large-v3', deps(fn)).transcribe(
        AUDIO,
        options({ hints: ['Tauri'] }),
      );
      expect(form(calls[0]!).has('prompt')).toBe(false);
    });

    it('keeps the prompt within the Whisper limit, whole terms only', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'x' } });
      const hints = Array.from({ length: 400 }, (_, i) => `Term${i}`);
      await createCloudTranscriber('groq/whisper-large-v3-turbo', deps(fn)).transcribe(
        AUDIO,
        options({ hints }),
      );
      const prompt = form(calls[0]!).get('prompt') as string;
      expect(prompt.length).toBeLessThanOrEqual(600);
      expect(prompt.startsWith('Term0, Term1, ')).toBe(true);
      expect(prompt).toMatch(/Term\d+$/);
    });

    it('OpenAI gpt-4o-mini-transcribe uses the same fields', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'ok' } });
      await createCloudTranscriber('openai/gpt-4o-mini-transcribe', deps(fn)).transcribe(
        AUDIO,
        options({ hints: ['Tauri'] }),
      );
      const call = calls[0]!;
      expect(call.url).toBe('https://api.openai.com/v1/audio/transcriptions');
      expect(call.headers).toEqual({ Authorization: 'Bearer sk-test' });
      const data = form(call);
      expect(data.get('model')).toBe('gpt-4o-mini-transcribe');
      expect(data.get('language')).toBe('en');
      expect(data.get('prompt')).toBe('Tauri');
      expect(data.get('response_format')).toBe('json');
    });

    it('OpenAI gpt-transcribe takes languages[] and keywords[] instead', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'ok', languages: [{ code: 'en' }] } });
      const result = await createCloudTranscriber('openai/gpt-transcribe', deps(fn)).transcribe(
        AUDIO,
        options({ hints: ['Tauri', 'premium <plan>', 'two\nlines'] }),
      );
      expect(result.text).toBe('ok');
      const data = form(calls[0]!);
      expect(data.get('model')).toBe('gpt-transcribe');
      expect(data.getAll('languages[]')).toEqual(['en']);
      expect(data.getAll('keywords[]')).toEqual(['Tauri', 'premium plan', 'two lines']);
      expect(data.has('language')).toBe(false);
      expect(data.has('prompt')).toBe(false);
      expect(data.has('response_format')).toBe(false);
    });

    it('custom endpoint: base_url and model from the config, key from its secret reference', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'ok' } });
      const d = deps(fn, {
        providers: {
          'custom-lan': {
            base_url: 'http://192.168.1.20:8000/v1/',
            api_key: 'secret:lan-key',
            model: 'Systran/faster-whisper-large-v3',
          },
        },
      });
      const transcriber = createCloudTranscriber('custom/custom-lan', d);
      expect(transcriber.id).toBe('custom/custom-lan');
      expect(transcriber.cloud).toBe(true);
      const result = await transcriber.transcribe(AUDIO, options({ hints: ['Tauri'] }));
      expect(result.model).toBe('custom/custom-lan');
      const call = calls[0]!;
      expect(call.url).toBe('http://192.168.1.20:8000/v1/audio/transcriptions');
      expect(call.headers).toEqual({ Authorization: 'Bearer lan-secret' });
      const data = form(call);
      expect(data.get('model')).toBe('Systran/faster-whisper-large-v3');
      expect(data.get('prompt')).toBe('Tauri');
      expectWavFile(data);
    });

    it('custom endpoint: key optional, model defaults to whisper-1', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'ok' } });
      const d = deps(fn, { providers: { lan: { base_url: 'http://10.0.0.2:9000/v1' } } });
      await createCloudTranscriber('custom/lan', d).transcribe(AUDIO, options());
      expect(calls[0]!.url).toBe('http://10.0.0.2:9000/v1/audio/transcriptions');
      expect(calls[0]!.headers).toEqual({});
      expect(form(calls[0]!).get('model')).toBe('whisper-1');
    });

    it('base_url in the config overrides a catalog provider', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'ok' } });
      const d = deps(fn, { providers: { groq: { base_url: 'https://proxy.example.com/v1' } } });
      await createCloudTranscriber('groq/whisper-large-v3-turbo', d).transcribe(AUDIO, options());
      expect(calls[0]!.url).toBe('https://proxy.example.com/v1/audio/transcriptions');
      expect(calls[0]!.headers).toEqual({ Authorization: 'Bearer gsk-test' });
    });
  });

  describe('Mistral', () => {
    it('Voxtral: multipart upload with context_bias terms', async () => {
      const { fn, calls } = fakeFetch({ json: { model: 'voxtral-mini-2602', text: ' Bonjour. ' } });
      const transcriber = createCloudTranscriber('mistral/voxtral-mini-transcribe-2', deps(fn));
      const result = await transcriber.transcribe(
        AUDIO,
        options({ language: 'en-US', hints: ['Tauri', 'Baker Street', 'a,b', 'tauri'] }),
      );
      expect(result.text).toBe('Bonjour.');
      expect(result.model).toBe('mistral/voxtral-mini-transcribe-2');
      const call = calls[0]!;
      expect(call.url).toBe('https://api.mistral.ai/v1/audio/transcriptions');
      expect(call.method).toBe('POST');
      expect(call.headers).toEqual({ Authorization: 'Bearer mistral-test' });
      const data = form(call);
      expectWavFile(data);
      expect(data.get('model')).toBe('voxtral-mini-2602');
      expect(data.get('language')).toBe('en');
      // The API rejects spaces and commas in a bias term.
      expect(data.getAll('context_bias')).toEqual(['Tauri', 'Baker', 'Street', 'a', 'b']);
    });

    it('caps context_bias at 100 terms', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'x' } });
      const hints = Array.from({ length: 150 }, (_, i) => `Term${i}`);
      await createCloudTranscriber('mistral/voxtral-mini-transcribe-2', deps(fn)).transcribe(
        AUDIO,
        options({ hints }),
      );
      expect(form(calls[0]!).getAll('context_bias')).toHaveLength(100);
    });
  });

  describe('ElevenLabs', () => {
    it('Scribe v2: xi-api-key, model_id and keyterms', async () => {
      const { fn, calls } = fakeFetch({
        json: { language_code: 'en', language_probability: 1, text: ' Hello. ', words: [] },
      });
      const transcriber = createCloudTranscriber('elevenlabs/scribe_v2', deps(fn));
      const result = await transcriber.transcribe(
        AUDIO,
        options({
          hints: ['Tauri', 'Baker Street', 'x'.repeat(50), 'one two three four five six'],
        }),
      );
      expect(result.text).toBe('Hello.');
      const call = calls[0]!;
      expect(call.url).toBe('https://api.elevenlabs.io/v1/speech-to-text');
      expect(call.method).toBe('POST');
      expect(call.headers).toEqual({ 'xi-api-key': 'xi-test' });
      const data = form(call);
      expectWavFile(data);
      expect(data.get('model_id')).toBe('scribe_v2');
      expect(data.get('language_code')).toBe('en');
      expect(data.get('tag_audio_events')).toBe('false');
      // Under 50 characters and at most five words each.
      expect(data.getAll('keyterms')).toEqual(['Tauri', 'Baker Street']);
    });

    it('caps keyterms at 100 to stay under the 20-second minimum billing', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'x' } });
      const hints = Array.from({ length: 300 }, (_, i) => `Term${i}`);
      await createCloudTranscriber('elevenlabs/scribe_v2', deps(fn)).transcribe(
        AUDIO,
        options({ hints }),
      );
      expect(form(calls[0]!).getAll('keyterms')).toHaveLength(100);
    });
  });

  describe('Deepgram', () => {
    const response = {
      results: {
        channels: [{ alternatives: [{ transcript: ' Hello there. ', confidence: 0.99 }] }],
      },
    };

    it('Nova-3: raw WAV body, Token auth, smart formatting and keyterms', async () => {
      const { fn, calls } = fakeFetch({ json: response });
      const transcriber = createCloudTranscriber('deepgram/nova-3', deps(fn));
      const result = await transcriber.transcribe(
        AUDIO,
        options({ hints: ['Tauri', 'Baker Street'] }),
      );
      expect(result.text).toBe('Hello there.');
      expect(result.model).toBe('deepgram/nova-3');
      const call = calls[0]!;
      const url = new URL(call.url);
      expect(url.origin + url.pathname).toBe('https://api.deepgram.com/v1/listen');
      expect(url.searchParams.get('model')).toBe('nova-3');
      expect(url.searchParams.get('language')).toBe('en');
      expect(url.searchParams.get('smart_format')).toBe('true');
      expect(url.searchParams.getAll('keyterm')).toEqual(['Tauri', 'Baker Street']);
      expect(call.url).toContain('keyterm=Baker+Street');
      expect(call.method).toBe('POST');
      expect(call.headers).toEqual({
        Authorization: 'Token dg-test',
        'Content-Type': 'audio/wav',
      });
      expect(call.body).toBeInstanceOf(Uint8Array);
      const bytes = call.body as Uint8Array;
      expect(bytes.byteLength).toBe(WAV_BYTES);
      expect(String.fromCharCode(...bytes.subarray(0, 4))).toBe('RIFF');
    });

    it('caps keyterms at 100', async () => {
      const { fn, calls } = fakeFetch({ json: response });
      const hints = Array.from({ length: 300 }, (_, i) => `T${i}`);
      await createCloudTranscriber('deepgram/nova-3', deps(fn)).transcribe(
        AUDIO,
        options({ hints }),
      );
      expect(new URL(calls[0]!.url).searchParams.getAll('keyterm')).toHaveLength(100);
    });

    it('sends no keyterms to a model without keyterm prompting', async () => {
      const { fn, calls } = fakeFetch({ json: response });
      await createCloudTranscriber('deepgram/nova-2', deps(fn)).transcribe(
        AUDIO,
        options({ hints: ['Tauri'] }),
      );
      expect(new URL(calls[0]!.url).searchParams.has('keyterm')).toBe(false);
    });

    it('treats an empty transcript as empty text', async () => {
      const { fn } = fakeFetch({
        json: { results: { channels: [{ alternatives: [{ transcript: '' }] }] } },
      });
      const result = await createCloudTranscriber('deepgram/nova-3', deps(fn)).transcribe(
        AUDIO,
        options(),
      );
      expect(result.text).toBe('');
    });
  });

  describe('cannot build', () => {
    function build(id: string, d: CloudDeps): TranscribeError {
      try {
        createCloudTranscriber(id, d);
      } catch (err) {
        expect(err).toBeInstanceOf(TranscribeError);
        return err as TranscribeError;
      }
      throw new Error(`expected "${id}" to throw`);
    }

    it.each([
      ['a model that is not in the catalog', 'groq/whisper-tiny'],
      ['a local model id', 'parakeet-tdt-0.6b-v2'],
      ['a model whose provider is unknown', 'nowhere/model'],
      ['a provider with no speech API', 'anthropic/claude-haiku-4-5'],
      ['a custom endpoint that is not configured', 'custom/missing'],
      ['a custom id with no provider', 'custom/'],
    ])('%s → unknown-model', (_name, id) => {
      const { fn, calls } = fakeFetch({ json: {} });
      expect(build(id, deps(fn)).code).toBe('unknown-model');
      expect(calls).toHaveLength(0);
    });

    it('a custom provider with a key but no base_url → unknown-model', () => {
      const { fn } = fakeFetch({ json: {} });
      const d = deps(fn, { providers: { lan: { api_key: 'secret:lan-key' } } });
      expect(build('custom/lan', d).code).toBe('unknown-model');
    });

    it('no stored key → no-key, naming the provider', () => {
      const { fn, calls } = fakeFetch({ json: {} });
      const err = build('groq/whisper-large-v3-turbo', { ...deps(fn), getSecret: () => undefined });
      expect(err.code).toBe('no-key');
      expect(err.userMessage).toContain('Groq');
      expect(calls).toHaveLength(0);
    });

    it('a blank stored key counts as no key', () => {
      const { fn } = fakeFetch({ json: {} });
      const err = build('groq/whisper-large-v3-turbo', { ...deps(fn), getSecret: () => '  ' });
      expect(err.code).toBe('no-key');
    });
  });

  describe('failures', () => {
    const ids = [
      'groq/whisper-large-v3-turbo',
      'mistral/voxtral-mini-transcribe-2',
      'elevenlabs/scribe_v2',
      'deepgram/nova-3',
    ];

    async function failure(id: string, reply: Reply, opts = options()): Promise<TranscribeError> {
      const { fn } = fakeFetch(reply);
      const err = await createCloudTranscriber(id, deps(fn))
        .transcribe(AUDIO, opts)
        .then(
          () => null,
          (e: unknown) => e,
        );
      expect(err).toBeInstanceOf(TranscribeError);
      return err as TranscribeError;
    }

    const cases: [string, Reply, string, number | undefined][] = [
      ['401', { status: 401, json: { error: { message: 'Invalid API Key' } } }, 'auth', 401],
      ['403', { status: 403, text: 'Forbidden' }, 'auth', 403],
      ['429', { status: 429, json: { error: { message: 'Slow down' } } }, 'rate-limit', 429],
      ['400', { status: 400, json: { error: { message: 'bad audio' } } }, 'http', 400],
      ['500', { status: 500, text: 'oops' }, 'http', 500],
      ['a rejected fetch', new TypeError('fetch failed'), 'network', undefined],
      ['a body that is not JSON', { text: '<html>gateway</html>' }, 'bad-response', 200],
      ['JSON without a transcript', { json: { unexpected: true } }, 'bad-response', undefined],
    ];

    describe.each(ids)('%s', (id) => {
      it.each(cases)('maps %s', async (_name, reply, code, status) => {
        const err = await failure(id, reply);
        expect(err.code).toBe(code);
        expect(err.status).toBe(status);
        expect(err.userMessage.length).toBeGreaterThan(0);
      });

      it('maps a cancel to aborted', async () => {
        const { fn, calls } = fakeFetch('hang');
        const controller = new AbortController();
        const pending = createCloudTranscriber(id, deps(fn))
          .transcribe(AUDIO, options({ signal: controller.signal }))
          .catch((e: unknown) => e);
        await Promise.resolve();
        expect(calls).toHaveLength(1);
        controller.abort();
        const err = (await pending) as TranscribeError;
        expect(err).toBeInstanceOf(TranscribeError);
        expect(err.code).toBe('aborted');
        expect(calls[0]!.signal.aborted).toBe(true);
      });

      it('does not send anything when already cancelled', async () => {
        const { fn, calls } = fakeFetch({ json: { text: 'x' } });
        const controller = new AbortController();
        controller.abort();
        const err = await createCloudTranscriber(id, deps(fn))
          .transcribe(AUDIO, options({ signal: controller.signal }))
          .catch((e: unknown) => e);
        expect((err as TranscribeError).code).toBe('aborted');
        expect(calls).toHaveLength(0);
      });
    });

    it('includes the provider and its message for the log', async () => {
      const err = await failure('groq/whisper-large-v3-turbo', {
        status: 401,
        json: { error: { message: 'Invalid API Key' } },
      });
      expect(err.message).toBe('Groq responded 401: Invalid API Key');
      expect(err.userMessage).toBe('Groq rejected the API key');
    });

    it('gives up on a provider that never answers', async () => {
      vi.useFakeTimers();
      const { fn, calls } = fakeFetch('hang');
      const pending = createCloudTranscriber('groq/whisper-large-v3-turbo', deps(fn))
        .transcribe(AUDIO, options())
        .catch((e: unknown) => e);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(calls[0]!.signal.aborted).toBe(false);
      await vi.advanceTimersByTimeAsync(600);
      const err = (await pending) as TranscribeError;
      expect(err.code).toBe('network');
      expect(err.userMessage).toBe('Groq timed out');
      expect(calls[0]!.signal.aborted).toBe(true);
    });

    it('returns empty text for empty audio without a request', async () => {
      const { fn, calls } = fakeFetch({ json: { text: 'x' } });
      const result = await createCloudTranscriber(
        'groq/whisper-large-v3-turbo',
        deps(fn),
      ).transcribe(new Float32Array(0), options());
      expect(result).toEqual({
        text: '',
        model: 'groq/whisper-large-v3-turbo',
        audioMs: 0,
        elapsedMs: 0,
      });
      expect(calls).toHaveLength(0);
    });
  });
});

describe('testProviderKey', () => {
  it.each([
    ['groq', 'https://api.groq.com/openai/v1/models', { Authorization: 'Bearer gsk-test' }],
    ['openai', 'https://api.openai.com/v1/models', { Authorization: 'Bearer sk-test' }],
    ['mistral', 'https://api.mistral.ai/v1/models', { Authorization: 'Bearer mistral-test' }],
    ['elevenlabs', 'https://api.elevenlabs.io/v1/user', { 'xi-api-key': 'xi-test' }],
    ['deepgram', 'https://api.deepgram.com/v1/projects', { Authorization: 'Token dg-test' }],
    [
      'anthropic',
      'https://api.anthropic.com/v1/models?limit=1',
      { 'x-api-key': 'sk-ant-test', 'anthropic-version': '2023-06-01' },
    ],
  ])('%s: one authenticated GET', async (provider, url, headers) => {
    const { fn, calls } = fakeFetch({ json: { data: [] } });
    const result = await testProviderKey(provider, deps(fn));
    expect(result).toEqual({ ok: true, message: 'Key works' });
    expect(calls).toHaveLength(1);
    expect(calls[0]!.url).toBe(url);
    expect(calls[0]!.method).toBe('GET');
    expect(calls[0]!.headers).toEqual(headers);
  });

  it('a keyless provider is checked for reachability', async () => {
    const { fn, calls } = fakeFetch({ json: { data: [] } });
    const result = await testProviderKey('ollama', deps(fn));
    expect(result).toEqual({ ok: true, message: 'Ollama is reachable' });
    expect(calls[0]!.url).toBe('http://localhost:11434/v1/models');
    expect(calls[0]!.headers).toEqual({});
  });

  it('a custom endpoint uses its base_url and secret reference', async () => {
    const { fn, calls } = fakeFetch({ json: { data: [] } });
    const d = deps(fn, {
      providers: { lan: { base_url: 'http://10.0.0.2:9000/v1', api_key: 'secret:lan-key' } },
    });
    expect((await testProviderKey('lan', d)).ok).toBe(true);
    expect(calls[0]!.url).toBe('http://10.0.0.2:9000/v1/models');
    expect(calls[0]!.headers).toEqual({ Authorization: 'Bearer lan-secret' });
  });

  it.each([
    [
      'a rejected key',
      { status: 401, json: { error: { message: 'Invalid API Key' } } },
      false,
      'Groq rejected the key (401)',
    ],
    ['a forbidden key', { status: 403 }, false, 'Groq rejected the key (403)'],
    ['a rate limit', { status: 429 }, true, 'Key accepted, but rate limited right now'],
    ['a server error', { status: 500, text: 'boom' }, false, 'Groq returned 500: boom'],
    ['a network failure', new TypeError('fetch failed'), false, "Couldn't reach Groq"],
  ] as [string, Reply, boolean, string][])('%s', async (_name, reply, ok, message) => {
    const { fn } = fakeFetch(reply);
    expect(await testProviderKey('groq', deps(fn))).toEqual({ ok, message });
  });

  it('accepts a restricted ElevenLabs key that cannot read the account', async () => {
    const { fn } = fakeFetch({
      status: 401,
      json: { detail: { status: 'missing_permissions', message: 'Missing user_read' } },
    });
    expect(await testProviderKey('elevenlabs', deps(fn))).toEqual({
      ok: true,
      message: 'Key works (restricted key)',
    });
  });

  it('reports a missing key without a request', async () => {
    const { fn, calls } = fakeFetch({ json: {} });
    const result = await testProviderKey('groq', { ...deps(fn), getSecret: () => undefined });
    expect(result).toEqual({ ok: false, message: 'No API key saved for Groq' });
    expect(calls).toHaveLength(0);
  });

  it('reports an unknown provider', async () => {
    const { fn } = fakeFetch({ json: {} });
    expect(await testProviderKey('nowhere', deps(fn))).toEqual({
      ok: false,
      message: 'Unknown provider "nowhere"',
    });
  });

  it('times out without throwing', async () => {
    vi.useFakeTimers();
    const { fn } = fakeFetch('hang');
    const pending = testProviderKey('groq', deps(fn));
    await vi.advanceTimersByTimeAsync(10_000);
    expect(await pending).toEqual({ ok: false, message: 'Groq did not respond' });
  });

  it('never throws, even if a dependency does', async () => {
    const { fn } = fakeFetch({ json: {} });
    const d: CloudDeps = {
      ...deps(fn),
      getSecret: () => {
        throw new Error('keychain locked');
      },
    };
    expect(await testProviderKey('groq', d)).toEqual({ ok: false, message: 'keychain locked' });
  });
});
