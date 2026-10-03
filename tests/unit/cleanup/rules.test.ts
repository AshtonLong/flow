import { describe, expect, it } from 'vitest';
import { type Config, ConfigSchema } from '@shared/config';
import { applyRules } from '../../../src/main/cleanup';

function config(overrides: Record<string, unknown> = {}): Config {
  return ConfigSchema.parse({
    dictionary: [{ heard: ['tory', 'towery'], write: 'Tauri' }],
    snippets: [
      { trigger: 'my address', text: '221B Baker Street, London' },
      { trigger: 'code block', text: 'if (x)  {\n    um  period\n}' },
    ],
    ...overrides,
  });
}

describe('applyRules', () => {
  it.each([
    ['empty input', '', ''],
    ['whitespace only', '  \n ', ''],
    ['only a filler', 'Um.', ''],
    ['adds a trailing space', 'Hello world.', 'Hello world. '],
    ['trims what the model padded', '  Hello world.  ', 'Hello world. '],
    ['collapses spaces and fixes punctuation', 'hello  world , ok', 'hello world, ok '],
    ['applies the dictionary', 'I use tory.', 'I use Tauri. '],
    ['expands a lone snippet', 'My address.', '221B Baker Street, London '],
    [
      'runs every rule in order',
      ' Um, I use towery. New line. my address',
      'I use Tauri.\n221B Baker Street, London ',
    ],
    ['converts spoken punctuation', 'hello comma world question mark', 'hello, world? '],
    ['adds no space after a trailing line break', 'Hello. New line.', 'Hello.\n'],
    [
      'treats a line break as the end of a clause',
      'I went home period new line next item',
      'I went home.\nNext item ',
    ],
    ['normalises Windows line endings', 'one\r\ntwo', 'one\ntwo '],
  ])('%s', (_name, input, expected) => {
    expect(applyRules(input, config())).toBe(expected);
  });

  it('never alters the text of a snippet', () => {
    expect(applyRules('insert code block', config())).toBe('insert if (x)  {\n    um  period\n} ');
  });

  it('applies the dictionary before snippets, so a corrected trigger still fires', () => {
    const cfg = config({
      dictionary: [{ heard: ['my a dress'], write: 'my address' }],
    });
    expect(applyRules('my a dress', cfg)).toBe('221B Baker Street, London ');
  });

  describe('switches', () => {
    it('spoken_formatting off keeps "new line" as words', () => {
      const cfg = config({ cleanup: { spoken_formatting: false } });
      expect(applyRules('Hello. New line. Bye', cfg)).toBe('Hello. New line. Bye ');
    });

    it('spoken_punctuation off keeps "comma" as a word', () => {
      const cfg = config({ cleanup: { spoken_punctuation: false } });
      expect(applyRules('hello comma world', cfg)).toBe('hello comma world ');
    });

    it('spoken_punctuation off still converts "new line"', () => {
      const cfg = config({ cleanup: { spoken_punctuation: false } });
      expect(applyRules('hello new line world', cfg)).toBe('hello\nWorld ');
    });

    it('filler_removal off keeps fillers and repeats', () => {
      const cfg = config({ cleanup: { filler_removal: false } });
      expect(applyRules('Um, I I think', cfg)).toBe('Um, I I think ');
    });

    it('trailing_space off adds nothing', () => {
      const cfg = config({ insert: { trailing_space: false } });
      expect(applyRules('Hello.', cfg)).toBe('Hello.');
    });
  });

  describe('with the text before the cursor', () => {
    it.each([
      [
        'continues a sentence',
        'I went to the',
        'Store and bought milk.',
        ' store and bought milk. ',
      ],
      ['starts a new sentence', 'Done. ', 'Store and bought milk.', 'Store and bought milk. '],
      ['keeps a dictionary term capitalised', 'I like', 'Tory a lot', ' Tauri a lot '],
      ['keeps a snippet as saved', 'send it to', 'my address', ' 221B Baker Street, London '],
      ['attaches leading punctuation', 'I went', ', and then left', ', and then left '],
      ['types straight into an empty field', '', 'Hello there', 'Hello there '],
    ])('%s', (_name, precedingText, input, expected) => {
      expect(applyRules(input, config(), { precedingText })).toBe(expected);
    });

    it('leaves casing alone when match_case is off', () => {
      const cfg = config({ insert: { match_case: false } });
      expect(applyRules('Store.', cfg, { precedingText: 'the' })).toBe('Store. ');
    });

    it('leaves casing alone without preceding text', () => {
      expect(applyRules('Store and back.', config())).toBe('Store and back. ');
    });
  });

  it('strips internal marker characters from the input', () => {
    expect(applyRules('a0b c', config())).toBe('a0b c ');
  });

  it('never throws, whatever the input', () => {
    const cfg = config();
    const inputs = [
      '((((',
      '\\',
      '$1 $& $`',
      '\n\n\n',
      '...',
      ',',
      '"',
      'new line',
      'period',
      'um um um',
    ];
    for (const input of inputs) expect(() => applyRules(input, cfg)).not.toThrow();
    expect(applyRules(undefined as unknown as string, cfg)).toBe('');
  });

  it('falls back to the raw transcript if a rule fails', () => {
    const broken = { ...config(), dictionary: null } as unknown as Config;
    expect(applyRules(' hello ', broken)).toBe('hello');
  });
});
