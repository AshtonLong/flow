/** Layer 1 of text cleanup: instant local rules. Always on, no network. */
import type { Config } from '@shared/config';
import { applyDictionary, dictionaryTerms } from './dictionary';
import { removeFillers } from './fillers';
import { expandSnippets, restoreSnippets, startsWithSnippet } from './snippets';
import { fitToContext } from './spacing';
import { applySpokenFormatting, applySpokenPunctuation } from './spoken';
import { tidy } from './text';

export interface RulesContext {
  /** Text immediately before the cursor, when it can be read. Used for casing/spacing. */
  precedingText?: string;
}

/** Private-use characters the rules use as internal markers; never valid transcript text. */
const MARKERS = /[-]/g;

/** Dictionary → snippets → spoken formatting → filler removal → spacing and casing. Pure. */
export function applyRules(text: string, config: Config, ctx: RulesContext = {}): string {
  if (typeof text !== 'string' || text.trim() === '') return '';
  try {
    let out = text.replace(MARKERS, '').replace(/\r\n?/g, '\n');
    out = applyDictionary(out, config.dictionary);
    // Snippets become opaque slots so the rules below cannot alter saved text.
    const expanded = expandSnippets(out, config.snippets);
    out = expanded.text;
    if (config.cleanup.spoken_formatting) out = applySpokenFormatting(out);
    if (config.cleanup.spoken_punctuation) out = applySpokenPunctuation(out);
    if (config.cleanup.filler_removal) out = removeFillers(out);
    out = tidy(out);
    const snippetFirst = startsWithSnippet(out);
    out = restoreSnippets(out, expanded.slots);
    return fitToContext(out, {
      trailingSpace: config.insert.trailing_space,
      matchCase: config.insert.match_case,
      precedingText: ctx.precedingText,
      keepCase: dictionaryTerms(config.dictionary),
      startsWithSnippet: snippetFirst,
    });
  } catch {
    // A rule must never cost the user a dictation: fall back to the raw transcript.
    return text.trim();
  }
}

/** Dictionary `write` terms, de-duplicated, to pass to speech models as hints. */
export function dictionaryHints(config: Config): string[] {
  return dictionaryTerms(config.dictionary);
}
