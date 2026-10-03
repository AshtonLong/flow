import { describe, expect, it } from 'vitest';
import { fitToContext } from '../../../src/main/cleanup/spacing';
import { tidy } from '../../../src/main/cleanup/text';

describe('tidy', () => {
  it.each([
    ['collapses runs of spaces', 'hello   world', 'hello world'],
    ['removes space before punctuation', 'hello , world .', 'hello, world.'],
    ['trims both ends', '  hello  ', 'hello'],
    ['strips spaces around line breaks', 'one \n two', 'one\ntwo'],
    ['keeps line breaks at the ends', '\nhello\n\n', '\nhello\n\n'],
    ['keeps a leading dot on a word', 'use .NET here', 'use .NET here'],
    ['keeps an emoticon', 'nice :)', 'nice :)'],
    ['handles the empty string', '', ''],
  ])('%s', (_name, input, expected) => {
    expect(tidy(input)).toBe(expected);
  });
});

describe('fitToContext', () => {
  const base = { trailingSpace: false, matchCase: true };

  describe('trailing space', () => {
    it.each([
      ['adds one space', 'Hello.', 'Hello. '],
      ['not after a trailing line break', 'Hello.\n', 'Hello.\n'],
      ['not a second one', 'Hello. ', 'Hello. '],
      ['not to nothing', '', ''],
    ])('%s', (_name, input, expected) => {
      expect(fitToContext(input, { trailingSpace: true, matchCase: false })).toBe(expected);
    });

    it('is off when trailing_space is off', () => {
      expect(fitToContext('Hello.', { trailingSpace: false, matchCase: false })).toBe('Hello.');
    });
  });

  describe('match case', () => {
    it.each([
      ['lower-cases mid-sentence', 'I went to the', 'Store and back.', ' store and back.'],
      ['lower-cases after a comma', 'Well, ', 'That works', 'that works'],
      ['lower-cases after a colon', 'Note:', 'Buy milk', ' buy milk'],
      ['keeps the capital after a full stop', 'Done. ', 'Next one', 'Next one'],
      ['keeps the capital after a question mark', 'Really?', 'Yes', ' Yes'],
      ['keeps the capital after a closing quote', 'He said "go." ', 'Then left', 'Then left'],
      ['keeps the capital on a new line', 'Title\n', 'Body text', 'Body text'],
      ['keeps the capital in an empty field', '', 'Hello there', 'Hello there'],
      ['keeps the capital after only whitespace', '   ', 'Hello there', 'Hello there'],
      ['keeps "I"', 'and then', 'I left', ' I left'],
      ["keeps I'm", 'and then', "I'm leaving", " I'm leaving"],
      ['lower-cases a word that only starts with I', 'and then', 'It broke', ' it broke'],
      ['keeps an acronym', 'use the', 'API docs', ' API docs'],
      ['keeps inner capitals', 'push to', 'GitHub now', ' GitHub now'],
      ['keeps a dictionary term', 'I like', 'Tauri a lot', ' Tauri a lot'],
      [
        'keeps a multi-word dictionary term',
        'ask',
        'Baker Street about it',
        ' Baker Street about it',
      ],
      ['does not treat a prefix as the term', 'ask', 'Tauriel about it', ' tauriel about it'],
      ['lower-cases inside an opening quote', 'he said', '"Hello there"', ' "hello there"'],
    ])('%s', (_name, precedingText, input, expected) => {
      expect(
        fitToContext(input, { ...base, precedingText, keepCase: ['Tauri', 'Baker Street'] }),
      ).toBe(expected);
    });

    it('leaves casing alone when the preceding text is unknown', () => {
      expect(fitToContext('Store and back.', base)).toBe('Store and back.');
    });

    it('leaves casing alone when match_case is off', () => {
      expect(
        fitToContext('Store.', { trailingSpace: false, matchCase: false, precedingText: 'the' }),
      ).toBe('Store.');
    });

    it('never re-cases a snippet', () => {
      expect(
        fitToContext('Baker Street', { ...base, precedingText: 'go to', startsWithSnippet: true }),
      ).toBe(' Baker Street');
    });
  });

  describe('leading space', () => {
    it.each([
      ['after a word', 'hello', 'world', ' world'],
      ['not after a space', 'hello ', 'world', 'world'],
      ['not after a line break', 'hello\n', 'World', 'World'],
      ['not after an opening bracket', 'call (', 'now', 'now'],
      ['not after an opening curly quote', 'say “', 'now', 'now'],
      ['not after an opening straight quote', 'say "', 'now', 'now'],
      ['after a closing straight quote', 'he said "hi"', 'and left', ' and left'],
      ['not before attaching punctuation', 'hello', ', world', ', world'],
      ['not before a line break', 'hello', '\nWorld', '\nWorld'],
      ['not in an empty field', '', 'World', 'World'],
      ['after sentence punctuation', 'Done.', 'Next', ' Next'],
    ])('%s', (_name, precedingText, input, expected) => {
      expect(fitToContext(input, { ...base, precedingText })).toBe(expected);
    });
  });
});
