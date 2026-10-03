/** Dictionary: replaces misheard words with the spelling the user wants. */
import type { DictionaryEntry } from '@shared/config';
import { PHRASE_END, PHRASE_START, phraseSource } from './text';

interface Rule {
  source: string;
  write: string;
  words: number;
  length: number;
}

function buildRules(dictionary: readonly DictionaryEntry[]): Rule[] {
  const rules: Rule[] = [];
  const seen = new Set<string>();
  for (const entry of dictionary) {
    for (const heard of entry.heard) {
      const source = phraseSource(heard);
      // The first entry to claim a heard phrase wins.
      if (!source || seen.has(source.toLowerCase())) continue;
      seen.add(source.toLowerCase());
      rules.push({
        source,
        write: entry.write,
        words: heard.trim().split(/[\s-]+/).length,
        length: heard.length,
      });
    }
  }
  // Longest phrase first, so "tory app" beats "tory" where both could match.
  return rules.sort((a, b) => b.words - a.words || b.length - a.length);
}

/**
 * Replaces every `heard` phrase with its `write` spelling. Matching ignores
 * case, respects word boundaries (Unicode-aware) and leaves the punctuation
 * around the phrase alone. One pass, so a replacement is never replaced again.
 */
export function applyDictionary(text: string, dictionary: readonly DictionaryEntry[]): string {
  if (!text || dictionary.length === 0) return text;
  const rules = buildRules(dictionary);
  if (rules.length === 0) return text;
  const pattern = new RegExp(
    `${PHRASE_START}(?:${rules.map((r) => `(${r.source})`).join('|')})${PHRASE_END}`,
    'giu',
  );
  return text.replace(pattern, (match: string, ...args: unknown[]) => {
    const index = args.slice(0, rules.length).findIndex((group) => group !== undefined);
    return rules[index]?.write ?? match;
  });
}

/** The `write` terms, trimmed and de-duplicated, in dictionary order. */
export function dictionaryTerms(dictionary: readonly DictionaryEntry[]): string[] {
  const terms = new Set<string>();
  for (const entry of dictionary) {
    const term = entry.write.trim();
    if (term) terms.add(term);
  }
  return [...terms];
}
