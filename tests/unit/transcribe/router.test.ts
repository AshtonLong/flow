import { describe, expect, it, vi } from 'vitest';
import type { Catalog } from '@shared/catalog';
import { DEFAULT_CONFIG, type Config } from '@shared/config';
import type { EngineClient } from '../../../src/main/transcribe/engine-client';
import { TranscribeError } from '../../../src/main/transcribe/errors';
import { TranscriberRouter, toTranscribeError } from '../../../src/main/transcribe/router';
import type { ModelManager } from '../../../src/main/models';

const CATALOG: Catalog = {
  version: 1,
  asOf: '2026-10-03',
  providers: [
    { id: 'groq', name: 'Groq', api: 'openai', baseUrl: 'https://api.groq.com/openai/v1' },
  ],
  local: [],
  cloud: [
    {
      kind: 'cloud',
      id: 'groq/whisper-large-v3-turbo',
      provider: 'groq',
      model: 'whisper-large-v3-turbo',
      name: 'Whisper large-v3-turbo',
      description: '',
      pricePerHour: 0.04,
    },
  ],
  cleanup: [],
};

function setup(options: { installed?: string[]; secret?: string } = {}) {
  const installed = new Set(options.installed ?? ['parakeet-tdt-0.6b-v2']);
  const engine = {
    load: vi.fn(async () => ({ modelId: '', backend: 'CPU', loadMs: 1 })),
    transcribe: vi.fn(async (_ref: unknown, audio: Float32Array) => ({
      text: `local ${audio.length}`,
      backend: 'CPU',
      loadMs: 0,
      inferMs: 1,
    })),
    unload: vi.fn(async () => undefined),
  };
  const models = {
    catalog: () => CATALOG,
    localEntry: (id: string) => ({ id, name: id }),
    modelPath: (id: string) => (installed.has(id) ? `C:/models/${id}.gguf` : null),
    isInstalled: (id: string) => installed.has(id),
  };
  const router = new TranscriberRouter({
    engine: engine as unknown as EngineClient,
    models: models as unknown as ModelManager,
    getSecret: () => options.secret,
  });
  return { router, engine };
}

const opts = () => ({ language: 'en', hints: [], signal: new AbortController().signal });
const cloudConfig: Config = {
  ...DEFAULT_CONFIG,
  model: { ...DEFAULT_CONFIG.model, active: 'groq/whisper-large-v3-turbo' },
};

describe('TranscriberRouter', () => {
  it('sends local model ids to the engine', async () => {
    const { router, engine } = setup();
    const result = await router.transcribe(new Float32Array(16000), DEFAULT_CONFIG, opts());
    expect(result).toMatchObject({ cloud: false, fellBack: false });
    expect(result.transcript).toMatchObject({ text: 'local 16000', model: 'parakeet-tdt-0.6b-v2' });
    expect(result.transcript.audioMs).toBe(1000);
    expect(engine.transcribe.mock.calls[0]![0]).toMatchObject({
      modelId: 'parakeet-tdt-0.6b-v2',
      device: 'auto',
    });
  });

  it('reports a local model that is not downloaded', async () => {
    const { router } = setup({ installed: [] });
    await expect(
      router.transcribe(new Float32Array(16000), DEFAULT_CONFIG, opts()),
    ).rejects.toMatchObject({ code: 'not-installed' });
  });

  it('falls back to the local model when the cloud model cannot run', async () => {
    // No key is stored, so the cloud adapter cannot be built.
    const { router, engine } = setup();
    const result = await router.transcribe(new Float32Array(16000), cloudConfig, opts());
    expect(result.fellBack).toBe(true);
    expect(result.cloud).toBe(false);
    expect(result.transcript.model).toBe('parakeet-tdt-0.6b-v2');
    expect(result.fallbackReason).toBeTruthy();
    expect(engine.transcribe).toHaveBeenCalledTimes(1);
  });

  it('reports the cloud failure when no fallback is installed', async () => {
    const { router } = setup({ installed: [] });
    await expect(
      router.transcribe(new Float32Array(16000), cloudConfig, opts()),
    ).rejects.toMatchObject({ code: 'no-key' });
  });

  it('does not fall back when the user cancels', async () => {
    const { router, engine } = setup();
    engine.transcribe.mockRejectedValueOnce(
      Object.assign(new Error('Aborted'), { name: 'AbortError' }),
    );
    await expect(
      router.transcribe(new Float32Array(16000), DEFAULT_CONFIG, opts()),
    ).rejects.toMatchObject({ code: 'aborted' });
  });

  it('splits recordings over 30 seconds and joins the pieces', async () => {
    const { router, engine } = setup();
    const audio = new Float32Array(16000 * 70).fill(0.2);
    const result = await router.transcribe(audio, DEFAULT_CONFIG, opts());
    const lengths = engine.transcribe.mock.calls.map((call) => (call[1] as Float32Array).length);
    expect(lengths.length).toBeGreaterThanOrEqual(3);
    expect(Math.max(...lengths)).toBeLessThanOrEqual(16000 * 30);
    expect(lengths.reduce((a, b) => a + b, 0)).toBe(audio.length);
    expect(result.transcript.text.split(' local ').length).toBe(lengths.length);
    expect(result.transcript.audioMs).toBe(70_000);
  });

  it('transcribeWith runs exactly the requested model with no fallback', async () => {
    const { router, engine } = setup();
    await expect(
      router.transcribeWith(
        'groq/whisper-large-v3-turbo',
        new Float32Array(16000),
        cloudConfig,
        opts(),
      ),
    ).rejects.toBeInstanceOf(TranscribeError);
    expect(engine.transcribe).not.toHaveBeenCalled();
  });

  it('warm-up loads the active local model and ignores cloud models', () => {
    const { router, engine } = setup();
    router.warmUp(DEFAULT_CONFIG);
    expect(engine.load).toHaveBeenCalledWith(
      'parakeet-tdt-0.6b-v2',
      'C:/models/parakeet-tdt-0.6b-v2.gguf',
      'auto',
      expect.anything(),
    );
    engine.load.mockClear();
    router.warmUp(cloudConfig);
    expect(engine.load).not.toHaveBeenCalled();
  });
});

describe('toTranscribeError', () => {
  it('keeps TranscribeErrors and classifies the rest', () => {
    const original = new TranscribeError('auth', 'bad key');
    expect(toTranscribeError(original)).toBe(original);
    expect(toTranscribeError(Object.assign(new Error('x'), { name: 'AbortError' })).code).toBe(
      'aborted',
    );
    expect(toTranscribeError(new Error('engine exploded'))).toMatchObject({
      code: 'engine',
      message: 'engine exploded',
    });
  });
});
