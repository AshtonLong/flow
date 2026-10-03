/**
 * ElevenLabs Scribe speech-to-text. `POST {baseUrl}/speech-to-text`, multipart, `xi-api-key`.
 * Docs: https://elevenlabs.io/docs/api-reference/speech-to-text/convert
 */
import type { Transcriber } from '@shared/types';
import { type AdapterOptions, cloudTranscriber, field, wavFile } from './base';
import { limitHints } from './hints';

/**
 * The API allows 1000 keyterms, but more than 100 switches on a 20-second
 * minimum billed duration per request, which would dwarf a short dictation.
 */
const MAX_KEYTERMS = 100;

/** "The length of each keyterm must be less than 50 characters", "at most 5 words". */
export function keyterms(hints: readonly string[]): string[] {
  return limitHints(hints, { maxTerms: MAX_KEYTERMS, maxTermChars: 49, maxWords: 5 });
}

export function createElevenLabsTranscriber(options: AdapterOptions): Transcriber {
  const { model, provider } = options;
  return cloudTranscriber(
    options,
    (wav, opts) => {
      const form = new FormData();
      form.set('file', wavFile(wav), 'audio.wav');
      form.set('model_id', model);
      if (opts.language) form.set('language_code', opts.language);
      // Dictation wants words only, not "(laughter)".
      form.set('tag_audio_events', 'false');
      for (const term of keyterms(opts.hints)) form.append('keyterms', term);
      return { url: `${provider.baseUrl}/speech-to-text`, body: form };
    },
    (json) => field(json, 'text'),
  );
}
