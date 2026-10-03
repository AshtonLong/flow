/**
 * Deepgram pre-recorded transcription. `POST {baseUrl}/listen` with the audio
 * as the raw request body and options in the query string.
 * Docs: https://developers.deepgram.com/docs/pre-recorded-audio
 *       https://developers.deepgram.com/docs/keyterm
 */
import type { Transcriber } from '@shared/types';
import { type AdapterOptions, cloudTranscriber, field } from './base';
import { limitHints } from './hints';

/** "Maximum of 100 important terms per request". */
const MAX_KEYTERMS = 100;
/** Keyterms are limited to 500 tokens per request; allow about two characters per token. */
const KEYTERM_CHAR_BUDGET = 1000;

/** Keyterm prompting exists on Nova-3 (and Flux); older models reject the parameter. */
function supportsKeyterms(model: string): boolean {
  return /^(nova-3|flux)/i.test(model);
}

export function createDeepgramTranscriber(options: AdapterOptions): Transcriber {
  const { model, provider } = options;
  return cloudTranscriber(
    options,
    (wav, opts) => {
      const query = new URLSearchParams({ model, smart_format: 'true' });
      if (opts.language) query.set('language', opts.language);
      if (supportsKeyterms(model)) {
        const terms = limitHints(opts.hints, {
          maxTerms: MAX_KEYTERMS,
          maxTotalChars: KEYTERM_CHAR_BUDGET,
        });
        for (const term of terms) query.append('keyterm', term);
      }
      return {
        url: `${provider.baseUrl}/listen?${query.toString()}`,
        headers: { 'Content-Type': 'audio/wav' },
        body: wav,
      };
    },
    (json) =>
      field(
        field(field(field(field(field(json, 'results'), 'channels'), 0), 'alternatives'), 0),
        'transcript',
      ),
  );
}
