import { describe, expect, it } from 'vitest';
import { applySpokenFormatting, applySpokenPunctuation } from '../../../src/main/cleanup/spoken';

describe('applySpokenFormatting', () => {
  it.each([
    [
      'absorbs the punctuation around the command',
      'Hello. New line. How are you',
      'Hello.\nHow are you',
    ],
    ['drops the commas around the command', 'Hello, new line, how are you', 'Hello\nHow are you'],
    ['works without punctuation', 'hello new line how are you', 'hello\nHow are you'],
    ['converts new paragraph to a blank line', 'First new paragraph second', 'First\n\nSecond'],
    ['handles a command at the end', 'Hello. New paragraph.', 'Hello.\n\n'],
    ['handles a command at the start', 'New line. Hello', '\nHello'],
    ['handles consecutive commands', 'first new line new line second', 'first\n\nSecond'],
    ['accepts the one-word spelling', 'done newline next', 'done\nNext'],
    ['keeps a question mark before the command', 'Really? New line. Yes', 'Really?\nYes'],
    ['leaves "a new line of code" alone', 'I need a new line of code', 'I need a new line of code'],
    [
      'leaves "the new paragraph" alone',
      'the new paragraph is better',
      'the new paragraph is better',
    ],
    ['leaves the plural alone', 'add new lines here', 'add new lines here'],
    [
      'leaves "new line character" alone',
      'insert new line characters',
      'insert new line characters',
    ],
    ['handles the empty string', '', ''],
  ])('%s', (_name, input, expected) => {
    expect(applySpokenFormatting(input)).toBe(expected);
  });
});

describe('applySpokenPunctuation', () => {
  describe('converts clear commands', () => {
    it.each([
      ['comma and question mark', 'hello comma how are you question mark', 'hello, how are you?'],
      ['exclamation mark', 'stop exclamation mark go', 'stop! Go'],
      ['exclamation point', 'wow exclamation point', 'wow!'],
      ['full stop', 'yes full stop no', 'yes. No'],
      ['colon', 'note colon buy milk', 'note: buy milk'],
      ['semicolon', 'we came semicolon we saw', 'we came; we saw'],
      ['hyphenated semi-colon', 'we came semi-colon we saw', 'we came; we saw'],
      ['hyphen joins words', 'well hyphen known', 'well-known'],
      ['dash', 'wait dash what', 'wait - what'],
      ['em dash', 'wait em dash what', 'wait — what'],
      ['ellipsis', 'so dot dot dot yeah', 'so... yeah'],
      ['quotes', 'he said open quote hello close quote', 'he said "hello"'],
      ['parentheses', 'see open paren below close paren now', 'see (below) now'],
      ['parenthesis spelled out', 'see open parenthesis below close parenthesis', 'see (below)'],
      ['period at the end', 'I went to the store period', 'I went to the store.'],
      [
        'period before a capitalised word',
        'that is all period Then we left',
        'that is all. Then we left',
      ],
    ])('%s', (_name, input, expected) => {
      expect(applySpokenPunctuation(input)).toBe(expected);
    });
  });

  describe('merges with punctuation the model already wrote', () => {
    it.each([
      ['a period the model also punctuated', 'I went to the store period.', 'I went to the store.'],
      ['a command transcribed as its own sentence', "I'm done. Period.", "I'm done."],
      ['commas around a comma', 'Hello, comma, how are you', 'Hello, how are you'],
      ['a doubled question mark', 'Is it done? Question mark.', 'Is it done?'],
      [
        'commas around quote commands',
        'He said, open quote, hello, close quote.',
        'He said, "hello".',
      ],
      [
        'a sentence that ends inside the quote',
        'she said open quote go. Close quote',
        'she said "go."',
      ],
    ])('%s', (_name, input, expected) => {
      expect(applySpokenPunctuation(input)).toBe(expected);
    });
  });

  describe('leaves ordinary uses of the words alone', () => {
    it.each([
      ['the Jurassic period was long'],
      ['It was the Jurassic period.'],
      ['It was the Edo period.'],
      ['a trial period'],
      ['The trial period ended.'],
      ['over a long period of time'],
      ['in that period we grew'],
      ['use a comma here'],
      ['the Oxford comma'],
      ['comma separated values'],
      ['comma-separated values'],
      ['colon cancer is serious'],
      ['a dash of salt'],
      ['I must dash'],
      ['a mad dash to the door'],
      ['it came to a full stop'],
      ['the question mark key'],
      ['add an exclamation mark'],
      ['put a hyphen between them'],
      ['periodic commas and colonies'],
    ])('%s', (input) => {
      expect(applySpokenPunctuation(input)).toBe(input);
    });
  });

  it('handles the empty string', () => {
    expect(applySpokenPunctuation('')).toBe('');
  });
});
