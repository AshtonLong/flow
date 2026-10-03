/**
 * Spoken formatting ("new line", "new paragraph") and spoken punctuation
 * ("comma", "question mark", ...).
 *
 * Speech models such as Parakeet already punctuate, and every command here is
 * also an ordinary English word, so conversion is deliberately conservative.
 * A phrase is treated as a command unless its neighbours show it is being
 * used as a noun or verb:
 *
 * - a determiner or typical modifier directly before it ("a comma", "the
 *   Oxford comma", "a new line of code", "trial period");
 * - a word directly after it that continues a noun phrase ("comma separated",
 *   "colon cancer", "a dash of salt", "question mark key").
 *
 * "period" is the most common ordinary word of the set, so it must also sit at
 * the edge of a clause: at the end of the utterance, next to punctuation the
 * model wrote, before a line break, or before a capitalised word. That is why
 * "the Jurassic period was long" is left alone while "that is all period"
 * becomes "that is all."
 *
 * Punctuation the model placed around a command ("Hello. New line. How") is
 * absorbed so the result isn't doubled.
 */
import {
  CAP,
  COMMAND_END,
  COMMAND_START,
  applyCapitalMarks,
  tidy,
  wordAfter,
  wordBefore,
} from './text';

/** Words that make the following command a noun: "a comma", "this period". */
// prettier-ignore
const DETERMINERS = new Set([
  'a', 'an', 'the', 'this', 'that', 'these', 'those', 'each', 'every', 'any', 'no', 'another',
  'one', 'some', 'which', 'what', 'my', 'your', 'his', 'her', 'its', 'our', 'their', 'per',
  'single', 'double', 'extra', 'missing', 'word', 'words',
]);

/** Words that turn the preceding command into a noun phrase: "comma key". */
const NOUN_FOLLOWERS = new Set(['key', 'keys', 'character', 'characters', 'symbol', 'symbols']);

/** Words after "new line" that show it is a noun: "a new line of code". */
const LINE_FOLLOWERS = new Set(['of', 'character', 'characters', 'break', 'breaks', 'item']);

const BREAK_PATTERN = new RegExp(
  `[ \\t]*([,;:]?)[ \\t]*${COMMAND_START}(new[ \\t-]?paragraph|new[ \\t-]?line)${COMMAND_END}[ \\t]*[.,!?;:…]*[ \\t]*`,
  'giu',
);

/**
 * Converts "new line" to a line break and "new paragraph" to a blank line,
 * absorbing the spaces and punctuation around the command and capitalising
 * the word that follows.
 */
export function applySpokenFormatting(text: string): string {
  if (!text) return text;
  const out = text.replace(
    BREAK_PATTERN,
    (match: string, comma: string, command: string, offset: number, whole: string) => {
      const previous = comma ? null : wordBefore(whole, offset);
      if (previous && DETERMINERS.has(previous)) return match;
      const next = /[.,!?;:…][ \t]*$/u.test(match) ? null : wordAfter(whole, offset + match.length);
      if (next && LINE_FOLLOWERS.has(next.toLowerCase())) return match;
      return (/paragraph/i.test(command) ? '\n\n' : '\n') + CAP;
    },
  );
  return tidy(applyCapitalMarks(out));
}

type Kind = 'stop' | 'join' | 'open' | 'close' | 'dash' | 'hyphen';

interface Command {
  phrases: string[];
  kind: Kind;
  symbol: string;
  /** Words directly before the phrase that mark it as an ordinary noun. */
  before?: string[];
  /** Words directly after the phrase that mark it as an ordinary noun or verb. */
  after?: string[];
  /** Only a command at the edge of a clause (see the file comment). */
  edgeOnly?: boolean;
}

const COMMANDS: Command[] = [
  { phrases: ['question mark'], kind: 'stop', symbol: '?' },
  { phrases: ['exclamation mark', 'exclamation point'], kind: 'stop', symbol: '!' },
  { phrases: ['full stop'], kind: 'stop', symbol: '.', before: ['complete', 'dead', 'sudden'] },
  {
    phrases: ['period'],
    kind: 'stop',
    symbol: '.',
    edgeOnly: true,
    // prettier-ignore
    before: [
      'time', 'trial', 'grace', 'class', 'waiting', 'notice', 'probation', 'probationary',
      'cooling-off', 'long', 'short', 'brief', 'extended', 'same', 'whole', 'entire', 'first',
      'second', 'third', 'fourth', 'last', 'next', 'previous', 'current', 'given', 'certain',
      'incubation', 'transition', 'reporting', 'billing', 'pay', 'rest', 'free', 'lunch', 'exam',
      'review', 'comment', 'warranty', 'return', 'refund', 'blackout', 'quiet', 'jurassic',
      'cretaceous', 'triassic', 'victorian', 'edwardian', 'colonial', 'medieval', 'of',
    ],
    after: ['of', 'drama', 'dramas', 'piece', 'pieces', 'costume', 'costumes'],
  },
  {
    phrases: ['comma'],
    kind: 'join',
    symbol: ',',
    before: ['oxford', 'serial', 'trailing', 'leading', 'inverted'],
    after: ['separated', 'delimited', 'splice', 'splices'],
  },
  { phrases: ['semi colon'], kind: 'join', symbol: ';' },
  {
    phrases: ['colon'],
    kind: 'join',
    symbol: ':',
    after: ['cancer', 'cleanse', 'surgery', 'polyp', 'polyps', 'health'],
  },
  { phrases: ['ellipsis', 'dot dot dot'], kind: 'join', symbol: '...' },
  { phrases: ['open quote', 'open quotes', 'begin quote'], kind: 'open', symbol: '"' },
  { phrases: ['close quote', 'close quotes', 'end quote', 'unquote'], kind: 'close', symbol: '"' },
  { phrases: ['open paren', 'open parenthesis', 'open parentheses'], kind: 'open', symbol: '(' },
  {
    phrases: ['close paren', 'close parenthesis', 'close parentheses'],
    kind: 'close',
    symbol: ')',
  },
  { phrases: ['em dash'], kind: 'dash', symbol: '—' },
  {
    phrases: ['dash'],
    kind: 'dash',
    symbol: '-',
    before: ['mad', 'quick', 'to', 'will', 'must', 'gotta', "i'll", "we'll", "let's"],
    after: ['of', 'to', 'cam', 'off', 'out', 'for', 'into', 'across', 'through', 'over', 'back'],
  },
  { phrases: ['hyphen'], kind: 'hyphen', symbol: '-' },
];

/** Commands keyed by phrase with the spaces removed, so "semi-colon" and "semicolon" both hit. */
const BY_PHRASE = new Map<string, Command>();
const PHRASE_SOURCES: string[] = [];
for (const command of COMMANDS) {
  for (const phrase of command.phrases) {
    BY_PHRASE.set(phrase.replace(/ /g, ''), command);
    PHRASE_SOURCES.push(phrase.replace(/ /g, '[ \\t-]*'));
  }
}
PHRASE_SOURCES.sort((a, b) => b.length - a.length);

const PUNCTUATION_PATTERN = new RegExp(
  `([ \\t]*(?:[,.;:!?…]+[ \\t]*)?)${COMMAND_START}(${PHRASE_SOURCES.join('|')})${COMMAND_END}([ \\t]*[,.;:!?…]*[ \\t]*)`,
  'giu',
);

function lookUp(spoken: string): Command | undefined {
  return BY_PHRASE.get(spoken.toLowerCase().replace(/[ \t-]+/g, ''));
}

/** True when the neighbouring words show the phrase is an ordinary noun or verb. */
function isOrdinaryWord(command: Command, previous: string | null, next: string | null): boolean {
  if (previous && (DETERMINERS.has(previous) || command.before?.includes(previous))) return true;
  const after = next?.toLowerCase();
  if (after && (NOUN_FOLLOWERS.has(after) || command.after?.includes(after))) return true;
  return false;
}

/** Converts spoken punctuation commands to symbols. See the file comment for the heuristic. */
export function applySpokenPunctuation(text: string): string {
  if (!text) return text;
  const out = text.replace(
    PUNCTUATION_PATTERN,
    (match: string, lead: string, spoken: string, trail: string, offset: number, whole: string) => {
      const command = lookUp(spoken);
      if (!command) return match;
      const leadMark = lead.trim();
      const trailMark = trail.trim();
      const end = offset + match.length;
      const previous = leadMark ? null : wordBefore(whole, offset);
      const next = trailMark ? null : wordAfter(whole, end);
      if (isOrdinaryWord(command, previous, next)) return match;

      const rest = whole.slice(end);
      const atEnd = rest.trim() === '';
      if (command.edgeOnly) {
        const startsSentence = next !== null && /^\p{Lu}/u.test(next);
        const atEdge = atEnd || leadMark !== '' || trailMark !== '' || /^\n/.test(rest);
        if (!atEdge && !startsSentence) return match;
        // "the Edo period.": a determiner, then a proper adjective, is a noun phrase.
        if (!leadMark && previous) {
          const before = whole.slice(0, offset);
          const m = /([\p{L}'’-]+)[ \t]+([\p{L}\p{N}'’-]+)[ \t]*$/u.exec(before);
          if (m && DETERMINERS.has(m[1]!.toLowerCase()) && /^\p{Lu}/u.test(m[2]!)) return match;
        }
      }

      switch (command.kind) {
        case 'stop':
          return command.symbol + CAP + ' ';
        case 'join':
          return command.symbol + ' ';
        case 'dash':
          return ` ${command.symbol} `;
        case 'hyphen':
          return command.symbol;
        case 'open':
          // Keeps the model's "He said," before the quote; whatever followed the command goes.
          return leadMark + (offset > 0 || leadMark ? ' ' : '') + command.symbol;
        case 'close': {
          // "hello. Close quote" keeps its full stop inside; a comma before the command was
          // only the model marking a pause.
          const inside = /[.!?…]/u.test(leadMark) ? leadMark : '';
          return inside + command.symbol + (inside ? '' : trailMark) + ' ';
        }
      }
    },
  );
  return tidy(applyCapitalMarks(out));
}
