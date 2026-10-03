/** Cloud speech adapters: one per provider API, all behind the Transcriber interface. */
import { CUSTOM_PREFIX, type Catalog } from '@shared/catalog';
import type { Config } from '@shared/config';
import type { KeyTestResult } from '@shared/ipc';
import type { Transcriber } from '@shared/types';
import { TranscribeError } from '../errors';
import type { AdapterOptions } from './base';
import { createDeepgramTranscriber } from './deepgram';
import { createElevenLabsTranscriber } from './elevenlabs';
import { errorDetail } from './http';
import { createMistralTranscriber } from './mistral';
import { createOpenAiCompatibleTranscriber } from './openai-compatible';
import { authHeaders, resolveProvider, type ResolvedProvider } from './provider';

export interface CloudDeps {
  catalog: Catalog;
  config: Config;
  /** Looks up a stored secret by id, normally the provider id. */
  getSecret(id: string): string | undefined;
  fetch?: typeof fetch;
}

/** Model a custom endpoint is asked for when `[providers.<id>].model` is not set. */
const DEFAULT_CUSTOM_MODEL = 'whisper-1';

const KEY_TEST_TIMEOUT_MS = 10_000;

function fetchOf(deps: CloudDeps): typeof fetch {
  // Looked up at call time so a test can stub the global.
  return deps.fetch ?? ((input, init) => globalThis.fetch(input, init));
}

function adapterFor(options: AdapterOptions): Transcriber {
  switch (options.provider.api) {
    case 'openai':
      return createOpenAiCompatibleTranscriber(options);
    case 'mistral':
      return createMistralTranscriber(options);
    case 'elevenlabs':
      return createElevenLabsTranscriber(options);
    case 'deepgram':
      return createDeepgramTranscriber(options);
    case 'anthropic':
      throw new TranscribeError(
        'unknown-model',
        `${options.provider.name} has no speech-to-text API (model "${options.id}")`,
      );
  }
}

/**
 * Builds the adapter for a cloud model id (`groq/whisper-large-v3-turbo`, `custom/<provider-id>`, …).
 * Throws TranscribeError('unknown-model' | 'no-key') synchronously if it cannot.
 */
export function createCloudTranscriber(modelId: string, deps: CloudDeps): Transcriber {
  const fetch = fetchOf(deps);

  if (modelId.startsWith(CUSTOM_PREFIX)) {
    const providerId = modelId.slice(CUSTOM_PREFIX.length);
    const settings = deps.config.providers[providerId];
    if (!providerId || !settings?.base_url) {
      throw new TranscribeError(
        'unknown-model',
        `Custom endpoint "${providerId}" has no base_url in [providers.${providerId}]`,
        { userMessage: 'Custom endpoint is not configured' },
      );
    }
    const resolved = resolveProvider(providerId, deps)!;
    // A custom endpoint always speaks the OpenAI API, whatever its id collides with.
    const provider: ResolvedProvider = { ...resolved, api: 'openai', keyless: true };
    return createOpenAiCompatibleTranscriber({
      id: modelId,
      model: settings.model?.trim() || DEFAULT_CUSTOM_MODEL,
      provider,
      fetch,
      useHints: true,
    });
  }

  const entry = deps.catalog.cloud.find((m) => m.id === modelId);
  if (!entry) throw new TranscribeError('unknown-model', `Unknown cloud model "${modelId}"`);
  const provider = resolveProvider(entry.provider, deps);
  if (!provider) {
    throw new TranscribeError(
      'unknown-model',
      `Model "${modelId}" names an unknown provider "${entry.provider}"`,
    );
  }
  if (!provider.apiKey && !provider.keyless) {
    throw new TranscribeError('no-key', `No API key saved for ${provider.name}`, {
      userMessage: `Add a ${provider.name} API key in Settings`,
    });
  }
  // Every catalogued speech model takes some form of hint; the catalog can opt one out.
  return adapterFor({
    id: modelId,
    model: entry.model,
    provider,
    fetch,
    useHints: entry.hints !== false,
  });
}

/** The cheapest authenticated GET each API offers. */
function keyTestUrl(provider: ResolvedProvider): string {
  switch (provider.api) {
    case 'anthropic':
      return `${provider.baseUrl}/models?limit=1`;
    case 'elevenlabs':
      return `${provider.baseUrl}/user`;
    case 'deepgram':
      return `${provider.baseUrl}/projects`;
    case 'openai':
    case 'mistral':
      return `${provider.baseUrl}/models`;
  }
}

/** One cheap authenticated request (e.g. GET models) to check a stored key. Never throws. */
export async function testProviderKey(providerId: string, deps: CloudDeps): Promise<KeyTestResult> {
  try {
    const provider = resolveProvider(providerId, deps);
    if (!provider) return { ok: false, message: `Unknown provider "${providerId}"` };
    if (!provider.apiKey && !provider.keyless) {
      return { ok: false, message: `No API key saved for ${provider.name}` };
    }
    const timeout = new AbortController();
    const timer = setTimeout(() => timeout.abort(), KEY_TEST_TIMEOUT_MS);
    let status: number;
    let body: string;
    try {
      const response = await fetchOf(deps)(keyTestUrl(provider), {
        method: 'GET',
        headers: authHeaders(provider),
        signal: timeout.signal,
      });
      status = response.status;
      body = await response.text().catch(() => '');
    } catch {
      return {
        ok: false,
        message: timeout.signal.aborted
          ? `${provider.name} did not respond`
          : `Couldn't reach ${provider.name}`,
      };
    } finally {
      clearTimeout(timer);
    }

    if (status >= 200 && status < 300) {
      return { ok: true, message: provider.apiKey ? 'Key works' : `${provider.name} is reachable` };
    }
    // A scoped ElevenLabs key is valid but may not read the account endpoint.
    if (status === 401 && /missing_permissions/i.test(body)) {
      return { ok: true, message: 'Key works (restricted key)' };
    }
    if (status === 401 || status === 403) {
      return { ok: false, message: `${provider.name} rejected the key (${status})` };
    }
    if (status === 429) return { ok: true, message: 'Key accepted, but rate limited right now' };
    const detail = errorDetail(body);
    return {
      ok: false,
      message: `${provider.name} returned ${status}${detail ? `: ${detail}` : ''}`.slice(0, 160),
    };
  } catch (err) {
    return { ok: false, message: err instanceof Error ? err.message : 'Key test failed' };
  }
}

export { TranscribeError } from '../errors';
export type { ResolvedProvider } from './provider';
