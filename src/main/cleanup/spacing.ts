/** Spacing and casing: fits the finished transcript to the text around the cursor. */

export interface FitOptions {
  /** Append one space so the next dictation doesn't run into this one. */
  trailingSpace: boolean;
  /** Match capitalisation and leading space to `precedingText`. */
  matchCase: boolean;
  /** Text immediately before the cursor, when it could be read. */
  precedingText?: string;
  /** Spellings that must keep their capital even mid-sentence (dictionary terms). */
  keepCase?: readonly string[];
  /** The transcript starts with a snippet, whose text is never re-cased. */
  startsWithSnippet?: boolean;
}

const OPENERS = new Set(['(', '[', '{', '<', '“', '‘', '«']);
const AMBIGUOUS_QUOTES = new Set(['"', "'", '`']);

/** True if the text before the cursor stops in the middle of a sentence. */
function endsMidSentence(preceding: string): boolean {
  const before = preceding.replace(/[ \t ]+$/u, '');
  if (before === '') return false;
  return !/(?:[.?!…]["'”’)\]}]*|[\n\r])$/u.test(before);
}

/** True if text typed after `preceding` should not be separated from it by a space. */
function joinsDirectly(preceding: string): boolean {
  const last = preceding.at(-1);
  if (last === undefined || /\s/u.test(last) || OPENERS.has(last)) return true;
  if (AMBIGUOUS_QUOTES.has(last)) {
    // A straight quote opens when nothing, a space or a bracket comes before it.
    const prior = preceding.at(-2);
    return prior === undefined || /\s/u.test(prior) || OPENERS.has(prior);
  }
  return false;
}

/** Lower-cases the first letter unless the first word has to keep its capital. */
function lowerFirst(text: string, keepCase: readonly string[]): string {
  const m = /^(["'“‘(\[]*)(\p{Lu})([\p{L}\p{N}'’]*)/u.exec(text);
  if (!m) return text;
  const [, lead, first, rest] = m as unknown as [string, string, string, string];
  // "I", "I'm", "I'll"...
  if (first === 'I' && (rest === '' || /^['’]/u.test(rest))) return text;
  // Acronyms and names with inner capitals: "API", "GitHub", "B2B".
  if (/[\p{Lu}\p{N}]/u.test(rest)) return text;
  const body = text.slice(lead.length);
  for (const term of keepCase) {
    if (term && body.startsWith(term) && !/^[\p{L}\p{N}_]/u.test(body.slice(term.length))) {
      return text;
    }
  }
  return lead + first.toLowerCase() + text.slice(lead.length + first.length);
}

/**
 * Applies `match_case` and `trailing_space`. With no `precedingText` the casing
 * is left alone: the model's capital is the best guess when the context is unknown.
 */
export function fitToContext(text: string, opts: FitOptions): string {
  if (text === '') return '';
  let out = text;
  const preceding = opts.precedingText;
  if (opts.matchCase && preceding !== undefined) {
    if (!opts.startsWithSnippet && endsMidSentence(preceding)) {
      out = lowerFirst(out, opts.keepCase ?? []);
    }
    const attaches = /^[\s,.;:!?…)\]}]/u.test(out);
    if (preceding !== '' && !attaches && !joinsDirectly(preceding)) out = ' ' + out;
  }
  if (opts.trailingSpace && !/\s$/u.test(out)) out += ' ';
  return out;
}
