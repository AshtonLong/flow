/**
 * OpenAI-compatible transcription: Groq, OpenAI and custom endpoints.
 * `POST {baseUrl}/audio/transcriptions`, multipart.
 * Docs: https://console.groq.com/docs/speech-to-text
 *       https://developers.openai.com/api/docs/guides/speech-to-text
 */
import type { Transcriber } from '@shared/types';
import { type AdapterOptions, cloudTranscriber, field, wavFile } from './base';
import { limitHints } from './hints';

/**
 * Whisper reads at most 224 tokens of prompt (Groq and OpenAI both document
 * it). Names tokenise badly, so budget about three characters per token.
 */
const PROMPT_CHAR_BUDGET = 600;

/** `gpt-transcribe` takes a keyword list instead; keep it to a sensible size. */
const MAX_KEYWORDS = 100;

/** `gpt-transcribe` replaced `language` with `languages[]` and added `keywords[]`. */
function usesKeywordFields(model: string): boolean {
  return /^gpt-transcribe/i.test(model);
}

/** The hint terms as a Whisper-style prompt: a short glossary the model treats as prior text. */
export function hintPrompt(hints: readonly string[]): string {
  return limitHints(hints, { maxTotalChars: PROMPT_CHAR_BUDGET }).join(', ');
}

export function createOpenAiCompatibleTranscriber(options: AdapterOptions): Transcriber {
  const { model, provider } = options;
  return cloudTranscriber(
    options,
    (wav, opts) => {
      const form = new FormData();
      form.set('file', wavFile(wav), 'audio.wav');
      form.set('model', model);
      if (usesKeywordFields(model)) {
        if (opts.language) form.append('languages[]', opts.language);
        // Keywords must stay on one line and may not contain angle brackets.
        const keywords = limitHints(
          opts.hints.map((h) => h.replace(/[<>\r\n]/g, ' ')),
          { maxTerms: MAX_KEYWORDS },
        );
        for (const keyword of keywords) form.append('keywords[]', keyword);
      } else {
        form.set('response_format', 'json');
        if (opts.language) form.set('language', opts.language);
        const prompt = hintPrompt(opts.hints);
        if (prompt) form.set('prompt', prompt);
      }
      return { url: `${provider.baseUrl}/audio/transcriptions`, body: form };
    },
    (json) => field(json, 'text'),
  );
}
