//! PORT: tests/unit/history/store.test.ts

use super::*;
use rusqlite::OpenFlags;

const DAY: i64 = 24 * 60 * 60 * 1000;
/// `Date.UTC(2026, 9, 3, 12)`: 2026-10-03T12:00:00Z.
const NOW: i64 = 1_791_028_800_000;

fn entry(text: &str) -> NewHistoryEntry {
    NewHistoryEntry {
        created_at: NOW,
        text: text.into(),
        raw_text: text.to_lowercase(),
        model: "parakeet-tdt-0.6b-v2".into(),
        app: "notepad.exe".into(),
        audio_ms: 4200,
        elapsed_ms: 380,
    }
}

fn entry_at(text: &str, created_at: i64) -> NewHistoryEntry {
    NewHistoryEntry { created_at, ..entry(text) }
}

fn with_id(id: i64, entry: NewHistoryEntry) -> HistoryEntry {
    HistoryEntry {
        id,
        created_at: entry.created_at,
        text: entry.text,
        raw_text: entry.raw_text,
        model: entry.model,
        app: entry.app,
        audio_ms: entry.audio_ms,
        elapsed_ms: entry.elapsed_ms,
    }
}

fn texts(entries: Vec<HistoryEntry>) -> Vec<String> {
    entries.into_iter().map(|item| item.text).collect()
}

fn memory(fts: bool) -> HistoryStore {
    HistoryStore::open_with(Path::new(":memory:"), HistoryStoreOptions { fts }).expect("open")
}

// ── fts_query ───────────────────────────────────────────────────────────────

#[test]
fn fts_query_makes_every_word_a_quoted_prefix_term() {
    assert_eq!(fts_query("hello wor").as_deref(), Some(r#""hello"* "wor"*"#));
    assert_eq!(fts_query("  don't   stop ").as_deref(), Some(r#""don"* "t"* "stop"*"#));
    assert_eq!(fts_query("café 2026").as_deref(), Some(r#""café"* "2026"*"#));
}

#[test]
fn fts_query_neutralises_fts_syntax() {
    assert_eq!(
        fts_query(r#"foo AND "bar" OR NEAR(baz) -qux col:x*"#).as_deref(),
        Some(r#""foo"* "AND"* "bar"* "OR"* "NEAR"* "baz"* "qux"* "col"* "x"*"#)
    );
}

#[test]
fn fts_query_returns_null_when_there_is_nothing_to_search_for() {
    assert_eq!(fts_query(""), None);
    assert_eq!(fts_query(r#""*()-:^"#), None);
}

// ── HistoryStore, in both search modes ──────────────────────────────────────

fn adds_an_entry_and_returns_it_with_an_id(fts: bool) {
    let store = memory(fts);
    let added = store.add(&entry("Hello world.")).unwrap();
    assert_eq!(added, with_id(1, entry("Hello world.")));
    assert_eq!(store.get(added.id).unwrap(), Some(added));
    assert_eq!(store.add(&entry("Second.")).unwrap().id, 2);
    assert_eq!(store.get(999).unwrap(), None);
    store.close();
}

fn lists_everything_newest_first_for_an_empty_query(fts: bool) {
    let store = memory(fts);
    store.add(&entry_at("oldest", NOW - 2000)).unwrap();
    store.add(&entry_at("newest", NOW)).unwrap();
    store.add(&entry_at("middle", NOW - 1000)).unwrap();
    assert_eq!(texts(store.search("", 10, 0).unwrap()), ["newest", "middle", "oldest"]);
    assert_eq!(texts(store.search("   ", 10, 0).unwrap()), ["newest", "middle", "oldest"]);
    store.close();
}

fn breaks_ties_on_the_same_timestamp_by_insertion_order(fts: bool) {
    let store = memory(fts);
    store.add(&entry("first")).unwrap();
    store.add(&entry("second")).unwrap();
    assert_eq!(texts(store.search("", 10, 0).unwrap()), ["second", "first"]);
    store.close();
}

fn pages_with_limit_and_offset(fts: bool) {
    let store = memory(fts);
    for i in 0..5 {
        store.add(&entry_at(&format!("note {i}"), NOW + i)).unwrap();
    }
    assert_eq!(texts(store.search("", 2, 0).unwrap()), ["note 4", "note 3"]);
    assert_eq!(texts(store.search("", 2, 2).unwrap()), ["note 2", "note 1"]);
    assert_eq!(texts(store.search("note", 2, 4).unwrap()), ["note 0"]);
    assert_eq!(store.search("", 0, 0).unwrap(), []);
    assert_eq!(store.search("", -5, -5).unwrap(), []);
    // (The TS case `limit = 1.9` has no equivalent: counts are integers here.)
    store.close();
}

fn finds_text_case_insensitively_newest_first(fts: bool) {
    let store = memory(fts);
    store.add(&entry_at("Send the quarterly report to Maria.", NOW - 3)).unwrap();
    store.add(&entry_at("Lunch at noon?", NOW - 2)).unwrap();
    store.add(&entry_at("The REPORT is late.", NOW - 1)).unwrap();
    assert_eq!(
        texts(store.search("report", 10, 0).unwrap()),
        ["The REPORT is late.", "Send the quarterly report to Maria."]
    );
    assert_eq!(store.search("nothing-like-this", 10, 0).unwrap(), []);
    store.close();
}

fn matches_the_start_of_a_word(fts: bool) {
    let store = memory(fts);
    store.add(&entry("Send the quarterly report to Maria.")).unwrap();
    assert_eq!(store.search("quart", 10, 0).unwrap().len(), 1);
    assert_eq!(store.search("repor", 10, 0).unwrap().len(), 1);
    store.close();
}

fn does_not_choke_on_quotes_operators_and_wildcards(fts: bool) {
    let store = memory(fts);
    store.add(&entry(r#"Use the "quoted" word and 100% of it."#)).unwrap();
    store.add(&entry(r"snake_case and C:\path")).unwrap();
    for query in ["\"", "\"quoted", "AND", "OR NOT", "(", "a*", "x:y", "it's", "^", "-"] {
        assert!(store.search(query, 10, 0).is_ok(), "{query}");
    }
    assert_eq!(store.search(r#""quoted""#, 10, 0).unwrap().len(), 1);
    // Punctuation-only queries match literally.
    assert_eq!(texts(store.search("%", 10, 0).unwrap()), [r#"Use the "quoted" word and 100% of it."#]);
    assert_eq!(texts(store.search("_", 10, 0).unwrap()), [r"snake_case and C:\path"]);
    assert_eq!(texts(store.search("\\", 10, 0).unwrap()), [r"snake_case and C:\path"]);
    store.close();
}

fn updates_the_text_and_searches_the_new_text(fts: bool) {
    let store = memory(fts);
    let added = store.add(&entry("Meet at the libary.")).unwrap();
    let updated = store.update(added.id, Some("Meet at the library.")).unwrap();
    assert_eq!(updated, Some(HistoryEntry { text: "Meet at the library.".into(), ..added.clone() }));
    let updated = updated.unwrap();
    assert_eq!(updated.raw_text, "meet at the libary.");
    assert_eq!(store.search("library", 10, 0).unwrap().len(), 1);
    assert_eq!(store.search("libary", 10, 0).unwrap().len(), 0);
    assert_eq!(store.update(added.id, None).unwrap(), Some(updated));
    assert_eq!(store.update(999, Some("x")).unwrap(), None);
    store.close();
}

fn deletes_one_entry_and_clears_all(fts: bool) {
    let store = memory(fts);
    let a = store.add(&entry("alpha one")).unwrap();
    store.add(&entry("alpha two")).unwrap();
    store.delete(a.id).unwrap();
    store.delete(999).unwrap();
    assert_eq!(texts(store.search("alpha", 10, 0).unwrap()), ["alpha two"]);
    assert_eq!(store.get(a.id).unwrap(), None);
    store.clear().unwrap();
    assert_eq!(store.search("", 10, 0).unwrap(), []);
    assert_eq!(store.search("alpha", 10, 0).unwrap(), []);
    store.close();
}

fn prunes_entries_older_than_the_retention_period(fts: bool) {
    let store = memory(fts);
    store.add(&entry_at("ancient", NOW - 31 * DAY)).unwrap();
    store.add(&entry_at("just past", NOW - 30 * DAY - 1)).unwrap();
    store.add(&entry_at("on the line", NOW - 30 * DAY)).unwrap();
    store.add(&entry_at("fresh", NOW - DAY)).unwrap();
    assert_eq!(store.prune_at(30, NOW).unwrap(), 2);
    assert_eq!(texts(store.search("", 10, 0).unwrap()), ["fresh", "on the line"]);
    assert_eq!(store.search("ancient", 10, 0).unwrap(), []);
    assert_eq!(store.prune_at(30, NOW).unwrap(), 0);
    store.close();
}

fn prunes_against_the_current_time_by_default(fts: bool) {
    let store = memory(fts);
    let now = now_ms();
    store.add(&entry_at("old", now - 2 * DAY)).unwrap();
    store.add(&entry_at("new", now)).unwrap();
    assert_eq!(store.prune(1).unwrap(), 1);
    assert_eq!(texts(store.search("", 10, 0).unwrap()), ["new"]);
    store.close();
}

fn stores_multi_line_and_non_ascii_text_intact(fts: bool) {
    let store = memory(fts);
    let text = "Línea uno\nLine two — “quoted”\n\t日本語 😀";
    let added = store.add(&NewHistoryEntry { app: "Über.exe".into(), ..entry(text) }).unwrap();
    let read = store.get(added.id).unwrap().unwrap();
    assert_eq!(read.text, text);
    assert_eq!(read.app, "Über.exe");
    store.close();
}

macro_rules! in_both_modes {
    ($($name:ident),* $(,)?) => {
        /// `HistoryStore (FTS5)`
        mod fts5 {
            $( #[test] fn $name() { super::$name(true) } )*
        }
        /// `HistoryStore (LIKE fallback)`
        mod like_fallback {
            $( #[test] fn $name() { super::$name(false) } )*
        }
    };
}

in_both_modes!(
    adds_an_entry_and_returns_it_with_an_id,
    lists_everything_newest_first_for_an_empty_query,
    breaks_ties_on_the_same_timestamp_by_insertion_order,
    pages_with_limit_and_offset,
    finds_text_case_insensitively_newest_first,
    matches_the_start_of_a_word,
    does_not_choke_on_quotes_operators_and_wildcards,
    updates_the_text_and_searches_the_new_text,
    deletes_one_entry_and_clears_all,
    prunes_entries_older_than_the_retention_period,
    prunes_against_the_current_time_by_default,
    stores_multi_line_and_non_ascii_text_intact,
);

// ── Full-text details ───────────────────────────────────────────────────────

#[test]
fn full_text_requires_every_word_and_ignores_accents() {
    let store = HistoryStore::open(Path::new(":memory:")).unwrap();
    store.add(&entry("Send the quarterly report to Maria.")).unwrap();
    store.add(&entry("The report is late.")).unwrap();
    store.add(&entry("Meet at the café.")).unwrap();
    assert_eq!(store.search("report maria", 10, 0).unwrap().len(), 1);
    assert_eq!(store.search("maria report", 10, 0).unwrap().len(), 1);
    assert_eq!(store.search("cafe", 10, 0).unwrap().len(), 1);
    store.close();
}

#[test]
fn the_bundled_sqlite_has_fts5() {
    let store = HistoryStore::open(Path::new(":memory:")).unwrap();
    assert!(store.full_text());
    store.close();
}

#[test]
fn calls_after_close_fail() {
    let store = memory(true);
    store.close();
    assert!(matches!(store.search("", 10, 0), Err(HistoryError::Closed)));
    store.close(); // twice is fine
}

// ── On disk ─────────────────────────────────────────────────────────────────

#[test]
fn on_disk_creates_the_database_in_wal_mode_and_persists_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let first = HistoryStore::open(&path).unwrap();
    first.add(&entry("remember me")).unwrap();
    first.close();

    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mode: String = db.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
    assert_eq!(mode, "wal");
    drop(db);

    let second = HistoryStore::open(&path).unwrap();
    assert_eq!(texts(second.search("remember", 10, 0).unwrap()), ["remember me"]);
    assert_eq!(second.add(&entry("another")).unwrap().id, 2);
    second.close();
}

#[test]
fn on_disk_indexes_rows_written_before_the_full_text_index_existed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let plain = HistoryStore::open_with(&path, HistoryStoreOptions { fts: false }).unwrap();
    plain.add(&entry("written without an index")).unwrap();
    plain.close();

    let indexed = HistoryStore::open(&path).unwrap();
    assert_eq!(indexed.search("index", 10, 0).unwrap().len(), 1);
    indexed.add(&entry("written with an index")).unwrap();
    assert_eq!(indexed.search("written", 10, 0).unwrap().len(), 2);
    indexed.close();
}

#[test]
fn on_disk_throws_when_the_database_cannot_be_opened() {
    let dir = tempfile::tempdir().unwrap();
    assert!(HistoryStore::open(&dir.path().join("missing").join("dir").join("history.db")).is_err());
}

#[test]
fn on_disk_opens_a_database_written_by_the_electron_build() {
    // What better-sqlite3 leaves behind: the same statements, run by another connection.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    {
        let db = Connection::open(&path).unwrap();
        db.query_row("PRAGMA journal_mode = WAL", [], |row| row.get::<_, String>(0)).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db.execute_batch(FTS_SCHEMA).unwrap();
        db.execute(
            "INSERT INTO history (created_at, text, raw_text, model, app, audio_ms, elapsed_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![NOW, "Quarterly numbers are in", "quarterly numbers are in", "m", "a.exe", 1, 2],
        )
        .unwrap();
    }
    let store = HistoryStore::open(&path).unwrap();
    assert!(store.full_text());
    assert_eq!(texts(store.search("quarter numb", 10, 0).unwrap()), ["Quarterly numbers are in"]);
    store.close();
}
