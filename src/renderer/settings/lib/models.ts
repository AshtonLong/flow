/** Lookups over the catalog snapshot. */
import type {
  CatalogSnapshot,
  CloudModelEntry,
  LocalModelEntry,
  ModelEntry,
  ProviderEntry,
} from '@shared/catalog';
import { CUSTOM_PREFIX, splitModelId } from '@shared/catalog';
import type { Config } from '@shared/config';
import type { BackendDevice } from '@shared/types';

export function allModels(snapshot: CatalogSnapshot): ModelEntry[] {
  return [...snapshot.catalog.local, ...snapshot.catalog.cloud, ...snapshot.custom];
}

export function findModel(snapshot: CatalogSnapshot | null, id: string): ModelEntry | undefined {
  if (!snapshot) return undefined;
  return allModels(snapshot).find((m) => m.id === id);
}

export function findProvider(
  snapshot: CatalogSnapshot | null,
  id: string,
): ProviderEntry | undefined {
  return snapshot?.catalog.providers.find((p) => p.id === id);
}

/** A name that stands on its own in a select or the status footer: `Groq: Whisper large-v3-turbo`. */
export function modelLabel(snapshot: CatalogSnapshot | null, id: string): string {
  const entry = findModel(snapshot, id);
  if (!entry) {
    return id.startsWith(CUSTOM_PREFIX) ? `Custom: ${id.slice(CUSTOM_PREFIX.length)}` : id;
  }
  if (entry.kind === 'local') return entry.name;
  const provider = findProvider(snapshot, entry.provider)?.name;
  if (!provider || entry.name.toLowerCase().startsWith(provider.toLowerCase())) return entry.name;
  return `${provider}: ${entry.name}`;
}

export function isReady(snapshot: CatalogSnapshot, id: string): boolean {
  return snapshot.status[id]?.ready ?? false;
}

/** Models that can transcribe right now: installed local, connected cloud, custom endpoints. */
export function readyModels(snapshot: CatalogSnapshot): ModelEntry[] {
  return allModels(snapshot).filter((m) => isReady(snapshot, m.id));
}

export function installedLocal(snapshot: CatalogSnapshot): LocalModelEntry[] {
  return allModels(snapshot).filter(
    (m): m is LocalModelEntry => m.kind === 'local' && isReady(snapshot, m.id),
  );
}

export function cloudByProvider(
  snapshot: CatalogSnapshot,
): { provider: ProviderEntry; models: CloudModelEntry[] }[] {
  const groups: { provider: ProviderEntry; models: CloudModelEntry[] }[] = [];
  for (const provider of snapshot.catalog.providers) {
    const models = snapshot.catalog.cloud.filter((m) => m.provider === provider.id);
    if (models.length > 0) groups.push({ provider, models });
  }
  return groups;
}

/** Whether a provider can be called: it has a stored key or needs none. */
export function providerConnected(snapshot: CatalogSnapshot, providerId: string): boolean {
  if (snapshot.keys[providerId]) return true;
  return findProvider(snapshot, providerId)?.keyless ?? false;
}

export interface CustomEndpoint {
  id: string;
  baseUrl: string;
  model: string;
  hasKeyRef: boolean;
}

/** Custom endpoints are `[providers.<id>]` tables with a `base_url` that are not catalog providers. */
export function customEndpoints(
  config: Config,
  snapshot: CatalogSnapshot | null,
): CustomEndpoint[] {
  const known = new Set(snapshot?.catalog.providers.map((p) => p.id) ?? []);
  return Object.entries(config.providers)
    .filter(([id, p]) => !known.has(id) && Boolean(p.base_url))
    .map(([id, p]) => ({
      id,
      baseUrl: p.base_url ?? '',
      model: p.model ?? '',
      hasKeyRef: Boolean(p.api_key),
    }));
}

export function providerOfModel(id: string): string | null {
  return splitModelId(id)?.provider ?? null;
}

export function isGpuDevice(device: BackendDevice): boolean {
  return /gpu|cuda|vulkan|metal/i.test(`${device.deviceType} ${device.kind}`);
}
