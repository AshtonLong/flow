import { describe, expect, it } from 'vitest';
import { ConfigSchema } from '@shared/config';
import { dictionaryHints } from '../../../src/main/cleanup';
import { applyDictionary } from '../../../src/main/cleanup/dictionary';

const dictionary = [
  { heard: ['tory', 'towery'], write: 'Tauri' },
  { heard: ['tory app'], write: 'Tauri App' },
  { heard: ['post gress', 'postgres'], write: 'PostgreSQL' },
  { heard: ['don'], write: 'Don' },
  { heard: ['c plus plus'], write: 'C++' },
  { heard: ['élan vital'], write: 'Élan Vital' },
  { heard: ['cash money'], write: '$& Co' },
];

describe('applyDictionary', () => {
  it.each([
    ['replaces a heard word', 'I use tory daily', 'I use Tauri daily'],
    ['ignores case and keeps punctuation', 'Tory, towery and TORY.', 'Tauri, Tauri and Tauri.'],
    ['does not replace inside larger words', 'history story toryism', 'history story toryism'],
    ['does not treat accented letters as boundaries', 'étory tory', 'étory Tauri'],
    ['keeps a possessive', "tory's API", "Tauri's API"],
    ['leaves other contractions alone', "I don't know don", "I don't know Don"],
    ['prefers the longest phrase', 'the tory app', 'the Tauri App'],
    ['tolerates a hyphen in a phrase', 'Post-gress is nice', 'PostgreSQL is nice'],
    ['tolerates a comma in a phrase', 'post, gress is nice', 'PostgreSQL is nice'],
    ['tolerates extra spaces in a phrase', 'c  plus   plus rocks', 'C++ rocks'],
    ['matches Unicode phrases', 'the Élan vital idea', 'the Élan Vital idea'],
    ['inserts the replacement literally', 'cash money', '$& Co'],
    ['is idempotent on the written form', 'Tauri is Tauri', 'Tauri is Tauri'],
    ['handles the empty string', '', ''],
  ])('%s', (_name, input, expected) => {
    expect(applyDictionary(input, dictionary)).toBe(expected);
  });

  it('returns the text unchanged with an empty dictionary', () => {
    expect(applyDictionary('tory', [])).toBe('tory');
  });

  it('does not replace a replacement again', () => {
    const chained = [
      { heard: ['alpha'], write: 'beta' },
      { heard: ['beta'], write: 'gamma' },
    ];
    expect(applyDictionary('alpha beta', chained)).toBe('beta gamma');
  });

  it('lets the first entry win a heard phrase claimed twice', () => {
    const twice = [
      { heard: ['kube'], write: 'Kubernetes' },
      { heard: ['Kube'], write: 'cube' },
    ];
    expect(applyDictionary('kube', twice)).toBe('Kubernetes');
  });

  it('treats regex characters in a heard phrase literally', () => {
    expect(applyDictionary('use a.b here, not axb', [{ heard: ['a.b'], write: 'AB' }])).toBe(
      'use AB here, not axb',
    );
  });
});

describe('dictionaryHints', () => {
  it('returns the write terms once each, in order', () => {
    const config = ConfigSchema.parse({
      dictionary: [
        { heard: ['tory'], write: 'Tauri' },
        { heard: ['towery'], write: 'Tauri' },
        { heard: ['cube control'], write: ' kubectl ' },
      ],
    });
    expect(dictionaryHints(config)).toEqual(['Tauri', 'kubectl']);
  });

  it('is empty with no dictionary', () => {
    expect(dictionaryHints(ConfigSchema.parse({}))).toEqual([]);
  });
});
