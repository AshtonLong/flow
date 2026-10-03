import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import Database from 'better-sqlite3';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { HistoryEntry } from '@shared/types';
import { ftsQuery, HistoryStore } from '../../../src/main/history/store';

const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.UTC(2026, 9, 3, 12);

function entry(text: string, overrides: Partial<Omit<HistoryEntry, 'id'>> = {}) {
  return {
    createdAt: NOW,
    text,
    rawText: text.toLowerCase(),
    model: 'parakeet-tdt-0.6b-v2',
    app: 'notepad.exe',
    audioMs: 4200,
    elapsedMs: 380,
    ...overrides,
  };
}

const texts = (entries: HistoryEntry[]) => entries.map((item) => item.text);

describe('ftsQuery', () => {
  it('makes every word a quoted prefix term', () => {
    expect(ftsQuery('hello wor')).toBe('"hello"* "wor"*');
    expect(ftsQuery("  don't   stop ")).toBe('"don"* "t"* "stop"*');
    expect(ftsQuery('café 2026')).toBe('"café"* "2026"*');
  });

  it('neutralises FTS syntax', () => {
    expect(ftsQuery('foo AND "bar" OR NEAR(baz) -qux col:x*')).toBe(
      '"foo"* "AND"* "bar"* "OR"* "NEAR"* "baz"* "qux"* "col"* "x"*',
    );
  });

  it('returns null when there is nothing to search for', () => {
    expect(ftsQuery('')).toBeNull();
    expect(ftsQuery('"*()-:^')).toBeNull();
  });
});

for (const mode of ['fts', 'like'] as const) {
  describe(`HistoryStore (${mode === 'fts' ? 'FTS5' : 'LIKE fallback'})`, () => {
    let store: HistoryStore;

    beforeEach(() => {
      store = new HistoryStore(':memory:', { fts: mode === 'fts' });
    });

    afterEach(() => {
      store.close();
    });

    it('adds an entry and returns it with an id', () => {
      const added = store.add(entry('Hello world.'));
      expect(added).toEqual({ id: 1, ...entry('Hello world.') });
      expect(store.get(added.id)).toEqual(added);
      expect(store.add(entry('Second.')).id).toBe(2);
      expect(store.get(999)).toBeUndefined();
    });

    it('lists everything, newest first, for an empty query', () => {
      store.add(entry('oldest', { createdAt: NOW - 2000 }));
      store.add(entry('newest', { createdAt: NOW }));
      store.add(entry('middle', { createdAt: NOW - 1000 }));
      expect(texts(store.search('', 10, 0))).toEqual(['newest', 'middle', 'oldest']);
      expect(texts(store.search('   ', 10, 0))).toEqual(['newest', 'middle', 'oldest']);
    });

    it('breaks ties on the same timestamp by insertion order', () => {
      store.add(entry('first'));
      store.add(entry('second'));
      expect(texts(store.search('', 10, 0))).toEqual(['second', 'first']);
    });

    it('pages with limit and offset', () => {
      for (let i = 0; i < 5; i++) store.add(entry(`note ${i}`, { createdAt: NOW + i }));
      expect(texts(store.search('', 2, 0))).toEqual(['note 4', 'note 3']);
      expect(texts(store.search('', 2, 2))).toEqual(['note 2', 'note 1']);
      expect(texts(store.search('note', 2, 4))).toEqual(['note 0']);
      expect(store.search('', 0, 0)).toEqual([]);
      expect(store.search('', -5, -5)).toEqual([]);
      expect(texts(store.search('', 1.9, 0))).toEqual(['note 4']);
    });

    it('finds text case-insensitively, newest first', () => {
      store.add(entry('Send the quarterly report to Maria.', { createdAt: NOW - 3 }));
      store.add(entry('Lunch at noon?', { createdAt: NOW - 2 }));
      store.add(entry('The REPORT is late.', { createdAt: NOW - 1 }));
      expect(texts(store.search('report', 10, 0))).toEqual([
        'The REPORT is late.',
        'Send the quarterly report to Maria.',
      ]);
      expect(store.search('nothing-like-this', 10, 0)).toEqual([]);
    });

    it('matches the start of a word', () => {
      store.add(entry('Send the quarterly report to Maria.'));
      expect(store.search('quart', 10, 0)).toHaveLength(1);
      expect(store.search('repor', 10, 0)).toHaveLength(1);
    });

    it('does not choke on quotes, operators and wildcards', () => {
      store.add(entry('Use the "quoted" word and 100% of it.'));
      store.add(entry('snake_case and C:\\path'));
      for (const query of ['"', '"quoted', 'AND', 'OR NOT', '(', 'a*', 'x:y', "it's", '^', '-']) {
        expect(() => store.search(query, 10, 0)).not.toThrow();
      }
      expect(store.search('"quoted"', 10, 0)).toHaveLength(1);
      // Punctuation-only queries match literally.
      expect(texts(store.search('%', 10, 0))).toEqual(['Use the "quoted" word and 100% of it.']);
      expect(texts(store.search('_', 10, 0))).toEqual(['snake_case and C:\\path']);
      expect(texts(store.search('\\', 10, 0))).toEqual(['snake_case and C:\\path']);
    });

    it('updates the text and searches the new text', () => {
      const added = store.add(entry('Meet at the libary.'));
      const updated = store.update(added.id, { text: 'Meet at the library.' });
      expect(updated).toEqual({ ...added, text: 'Meet at the library.' });
      expect(updated!.rawText).toBe('meet at the libary.');
      expect(store.search('library', 10, 0)).toHaveLength(1);
      expect(store.search('libary', 10, 0)).toHaveLength(0);
      expect(store.update(added.id, {})).toEqual(updated);
      expect(store.update(999, { text: 'x' })).toBeUndefined();
    });

    it('deletes one entry and clears all', () => {
      const a = store.add(entry('alpha one'));
      store.add(entry('alpha two'));
      store.delete(a.id);
      store.delete(999);
      expect(texts(store.search('alpha', 10, 0))).toEqual(['alpha two']);
      expect(store.get(a.id)).toBeUndefined();
      store.clear();
      expect(store.search('', 10, 0)).toEqual([]);
      expect(store.search('alpha', 10, 0)).toEqual([]);
    });

    it('prunes entries older than the retention period', () => {
      store.add(entry('ancient', { createdAt: NOW - 31 * DAY }));
      store.add(entry('just past', { createdAt: NOW - 30 * DAY - 1 }));
      store.add(entry('on the line', { createdAt: NOW - 30 * DAY }));
      store.add(entry('fresh', { createdAt: NOW - DAY }));
      expect(store.prune(30, NOW)).toBe(2);
      expect(texts(store.search('', 10, 0))).toEqual(['fresh', 'on the line']);
      expect(store.search('ancient', 10, 0)).toEqual([]);
      expect(store.prune(30, NOW)).toBe(0);
    });

    it('prunes against the current time by default', () => {
      store.add(entry('old', { createdAt: Date.now() - 2 * DAY }));
      store.add(entry('new', { createdAt: Date.now() }));
      expect(store.prune(1)).toBe(1);
      expect(texts(store.search('', 10, 0))).toEqual(['new']);
    });

    it('stores multi-line and non-ASCII text intact', () => {
      const text = 'Línea uno\nLine two — “quoted”\n\t日本語 😀';
      const added = store.add(entry(text, { app: 'Über.exe' }));
      expect(store.get(added.id)!.text).toBe(text);
      expect(store.get(added.id)!.app).toBe('Über.exe');
    });
  });
}

describe('HistoryStore: full-text details', () => {
  it('requires every word and ignores accents', () => {
    const store = new HistoryStore(':memory:');
    store.add(entry('Send the quarterly report to Maria.'));
    store.add(entry('The report is late.'));
    store.add(entry('Meet at the café.'));
    expect(store.search('report maria', 10, 0)).toHaveLength(1);
    expect(store.search('maria report', 10, 0)).toHaveLength(1);
    expect(store.search('cafe', 10, 0)).toHaveLength(1);
    store.close();
  });
});

describe('HistoryStore: on disk', () => {
  let dir: string;
  let path: string;

  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'flow-history-'));
    path = join(dir, 'history.db');
  });

  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  it('creates the database in WAL mode and persists entries', () => {
    const first = new HistoryStore(path);
    first.add(entry('remember me'));
    first.close();

    const db = new Database(path, { readonly: true });
    expect(db.pragma('journal_mode', { simple: true })).toBe('wal');
    db.close();

    const second = new HistoryStore(path);
    expect(texts(second.search('remember', 10, 0))).toEqual(['remember me']);
    expect(second.add(entry('another')).id).toBe(2);
    second.close();
  });

  it('indexes rows written before the full-text index existed', () => {
    const plain = new HistoryStore(path, { fts: false });
    plain.add(entry('written without an index'));
    plain.close();

    const indexed = new HistoryStore(path);
    expect(indexed.search('index', 10, 0)).toHaveLength(1);
    indexed.add(entry('written with an index'));
    expect(indexed.search('written', 10, 0)).toHaveLength(2);
    indexed.close();
  });

  it('throws when the database cannot be opened', () => {
    expect(() => new HistoryStore(join(dir, 'missing', 'dir', 'history.db'))).toThrow();
  });
});
