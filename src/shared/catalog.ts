/** The model catalog manifest: local models, cloud models, cleanup models and providers. */
import { z } from 'zod';

/** How a provider's HTTP API is spoken. One adapter exists per value. */
export const PROVIDER_APIS = ['openai', 'mistral', 'elevenlabs', 'deepgram', 'anthropic'] as const;
export type ProviderApi = (typeof PROVIDER_APIS)[number];

export const ProviderEntrySchema = z.object({
  id: z.string(),
  name: z.string(),
  api: z.enum(PROVIDER_APIS),
  /** Base URL without a trailing slash, e.g. `https://api.groq.com/openai/v1`. */
  baseUrl: z.string(),
  /** Where the user creates an API key. */
  keyUrl: z.string().optional(),
  /** Needs no key (local endpoints such as Ollama). */
  keyless: z.boolean().optional(),
});
export type ProviderEntry = z.infer<typeof ProviderEntrySchema>;

export const LocalModelEntrySchema = z.object({
  kind: z.literal('local'),
  id: z.string(),
  name: z.string(),
  family: z.string(),
  /** "Pick it for…" line shown in the catalog. */
  description: z.string(),
  url: z.string(),
  /** File name under the models directory. */
  file: z.string(),
  sha256: z.string().length(64),
  sizeBytes: z.number(),
  languages: z.array(z.string()),
  licence: z.string(),
  licenceUrl: z.string().optional(),
  /** Hardware the model is practical on. */
  runsOn: z.enum(['cpu', 'gpu']),
  /** `core` ships in v1; `accuracy` is the large GPU tier. */
  tier: z.enum(['core', 'accuracy']),
  /** Average WER on the Open ASR Leaderboard, in percent. */
  wer: z.number().optional(),
  /** The user must accept terms on the model page before downloading. */
  gated: z.boolean().optional(),
  note: z.string().optional(),
});
export type LocalModelEntry = z.infer<typeof LocalModelEntrySchema>;

export const CloudModelEntrySchema = z.object({
  kind: z.literal('cloud'),
  /** `<provider>/<model>`, e.g. `groq/whisper-large-v3-turbo`. */
  id: z.string(),
  provider: z.string(),
  /** Model name sent to the provider. */
  model: z.string(),
  name: z.string(),
  description: z.string(),
  /** USD per hour of audio, list price. */
  pricePerHour: z.number(),
  recommended: z.boolean().optional(),
  /** Supports dictionary terms as a hint. */
  hints: z.boolean().optional(),
  /** Minimum billed seconds per request. */
  minBilledSeconds: z.number().optional(),
  docsUrl: z.string().optional(),
});
export type CloudModelEntry = z.infer<typeof CloudModelEntrySchema>;

export const CleanupModelEntrySchema = z.object({
  kind: z.literal('cleanup'),
  /** `<provider>/<model>`, e.g. `groq/openai/gpt-oss-20b`. */
  id: z.string(),
  provider: z.string(),
  model: z.string(),
  name: z.string(),
  description: z.string(),
  /** USD per million tokens. */
  priceIn: z.number(),
  priceOut: z.number(),
  recommended: z.boolean().optional(),
});
export type CleanupModelEntry = z.infer<typeof CleanupModelEntrySchema>;

export const CatalogSchema = z.object({
  version: z.number().int(),
  /** Date the prices were read, `YYYY-MM-DD`. */
  asOf: z.string(),
  providers: z.array(ProviderEntrySchema),
  local: z.array(LocalModelEntrySchema),
  cloud: z.array(CloudModelEntrySchema),
  cleanup: z.array(CleanupModelEntrySchema),
});
export type Catalog = z.infer<typeof CatalogSchema>;

export type ModelEntry = LocalModelEntry | CloudModelEntry;

export interface DownloadProgress {
  id: string;
  receivedBytes: number;
  totalBytes: number;
  /** Bytes per second, smoothed. */
  speed: number;
  state: 'downloading' | 'verifying' | 'done' | 'error' | 'cancelled';
  error?: string;
}

/** Runtime status of one model, merged with its catalog entry for the UI. */
export interface ModelStatus {
  id: string;
  /** Local: file present and verified. Cloud: the provider has a key (or needs none). */
  ready: boolean;
  active: boolean;
  /** Local model currently held in engine memory. */
  loaded: boolean;
  download?: DownloadProgress;
  /** Registered by the user in `[[local_models]]` or as a custom endpoint. */
  custom?: boolean;
}

export interface CatalogSnapshot {
  catalog: Catalog;
  status: Record<string, ModelStatus>;
  /** Provider id → has a stored key. */
  keys: Record<string, boolean>;
  /** User-registered entries (custom local files and custom endpoints). */
  custom: ModelEntry[];
}

/** Prefix of model ids served by a custom OpenAI-compatible endpoint: `custom/<provider-id>`. */
export const CUSTOM_PREFIX = 'custom/';

/** Splits `groq/openai/gpt-oss-20b` into provider `groq` and model `openai/gpt-oss-20b`. */
export function splitModelId(id: string): { provider: string; model: string } | null {
  const slash = id.indexOf('/');
  if (slash <= 0) return null;
  return { provider: id.slice(0, slash), model: id.slice(slash + 1) };
}

/** Local model ids contain no slash; cloud ids are `<provider>/<model>`. */
export function isCloudModelId(id: string): boolean {
  return id.includes('/');
}

/** Cost in USD of transcribing `audioMs` of audio on a cloud model. */
export function cloudCost(entry: CloudModelEntry, audioMs: number): number {
  const seconds = Math.max(audioMs / 1000, entry.minBilledSeconds ?? 0);
  return (seconds / 3600) * entry.pricePerHour;
}
