//! Transcript history in SQLite. History is off by default; the caller decides
//! when to add, and disables the feature if the database fails to open.
//! PORT: src/main/history/store.ts
//!
//! The schema is the Electron build's, byte for byte, so an existing `history.db`
//! opens and searches unchanged (both builds bundle SQLite with FTS5).

use std::path::Path;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use flow_core::types::{HistoryEntry, NewHistoryEntry};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension as _, Row, params};

const COLUMNS: &str = "id, created_at, text, raw_text, model, app, audio_ms, elapsed_ms";
const NEWEST_FIRST: &str = "ORDER BY created_at DESC, id DESC";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

const SCHEMA: &str = "
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
";

// An external-content index: the text lives in `history` only, and triggers
// keep the index in step with it.
const FTS_SCHEMA: &str = "
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
";

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// Used after [`HistoryStore::close`].
    #[error("The database connection is not open")]
    Closed,
}

fn to_entry(row: &Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get(0)?,
        created_at: row.get(1)?,
        text: row.get(2)?,
        raw_text: row.get(3)?,
        model: row.get(4)?,
        app: row.get(5)?,
        audio_ms: row.get(6)?,
        elapsed_ms: row.get(7)?,
    })
}

/// Turns what the user typed into an FTS5 query: every word must appear, as a
/// prefix. Words are quoted, so FTS operators and punctuation in the input are
/// inert. Returns `None` when the input has no searchable word.
pub fn fts_query(query: &str) -> Option<String> {
    static WORD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"[\p{L}\p{N}]+").expect("valid pattern"));
    let words: Vec<String> = WORD.find_iter(query).map(|word| format!("\"{}\"*", word.as_str())).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn like_pattern(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len() + 2);
    escaped.push('%');
    for c in query.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('%');
    escaped
}

fn clamp_count(value: i64) -> i64 {
    value.max(0)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Copy)]
pub struct HistoryStoreOptions {
    /// Set to `false` to search with `LIKE` only; used when FTS5 is unavailable.
    pub fts: bool,
}

impl Default for HistoryStoreOptions {
    fn default() -> Self {
        Self { fts: true }
    }
}

/// The history database. Thread-safe: calls are serialised on one connection.
pub struct HistoryStore {
    db: Mutex<Option<Connection>>,
    fts: bool,
}

impl HistoryStore {
    /// Opens or creates the database (`":memory:"` for a private in-memory one).
    /// Fails if the file cannot be opened.
    pub fn open(path: &Path) -> Result<Self, HistoryError> {
        Self::open_with(path, HistoryStoreOptions::default())
    }

    pub fn open_with(path: &Path, options: HistoryStoreOptions) -> Result<Self, HistoryError> {
        let db = Connection::open(path)?;
        // Answers with the resulting mode (`memory` for an in-memory database).
        db.query_row("PRAGMA journal_mode = WAL", [], |row| row.get::<_, String>(0))?;
        db.execute_batch(SCHEMA)?;
        let fts = options.fts && set_up_fts(&db);
        Ok(Self { db: Mutex::new(Some(db)), fts })
    }

    /// Whether searches use the full-text index (false only when FTS5 is missing or disabled).
    pub fn full_text(&self) -> bool {
        self.fts
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, HistoryError> {
        let db = self.db.lock();
        let db = db.as_ref().ok_or(HistoryError::Closed)?;
        Ok(f(db)?)
    }

    pub fn add(&self, entry: &NewHistoryEntry) -> Result<HistoryEntry, HistoryError> {
        let id = self.with(|db| {
            db.prepare_cached(
                "INSERT INTO history (created_at, text, raw_text, model, app, audio_ms, elapsed_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
            )?
            .execute(params![
                entry.created_at,
                entry.text,
                entry.raw_text,
                entry.model,
                entry.app,
                entry.audio_ms,
                entry.elapsed_ms
            ])?;
            Ok(db.last_insert_rowid())
        })?;
        Ok(self.get(id)?.unwrap_or_else(|| HistoryEntry {
            id,
            created_at: entry.created_at,
            text: entry.text.clone(),
            raw_text: entry.raw_text.clone(),
            model: entry.model.clone(),
            app: entry.app.clone(),
            audio_ms: entry.audio_ms,
            elapsed_ms: entry.elapsed_ms,
        }))
    }

    /// Newest first. An empty query returns everything. Negative counts mean 0.
    pub fn search(&self, query: &str, limit: i64, offset: i64) -> Result<Vec<HistoryEntry>, HistoryError> {
        let (limit, offset) = (clamp_count(limit), clamp_count(offset));
        let trimmed = query.trim();
        self.with(|db| {
            let collect = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<Vec<HistoryEntry>> {
                let mut statement = db.prepare_cached(sql)?;
                let rows = statement.query_map(params, to_entry)?;
                rows.collect()
            };
            if trimmed.is_empty() {
                return collect(
                    &format!("SELECT {COLUMNS} FROM history {NEWEST_FIRST} LIMIT ? OFFSET ?"),
                    &[&limit, &offset],
                );
            }
            match self.fts.then(|| fts_query(trimmed)).flatten() {
                Some(matched) => collect(
                    &format!(
                        "SELECT {COLUMNS} FROM history
         WHERE id IN (SELECT rowid FROM history_fts WHERE history_fts MATCH ?)
         {NEWEST_FIRST} LIMIT ? OFFSET ?"
                    ),
                    &[&matched, &limit, &offset],
                ),
                None => collect(
                    &format!(
                        "SELECT {COLUMNS} FROM history WHERE text LIKE ? ESCAPE '\\'
         {NEWEST_FIRST} LIMIT ? OFFSET ?"
                    ),
                    &[&like_pattern(trimmed), &limit, &offset],
                ),
            }
        })
    }

    pub fn get(&self, id: i64) -> Result<Option<HistoryEntry>, HistoryError> {
        self.with(|db| {
            db.prepare_cached(&format!("SELECT {COLUMNS} FROM history WHERE id = ?"))?
                .query_row([id], to_entry)
                .optional()
        })
    }

    /// Replaces the text (when given) and returns the entry, or `None` if there is no such id.
    pub fn update(&self, id: i64, text: Option<&str>) -> Result<Option<HistoryEntry>, HistoryError> {
        if let Some(text) = text {
            self.with(|db| db.prepare_cached("UPDATE history SET text = ? WHERE id = ?")?.execute(params![text, id]))?;
        }
        self.get(id)
    }

    pub fn delete(&self, id: i64) -> Result<(), HistoryError> {
        self.with(|db| db.prepare_cached("DELETE FROM history WHERE id = ?")?.execute([id]))?;
        Ok(())
    }

    pub fn clear(&self) -> Result<(), HistoryError> {
        self.with(|db| db.execute_batch("DELETE FROM history"))
    }

    /// Removes entries older than `retention_days`. Returns the number of rows removed.
    pub fn prune(&self, retention_days: i64) -> Result<usize, HistoryError> {
        self.prune_at(retention_days, now_ms())
    }

    /// [`prune`](Self::prune) against a given time (epoch milliseconds).
    pub fn prune_at(&self, retention_days: i64, now: i64) -> Result<usize, HistoryError> {
        let cutoff = now - retention_days * DAY_MS;
        self.with(|db| db.prepare_cached("DELETE FROM history WHERE created_at < ?")?.execute([cutoff]))
    }

    /// Closes the database; later calls fail with [`HistoryError::Closed`].
    pub fn close(&self) {
        if let Some(db) = self.db.lock().take()
            && let Err((_, error)) = db.close()
        {
            log::warn!(target: "history", "close failed: {error}");
        }
    }
}

/// Creates the full-text index. Returns false if this SQLite build has no FTS5.
fn set_up_fts(db: &Connection) -> bool {
    let attempt = || -> rusqlite::Result<()> {
        let existed = db
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'history_fts'", [], |_| Ok(()))
            .optional()?
            .is_some();
        db.execute_batch(FTS_SCHEMA)?;
        // Rows written while the index did not exist have to be indexed now.
        if !existed {
            db.execute_batch("INSERT INTO history_fts (history_fts) VALUES ('rebuild')")?;
        }
        Ok(())
    };
    match attempt() {
        Ok(()) => true,
        Err(error) => {
            log::warn!(target: "history", "full-text search unavailable: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests;
