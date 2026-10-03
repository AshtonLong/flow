/**
 * Mistral Voxtral transcription. `POST {baseUrl}/audio/transcriptions`, multipart.
 * Docs: https://docs.mistral.ai/api/endpoint/audio/transcriptions
 */
import type { Transcriber } from '@shared/types';
import { type AdapterOptions, cloudTranscriber, field, wavFile } from './base';
import { limitHints } from './hints';

/** "Provide up to 100 words or phrases" (context biasing). */
const MAX_CONTEXT_BIAS = 100;

/**
 * The API schema only accepts bias terms without spaces or commas
 * (`^[^,\s]+$`), so a multi-word hint is sent as its separate words.
 */
export function contextBias(hints: readonly string[]): string[] {
  const words = hints.flatMap((hint) => hint.split(/[\s,]+/u));
  return limitHints(words, { maxTerms: MAX_CONTEXT_BIAS });
}

export function createMistralTranscriber(options: AdapterOptions): Transcriber {
  const { model, provider } = options;
  return cloudTranscriber(
    options,
    (wav, opts) => {
      const form = new FormData();
      form.set('file', wavFile(wav), 'audio.wav');
      form.set('model', model);
      // The schema wants exactly two letters; `en-US` would be rejected.
      const language = /^[a-z]{2}/i.exec(opts.language)?.[0].toLowerCase();
      if (language) form.set('language', language);
      for (const term of contextBias(opts.hints)) form.append('context_bias', term);
      return { url: `${provider.baseUrl}/audio/transcriptions`, body: form };
    },
    (json) => field(json, 'text'),
  );
}
