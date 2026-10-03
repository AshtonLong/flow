/** Snippets: a spoken trigger phrase expands to saved text. */
import type { Snippet } from '@shared/config';
import { PHRASE_END, PHRASE_START, phraseSource } from './text';

const SLOT_OPEN = '';
const SLOT_CLOSE = '';

/** Punctuation and quotes a speech model may wrap around a lone trigger. */
const WRAP_BEFORE = '[\\s"\'“”‘’(\\[]*';
const WRAP_AFTER = '[\\s.,!?;:…"\'“”‘’)\\]]*';

export interface ExpandedSnippets {
  /** The text with each expansion replaced by an opaque slot marker. */
  text: string;
  /** Slot contents, restored with `restoreSnippets`. */
  slots: string[];
}

interface Rule {
  source: string;
  text: string;
  length: number;
}

function buildRules(snippets: readonly Snippet[]): Rule[] {
  const rules: Rule[] = [];
  const seen = new Set<string>();
  for (const snippet of snippets) {
    const source = phraseSource(snippet.trigger.replace(/[.,!?;:…]+$/u, ''));
    if (!source || seen.has(source.toLowerCase())) continue;
    seen.add(source.toLowerCase());
    rules.push({ source, text: snippet.text, length: snippet.trigger.length });
  }
  return rules.sort((a, b) => b.length - a.length);
}

/**
 * Replaces each trigger with a slot marker and returns the saved texts
 * separately, so later rules (filler removal, spacing) cannot alter a snippet.
 * An utterance that is only the trigger, give or take punctuation, becomes
 * just the snippet.
 */
export function expandSnippets(text: string, snippets: readonly Snippet[]): ExpandedSnippets {
  const slots: string[] = [];
  if (!text || snippets.length === 0) return { text, slots };
  const rules = buildRules(snippets);
  if (rules.length === 0) return { text, slots };
  const slot = (value: string): string => {
    slots.push(value);
    return `${SLOT_OPEN}${slots.length - 1}${SLOT_CLOSE}`;
  };

  for (const rule of rules) {
    const whole = new RegExp(`^${WRAP_BEFORE}${rule.source}${WRAP_AFTER}$`, 'iu');
    if (whole.test(text)) return { text: slot(rule.text), slots };
  }

  const pattern = new RegExp(
    `${PHRASE_START}(?:${rules.map((r) => `(${r.source})`).join('|')})${PHRASE_END}([.,!?;:…]*)([ \\t]*)`,
    'giu',
  );
  const out = text.replace(pattern, (match: string, ...args: unknown[]) => {
    const rule = rules[args.slice(0, rules.length).findIndex((group) => group !== undefined)];
    if (!rule) return match;
    const punctuation = (args[rules.length] as string | undefined) ?? '';
    // A snippet that already ends a clause (or a line) doesn't take the model's punctuation too.
    const selfTerminated = /[.,!?;:…\s]$/u.test(rule.text);
    const space = (args[rules.length + 1] as string | undefined) ?? '';
    // After a snippet that ends in a line break, the next word starts the new line.
    const gap = /\s$/u.test(rule.text) ? '' : space;
    return slot(rule.text) + (selfTerminated ? '' : punctuation) + gap;
  });
  return { text: out, slots };
}

/** Puts the saved texts back in place of their slot markers. */
export function restoreSnippets(text: string, slots: readonly string[]): string {
  if (slots.length === 0) return text;
  return text.replace(
    new RegExp(`${SLOT_OPEN}(\\d+)${SLOT_CLOSE}`, 'g'),
    (_m, index: string) => slots[Number(index)] ?? '',
  );
}

/** True if the text begins with a snippet slot (its casing must not be touched). */
export function startsWithSnippet(text: string): boolean {
  return text.startsWith(SLOT_OPEN);
}

/** Expands triggers in one step, for callers with no further rules to run. */
export function applySnippets(text: string, snippets: readonly Snippet[]): string {
  const expanded = expandSnippets(text, snippets);
  return restoreSnippets(expanded.text, expanded.slots);
}
