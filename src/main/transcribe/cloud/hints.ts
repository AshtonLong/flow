/** Trims dictionary hints to what a provider documents for its prompt or keyterm list. */

export interface HintLimits {
  /** Most terms to send. */
  maxTerms?: number;
  /** Budget for all terms together, in characters. */
  maxTotalChars?: number;
  /** Terms at least this long are dropped. */
  maxTermChars?: number;
  /** Terms with more words than this are dropped. */
  maxWords?: number;
}

/**
 * Cleans, de-duplicates (ignoring case) and caps a hint list. Whole terms are
 * kept or dropped, never cut, and earlier terms win when the budget runs out.
 */
export function limitHints(hints: readonly string[], limits: HintLimits = {}): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  let total = 0;
  for (const raw of hints) {
    if (typeof raw !== 'string') continue;
    const term = raw.replace(/\s+/g, ' ').trim();
    const key = term.toLowerCase();
    if (!term || seen.has(key)) continue;
    if (limits.maxTermChars !== undefined && term.length > limits.maxTermChars) continue;
    if (limits.maxWords !== undefined && term.split(' ').length > limits.maxWords) continue;
    if (limits.maxTerms !== undefined && out.length >= limits.maxTerms) break;
    if (limits.maxTotalChars !== undefined && total + term.length > limits.maxTotalChars) continue;
    seen.add(key);
    out.push(term);
    total += term.length + 2;
  }
  return out;
}
