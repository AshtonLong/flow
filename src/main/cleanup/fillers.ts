/** Filler removal: strips "um"/"uh"-style sounds and collapses stuttered words. */
import { CAP, COMMAND_END, COMMAND_START, applyCapitalMarks, tidy } from './text';

/** um, umm, uh, uhh, uhm, er, erm, hm, hmm. Deliberately not "ah", "eh" or "mm" (a unit). */
const FILLER = '(?:u+m+|u+h+m*|erm+|er|h+m+)';

/** A run of fillers, with whatever commas or pauses the model put between them. */
const FILLER_RUN = `${FILLER}${COMMAND_END}(?:[ \\t]*(?:[,;]|\\.{2,}|…)?[ \\t]*${COMMAND_START}${FILLER}${COMMAND_END})*`;

const FILLER_PATTERN = new RegExp(
  `([ \\t]*(?:(?:[,;]|\\.{2,}|…)[ \\t]*)?)${COMMAND_START}(${FILLER_RUN})([ \\t]*(?:[,;]|\\.{2,}|…|[-–—](?![\\p{L}\\p{N}]))?[ \\t]*)([.!?]+)?`,
  'giu',
);

/**
 * Words that are legitimately said twice in a row ("she had had enough",
 * "I know that that is true", "very very good"). Everything else is treated as
 * a stutter.
 */
// prettier-ignore
const KEEP_DOUBLED = new Set([
  'had', 'that', 'is', 'do', 'her', 'very', 'really', 'so', 'no', 'yes', 'yeah', 'ok', 'okay',
  'bye', 'ha', 'there', 'now', 'well', 'many', 'much', 'far', 'long', 'never', 'hear', 'knock',
  'blah',
]);

/** Spoken digits repeat in phone numbers and codes. */
const NUMBER_WORD =
  /^(?:\p{N}+|zero|oh|one|two|three|four|five|six|seven|eight|nine|ten|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|million)$/iu;

const REPEAT_PATTERN = new RegExp(
  `${COMMAND_START}([\\p{L}\\p{N}'’]+)((?:[ \\t]*,?[ \\t]+\\1)+)${COMMAND_END}`,
  'giu',
);

function stripFillers(text: string): string {
  return text.replace(
    FILLER_PATTERN,
    (
      match: string,
      lead: string,
      fillers: string,
      _trail: string,
      stop: string | undefined,
      offset: number,
      whole: string,
    ) => {
      // "ER" and "UM" in capitals are abbreviations, not hesitations.
      if (fillers.length > 1 && fillers === fillers.toUpperCase()) return match;
      const before = whole.slice(0, offset);
      const startsSentence = lead.trim() === '' && /(?:^|[.!?\n]["'”’)\]]*)[ \t]*$/u.test(before);
      if (startsSentence) {
        // "Um. I think" and "Um, I think" both leave "I think".
        return (before === '' || /\n$/.test(before) ? '' : ' ') + CAP;
      }
      return stop ?? ' ';
    },
  );
}

function collapseRepeats(text: string): string {
  return text.replace(REPEAT_PATTERN, (match: string, word: string) => {
    if (KEEP_DOUBLED.has(word.toLowerCase()) || NUMBER_WORD.test(word)) return match;
    return word;
  });
}

/**
 * Removes filler sounds along with the commas around them, collapses
 * immediately repeated words ("I I think" → "I think"), and repairs the
 * spacing and capitalisation left behind.
 */
export function removeFillers(text: string): string {
  if (!text) return text;
  return tidy(collapseRepeats(tidy(applyCapitalMarks(stripFillers(text)))));
}
