/**
 * Transcript history in SQLite. History is off by default; the caller decides
 * when to add, and disables the feature if the native module fails to load.
 */
import Database from 'better-sqlite3';
import type { HistoryEntry } from '@shared/types';

interface Row {
  id: number;
  created_at: number;
  text: string;
  raw_text: string;
  model: string;
  app: string;
  audio_ms: number;
  elapsed_ms: number;
}

const COLUMNS = 'id, created_at, text, raw_text, model, app, audio_ms, elapsed_ms';
const NEWEST_FIRST = 'ORDER BY created_at DESC, id DESC';
const DAY_MS = 24 * 60 * 60 * 1000;

const SCHEMA = `
  CREATE TABLE IF NOT EXISTS history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    text TEXT NOT NULL,
    raw_text TEXT NOT NULL,
    model TEXT NOT NULL,
    app TEXT NOT NULL,
    audio_ms INTEGER NOT NULL,
    elapsed_ms INTEGER NOT NULL
  );
  CREATE INDEX IF NOT EXISTS history_created_at ON history (created_at);
`;

// An external-content index: the text lives in `history` only, and triggers
// keep the index in step with it.
const FTS_SCHEMA = `
  CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
    text, content='history', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
  );
  CREATE TRIGGER IF NOT EXISTS history_ai AFTER INSERT ON history BEGIN
    INSERT INTO history_fts (rowid, text) VALUES (new.id, new.text);
  END;
  CREATE TRIGGER IF NOT EXISTS history_ad AFTER DELETE ON history BEGIN
    INSERT INTO history_fts (history_fts, rowid, text) VALUES ('delete', old.id, old.text);
  END;
  CREATE TRIGGER IF NOT EXISTS history_au AFTER UPDATE OF text ON history BEGIN
    INSERT INTO history_fts (history_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO history_fts (rowid, text) VALUES (new.id, new.text);
  END;
`;

function toEntry(row: Row): HistoryEntry {
  return {
    id: row.id,
    createdAt: row.created_at,
    text: row.text,
    rawText: row.raw_text,
    model: row.model,
    app: row.app,
    audioMs: row.audio_ms,
    elapsedMs: row.elapsed_ms,
  };
}

/**
 * Turns what the user typed into an FTS5 query: every word must appear, as a
 * prefix. Words are quoted, so FTS operators and punctuation in the input are
 * inert. Returns null when the input has no searchable word.
 */
export function ftsQuery(query: string): string | null {
  const words = query.match(/[\p{L}\p{N}]+/gu);
  if (!words) return null;
  return words.map((word) => `"${word}"*`).join(' ');
}

function likePattern(query: string): string {
  return `%${query.replace(/[\\%_]/g, (char) => `\\${char}`)}%`;
}

function clampCount(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0;
}

export interface HistoryStoreOptions {
  /** Set to `false` to search with `LIKE` only; used when FTS5 is unavailable. */
  fts?: boolean;
}

export class HistoryStore {
  private readonly db: Database.Database;
  private readonly fts: boolean;

  /** Opens or creates the database. Throws if the native module fails to load. */
  constructor(dbPath: string, options: HistoryStoreOptions = {}) {
    this.db = new Database(dbPath);
    this.db.pragma('journal_mode = WAL');
    this.db.exec(SCHEMA);
    this.fts = options.fts !== false && this.setUpFts();
  }

  add(entry: Omit<HistoryEntry, 'id'>): HistoryEntry {
    const result = this.db
      .prepare(
        `INSERT INTO history (created_at, text, raw_text, model, app, audio_ms, elapsed_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        Math.round(entry.createdAt),
        entry.text,
        entry.rawText,
        entry.model,
        entry.app,
        Math.round(entry.audioMs),
        Math.round(entry.elapsedMs),
      );
    return this.get(Number(result.lastInsertRowid))!;
  }

  /** Newest first. An empty query returns everything. */
  search(query: string, limit: number, offset: number): HistoryEntry[] {
    const page = [clampCount(limit), clampCount(offset)];
    const trimmed = query.trim();
    let rows: Row[];
    if (trimmed === '') {
      rows = this.db
        .prepare(`SELECT ${COLUMNS} FROM history ${NEWEST_FIRST} LIMIT ? OFFSET ?`)
        .all(...page) as Row[];
    } else {
      const match = this.fts ? ftsQuery(trimmed) : null;
      rows = match === null ? this.searchLike(trimmed, page) : this.searchFts(match, page);
    }
    return rows.map(toEntry);
  }

  get(id: number): HistoryEntry | undefined {
    const row = this.db.prepare(`SELECT ${COLUMNS} FROM history WHERE id = ?`).get(id) as
      Row | undefined;
    return row ? toEntry(row) : undefined;
  }

  update(id: number, fields: Partial<Pick<HistoryEntry, 'text'>>): HistoryEntry | undefined {
    if (fields.text !== undefined) {
      this.db.prepare('UPDATE history SET text = ? WHERE id = ?').run(fields.text, id);
    }
    return this.get(id);
  }

  delete(id: number): void {
    this.db.prepare('DELETE FROM history WHERE id = ?').run(id);
  }

  clear(): void {
    this.db.exec('DELETE FROM history');
  }

  /** Removes entries older than `retentionDays`. Returns the number of rows removed. */
  prune(retentionDays: number, now: number = Date.now()): number {
    const cutoff = now - retentionDays * DAY_MS;
    return this.db.prepare('DELETE FROM history WHERE created_at < ?').run(cutoff).changes;
  }

  close(): void {
    this.db.close();
  }

  /** Creates the full-text index. Returns false if this SQLite build has no FTS5. */
  private setUpFts(): boolean {
    try {
      const existed = this.db
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'history_fts'")
        .get();
      this.db.exec(FTS_SCHEMA);
      // Rows written while the index did not exist have to be indexed now.
      if (!existed) this.db.exec("INSERT INTO history_fts (history_fts) VALUES ('rebuild')");
      return true;
    } catch {
      return false;
    }
  }

  private searchFts(match: string, page: number[]): Row[] {
    return this.db
      .prepare(
        `SELECT ${COLUMNS} FROM history
         WHERE id IN (SELECT rowid FROM history_fts WHERE history_fts MATCH ?)
         ${NEWEST_FIRST} LIMIT ? OFFSET ?`,
      )
      .all(match, ...page) as Row[];
  }

  private searchLike(query: string, page: number[]): Row[] {
    return this.db
      .prepare(
        `SELECT ${COLUMNS} FROM history WHERE text LIKE ? ESCAPE '\\'
         ${NEWEST_FIRST} LIMIT ? OFFSET ?`,
      )
      .all(likePattern(query), ...page) as Row[];
  }
}
