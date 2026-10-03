import { describe, expect, it } from 'vitest';
import { applySnippets, expandSnippets, restoreSnippets } from '../../../src/main/cleanup/snippets';

const snippets = [
  { trigger: 'my address', text: '221B Baker Street, London' },
  { trigger: 'my address two', text: 'Flat 2' },
  { trigger: 'sign off', text: 'Best,\nAshton\n' },
  { trigger: 'Shrug.', text: '¯\\_(ツ)_/¯' },
];

describe('applySnippets', () => {
  it.each([
    ['expands a lone trigger', 'my address', '221B Baker Street, London'],
    ['drops punctuation around a lone trigger', 'My address.', '221B Baker Street, London'],
    ['drops quotes around a lone trigger', '"My address!"', '221B Baker Street, London'],
    ['ignores case', 'MY ADDRESS', '221B Baker Street, London'],
    [
      'expands inline and keeps the sentence punctuation',
      'Send it to my address.',
      'Send it to 221B Baker Street, London.',
    ],
    [
      'expands before a comma',
      'Send it to my address, please',
      'Send it to 221B Baker Street, London, please',
    ],
    ['respects word boundaries', 'all my addresses', 'all my addresses'],
    ['prefers the longest trigger', 'use my address two', 'use Flat 2'],
    ['outputs a multi-line snippet verbatim', 'Sign off.', 'Best,\nAshton\n'],
    ['starts the next word on the new line', 'then sign off. Bye', 'then Best,\nAshton\nBye'],
    ['ignores punctuation saved with the trigger', 'shrug', '¯\\_(ツ)_/¯'],
    ['tolerates a comma inside the trigger', 'my, address', '221B Baker Street, London'],
    ['handles the empty string', '', ''],
  ])('%s', (_name, input, expected) => {
    expect(applySnippets(input, snippets)).toBe(expected);
  });

  it('does nothing without snippets', () => {
    expect(applySnippets('my address', [])).toBe('my address');
  });
});

describe('expandSnippets', () => {
  it('hides snippet text behind slots until restored', () => {
    const expanded = expandSnippets('go to my address now', snippets);
    expect(expanded.slots).toEqual(['221B Baker Street, London']);
    expect(expanded.text).not.toContain('Baker');
    expect(restoreSnippets(expanded.text, expanded.slots)).toBe(
      'go to 221B Baker Street, London now',
    );
  });
});
