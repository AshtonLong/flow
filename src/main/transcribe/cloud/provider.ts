/** Resolves a provider id to its endpoint and key, for speech adapters and the cleanup LLM. */
import type { Catalog, ProviderApi } from '@shared/catalog';
import type { Config } from '@shared/config';

export interface ProviderDeps {
  catalog: Catalog;
  config: Config;
  /** Looks up a stored secret by id, normally the provider id. */
  getSecret(id: string): string | undefined;
}

export interface ResolvedProvider {
  id: string;
  name: string;
  api: ProviderApi;
  /** Base URL without a trailing slash. */
  baseUrl: string;
  /** Requests work without a key (local endpoints, custom endpoints). */
  keyless: boolean;
  apiKey?: string;
  /** Defined only in `[providers.<id>]`, not in the catalog. */
  custom: boolean;
}

export const ANTHROPIC_VERSION = '2023-06-01';

const SECRET_PREFIX = 'secret:';

/** The secret id a provider's key is stored under: `api_key = "secret:<id>"`, else the provider id. */
export function secretIdFor(providerId: string, config: Config): string {
  const ref = config.providers[providerId]?.api_key;
  if (ref?.startsWith(SECRET_PREFIX) && ref.length > SECRET_PREFIX.length) {
    return ref.slice(SECRET_PREFIX.length);
  }
  return providerId;
}

/**
 * Looks a provider up in the catalog, with `[providers.<id>].base_url`
 * overriding the endpoint. A provider that exists only in the config (it has a
 * `base_url`) is treated as an OpenAI-compatible endpoint whose key is optional.
 * Returns null for an unknown provider.
 */
export function resolveProvider(id: string, deps: ProviderDeps): ResolvedProvider | null {
  const entry = deps.catalog.providers.find((p) => p.id === id);
  const override = deps.config.providers[id];
  const baseUrl = override?.base_url ?? entry?.baseUrl;
  if (!baseUrl) return null;
  const apiKey = deps.getSecret(secretIdFor(id, deps.config))?.trim() || undefined;
  return {
    id,
    name: entry?.name ?? id,
    api: entry?.api ?? 'openai',
    baseUrl: baseUrl.replace(/\/+$/, ''),
    keyless: entry ? entry.keyless === true : true,
    apiKey,
    custom: !entry,
  };
}

/** Headers that authenticate a request to the provider; empty when it has no key. */
export function authHeaders(provider: ResolvedProvider): Record<string, string> {
  if (!provider.apiKey) return {};
  switch (provider.api) {
    case 'anthropic':
      return { 'x-api-key': provider.apiKey, 'anthropic-version': ANTHROPIC_VERSION };
    case 'elevenlabs':
      return { 'xi-api-key': provider.apiKey };
    case 'deepgram':
      return { Authorization: `Token ${provider.apiKey}` };
    case 'openai':
    case 'mistral':
      return { Authorization: `Bearer ${provider.apiKey}` };
  }
}
