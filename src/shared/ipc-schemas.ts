/** Validators for every message a renderer can send. Used only in the main process. */
import { z } from 'zod';

const pathSchema = z.array(z.union([z.string(), z.number()])).min(1);

export const ConfigPatchesSchema = z
  .array(z.object({ path: pathSchema, value: z.unknown() }))
  .max(200);

export const ModelIdSchema = z.string().min(1).max(200);
export const ProviderIdSchema = z
  .string()
  .min(1)
  .max(64)
  .regex(/^[a-z0-9][a-z0-9._-]*$/i);
export const SecretSetSchema = z.object({ provider: ProviderIdSchema, key: z.string().max(1000) });

/** 16 kHz mono PCM, capped at ten minutes. */
export const PcmSchema = z.instanceof(Float32Array).refine((a) => a.length <= 16000 * 600, {
  message: 'audio too long',
});

export const BenchRunSchema = z.object({
  audio: PcmSchema,
  modelIds: z.array(ModelIdSchema).min(1).max(24),
});

export const HistorySearchSchema = z.object({
  query: z.string().max(500),
  limit: z.number().int().min(1).max(500),
  offset: z.number().int().min(0),
});

export const CaptureErrorSchema = z.object({
  reason: z.enum(['device-missing', 'permission', 'busy', 'unknown']),
  message: z.string().max(500).optional(),
});

export const ExternalUrlSchema = z.string().url().startsWith('https://');
