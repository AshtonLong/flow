/** Regex building blocks and small string helpers shared by the cleanup rules. */

/** Characters that count as part of a word (Unicode letters, digits, underscore). */
const WORD = '\\p{L}\\p{N}_';

/**
 * Lookbehind for the start of a spoken phrase: not inside a larger word, and
 * not the tail of an elided word such as `o'clock`.
 */
export const PHRASE_START = `(?<![${WORD}])(?<![\\p{L}\\p{N}]['’])`;

/**
 * Lookahead for the end of a spoken phrase: not inside a larger word. A
 * possessive `'s` may follow (`Tory's`), any other contraction may not (`don't`).
 */
export const PHRASE_END = `(?![${WORD}])(?!['’](?!s(?![${WORD}]))\\p{L})`;

/** Stricter boundaries for command words: a hyphenated compound is not a command. */
export const COMMAND_START = `(?<![${WORD}'’-])`;
export const COMMAND_END = `(?![${WORD}]|['’-][\\p{L}\\p{N}])`;

/**
 * Private-use marker meaning "capitalise the next letter". Rules insert it
 * while rewriting and resolve it with `applyCapitalMarks` before returning.
 */
export const CAP = '';

export function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Regex source that matches a phrase the way a speech model might write it:
 * any run of spaces between words, a hyphen or a comma in place of the space,
 * and either apostrophe style. Returns null for a phrase with no words.
 */
export function phraseSource(phrase: string): string | null {
  const words = phrase
    .trim()
    .split(/[\s -]+/u)
    .filter((w) => w.length > 0);
  if (words.length === 0) return null;
  return words
    .map((w) => escapeRegExp(w).replace(/['’]/g, "['’]"))
    .join('(?:[\\s\\u00A0]*[-–,][\\s\\u00A0]*|[\\s\\u00A0]+)');
}

/** The word directly before `index`, lower-cased, if only spaces separate them. */
export function wordBefore(text: string, index: number): string | null {
  const m = /([\p{L}\p{N}'’-]+)[ \t]*$/u.exec(text.slice(Math.max(0, index - 64), index));
  return m ? m[1]!.toLowerCase() : null;
}

/** The word directly after `index`, in its original case, if only spaces separate them. */
export function wordAfter(text: string, index: number): string | null {
  const m = /^[ \t]*([\p{L}\p{N}'’-]+)/u.exec(text.slice(index, index + 64));
  return m ? m[1]! : null;
}

/** Upper-cases the letter after each capitalisation mark and removes the marks. */
export function applyCapitalMarks(text: string): string {
  return text
    .replace(
      new RegExp(`${CAP}((?:${CAP}|[\\s"'“‘(\\[])*)(\\p{Ll})`, 'gu'),
      (_m, gap: string, letter: string) => gap + letter.toUpperCase(),
    )
    .replaceAll(CAP, '');
}

/**
 * Normalises whitespace: one space between words, none before punctuation that
 * closes a clause, none around line breaks, none at either end. Line breaks
 * are kept.
 */
export function tidy(text: string): string {
  return (
    text
      .replace(/[ \t ]+/g, ' ')
      // Only before punctuation that ends a clause, so ` .NET` and ` :)` survive.
      .replace(/ +([,.;:!?…]+)(?=\s|$|["”’])/g, '$1')
      .replace(/ *\n */g, '\n')
      .replace(/^ +| +$/g, '')
  );
}
