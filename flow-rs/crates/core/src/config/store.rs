//! The config store: `config.toml` is the single source of settings. The store
//! reads, migrates and validates it, hot-reloads it when it is edited by hand,
//! and applies settings-window edits as in-place patches.
//!
//! PORT: src/main/config/store.ts

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};
use serde_json::Value as Json;

use super::migrations::{MIGRATIONS, Migration, PlainObject, diff_patches, file_version, run_migrations};
use super::patch::{TomlPatchError, patch_toml};
use super::toml_doc::create_line_locator;
use super::toml_parse::{TomlSyntaxError, parse_toml};
use super::toml_write::js_number_string;
use super::validate::CONFIG_KEYS;
use super::value::toml_to_json;
use super::{
    CONFIG_VERSION, Config, ConfigIssue, ConfigPatch, ConfigSnapshot, PathSeg, validate_config, write_file_atomic,
};

/// What a new `config.toml` contains.
pub const DEFAULT_CONFIG_TEXT: &str =
    "# Flow settings. Edit this file by hand or use the settings window: changes apply when you save.
# Every option has a default, so anything left out keeps its default value.
# API keys are never stored here. Providers hold a reference such as api_key = \"secret:groq\".

version = 1
";

const BOM: char = '\u{feff}';

#[derive(Debug, Clone, Default)]
pub struct ConfigStoreOptions {
    /// Ordered migrations; defaults to the built-in list.
    pub migrations: Option<Vec<Migration>>,
    /// The version this build understands; defaults to `CONFIG_VERSION`.
    pub version: Option<i64>,
    /// Quiet time after a file event before reloading; defaults to 150 ms.
    pub debounce: Option<Duration>,
}

/// Why `set`, `export_to` or `import_from` failed. The file is left alone.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// `config.toml` does not parse, so it cannot be patched.
    #[error("config.toml has a syntax error on line {line}. Fix it before changing settings.")]
    Syntax {
        line: u32,
        #[source]
        source: TomlSyntaxError,
    },
    /// A patch addresses something that cannot be addressed, such as a list
    /// item that does not exist.
    #[error(transparent)]
    Patch(TomlPatchError),
    /// The file to import is not valid TOML, or not a Flow config.
    #[error("{0}")]
    Import(String),
    /// A file could not be read or written.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The first line of an error message (Node errors and parse errors can run on).
fn error_message(error: &dyn std::fmt::Display) -> String {
    error.to_string().lines().next().unwrap_or_default().trim().to_string()
}

/// Matches `api_key = ...` and dotted forms such as `groq.api_key = ...`.
fn api_key_line() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r#"^[ \t]*(?:[A-Za-z0-9_"'-]+[ \t]*\.[ \t]*)*api_key[ \t]*="#).expect("valid regex")
    })
}

/// Removes every provider's `api_key` from config text, leaving the rest untouched.
pub fn strip_api_keys(text: &str) -> String {
    let patched = parse_toml(text).ok().and_then(|parsed| {
        let Some(toml::Value::Table(providers)) = parsed.get("providers") else {
            return Some(text.to_string());
        };
        let patches: Vec<ConfigPatch> = providers
            .iter()
            .filter(|(_, provider)| provider.as_table().is_some_and(|table| table.contains_key("api_key")))
            .map(|(id, _)| ConfigPatch::remove(crate::path!["providers", id.as_str(), "api_key"]))
            .collect();
        patch_toml(text, &patches).ok()
    });
    // The text does not parse, so it cannot be patched: drop the lines instead.
    patched.unwrap_or_else(|| {
        text.split('\n').filter(|line| !api_key_line().is_match(line)).collect::<Vec<_>>().join("\n")
    })
}

/// `yyyyMMdd-HHmmss` in local time, for backup names.
fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

type Listener = Arc<dyn Fn(&ConfigSnapshot) + Send + Sync>;

/// The file as last read or written. Only touched with the queue lock held.
struct FileState {
    /// The file's text as last read or written, without a byte-order mark.
    text: Option<String>,
    /// Notepad may add a byte-order mark; it is kept so our writes do not change the encoding.
    bom: bool,
    loaded: bool,
}

/// What `get()` and `issues()` read. Never held across file I/O.
struct Current {
    config: Arc<Config>,
    issues: Vec<ConfigIssue>,
}

#[derive(Default)]
struct Events {
    queue: VecDeque<ConfigSnapshot>,
    dispatching: bool,
}

struct WatchHandle {
    _watcher: notify::RecommendedWatcher,
    generation: u64,
}

/// The config store. Thread-safe: reads, patches and reloads run one at a time,
/// in the order they were asked for.
pub struct ConfigStore {
    path: PathBuf,
    migrations: Vec<Migration>,
    version: i64,
    debounce: Duration,
    /// The TS `enqueue` queue: held for the whole of each operation.
    file: Mutex<FileState>,
    current: RwLock<Current>,
    listeners: RwLock<Vec<Listener>>,
    events: Mutex<Events>,
    watcher: Mutex<Option<WatchHandle>>,
    watch_generation: AtomicU64,
}

impl std::fmt::Debug for ConfigStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigStore").field("path", &self.path).finish_non_exhaustive()
    }
}

/// Resets `dispatching` even if a listener panics.
struct DispatchGuard<'a>(&'a Mutex<Events>);

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        self.0.lock().dispatching = false;
    }
}

/// `JSON.stringify([config, issues])`, for change detection.
fn fingerprint(current: &Current) -> String {
    serde_json::to_string(&(&*current.config, &current.issues)).unwrap_or_default()
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Arc<Self> {
        Self::with_options(path, ConfigStoreOptions::default())
    }

    pub fn with_options(path: impl Into<PathBuf>, options: ConfigStoreOptions) -> Arc<Self> {
        Arc::new(Self {
            path: path.into(),
            migrations: options.migrations.unwrap_or_else(|| MIGRATIONS.to_vec()),
            version: options.version.unwrap_or(CONFIG_VERSION),
            debounce: options.debounce.unwrap_or(Duration::from_millis(150)),
            file: Mutex::new(FileState { text: None, bom: false, loaded: false }),
            current: RwLock::new(Current { config: Arc::new(Config::default()), issues: Vec::new() }),
            listeners: RwLock::new(Vec::new()),
            events: Mutex::new(Events::default()),
            watcher: Mutex::new(None),
            watch_generation: AtomicU64::new(0),
        })
    }

    /// Reads, migrates, validates. Never fails: problems end up in `issues()`.
    /// Emits `changed` when a later call finds different settings.
    pub fn load(&self) {
        {
            let mut file = self.file.lock();
            self.reload(&mut file, false);
        }
        self.dispatch();
    }

    pub fn get(&self) -> Arc<Config> {
        self.current.read().config.clone()
    }

    pub fn issues(&self) -> Vec<ConfigIssue> {
        self.current.read().issues.clone()
    }

    pub fn snapshot(&self) -> ConfigSnapshot {
        let current = self.current.read();
        ConfigSnapshot {
            config: (*current.config).clone(),
            issues: current.issues.clone(),
            path: self.path.to_string_lossy().into_owned(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Calls `listener` with every `changed` snapshot. Listeners run on the
    /// thread that made the change (or the watcher's thread) and must be quick;
    /// they may call back into the store.
    pub fn on_changed(&self, listener: impl Fn(&ConfigSnapshot) + Send + Sync + 'static) {
        self.listeners.write().push(Arc::new(listener));
    }

    /// Patches the file in place, writes it atomically, reloads, emits `changed`.
    /// Fails, leaving the file alone, if it currently has a syntax error or a
    /// patch addresses a list item that does not exist.
    pub fn set(&self, patches: &[ConfigPatch]) -> Result<ConfigSnapshot, ConfigError> {
        let snapshot = {
            let mut file = self.file.lock();
            let text = self.read(&mut file)?;
            let next = patch_toml(&text, patches).map_err(|error| match error {
                TomlPatchError::Syntax(source) => ConfigError::Syntax { line: source.line, source },
                other => ConfigError::Patch(other),
            })?;
            if next != text {
                self.write(&mut file, &next)?;
            }
            self.ingest(&mut file, next);
            self.announce(&mut file)
        };
        self.dispatch();
        Ok(snapshot)
    }

    /// Starts hot-reloading the file when it changes on disk. The directory is
    /// watched rather than the file, so editors that save by replacing the file
    /// are seen too.
    pub fn watch(self: &Arc<Self>) {
        let mut slot = self.watcher.lock();
        if slot.is_some() {
            return;
        }
        let generation = self.watch_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let name = self.path.file_name().map(|name| name.to_string_lossy().to_lowercase());
        let (tx, rx) = mpsc::channel::<()>();
        let handler = move |result: notify::Result<notify::Event>| {
            // A watcher error must not take the app down; the next save retries.
            let Ok(event) = result else { return };
            // Our own reads must not count as changes.
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let ours = event
                .paths
                .iter()
                .any(|path| path.file_name().map(|file| file.to_string_lossy().to_lowercase()) == name);
            if ours {
                let _ = tx.send(());
            }
        };
        let mut watcher = match notify::recommended_watcher(handler) {
            Ok(watcher) => watcher,
            Err(error) => {
                log::warn!(target: "config", "Cannot watch config.toml: {error}");
                return;
            }
        };
        let dir = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        if let Err(error) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            log::warn!(target: "config", "Cannot watch config.toml: {error}");
            return;
        }
        *slot = Some(WatchHandle { _watcher: watcher, generation });
        drop(slot);

        let store = Arc::downgrade(self);
        let debounce = self.debounce;
        let spawned = std::thread::Builder::new().name("config-watch".into()).spawn(move || {
            while rx.recv().is_ok() {
                // Wait for a quiet spell: editors often write a file in several steps.
                loop {
                    match rx.recv_timeout(debounce) {
                        Ok(()) => continue,
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                let Some(store) = store.upgrade() else { return };
                if !store.is_watching(generation) {
                    return;
                }
                store.reload_from_watcher();
            }
        });
        if let Err(error) = spawned {
            log::warn!(target: "config", "Cannot watch config.toml: {error}");
            self.watcher.lock().take();
        }
    }

    fn is_watching(&self, generation: u64) -> bool {
        self.watcher.lock().as_ref().is_some_and(|handle| handle.generation == generation)
    }

    fn reload_from_watcher(&self) {
        {
            let mut file = self.file.lock();
            self.reload(&mut file, true);
        }
        self.dispatch();
    }

    /// Stops watching and waits for pending work.
    pub fn close(&self) {
        let watcher = self.watcher.lock().take();
        drop(watcher);
        drop(self.file.lock());
    }

    /// Writes the current file to `target` with every provider `api_key` line removed.
    pub fn export_to(&self, target: &Path) -> Result<(), ConfigError> {
        let mut file = self.file.lock();
        let text = self.read(&mut file)?;
        if let Some(dir) = target.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(target, strip_api_keys(&text))?;
        Ok(())
    }

    /// Replaces the config with the file at `source`, after backing up the
    /// current one to `config.toml.bak-<timestamp>`. Fails, changing nothing,
    /// if the file is not valid TOML or is not a Flow config. Invalid values are
    /// accepted and reported in the snapshot, as they are for hand edits.
    /// `api_key` references are dropped: secrets do not travel with a config.
    pub fn import_from(&self, source: &Path) -> Result<ConfigSnapshot, ConfigError> {
        let snapshot = {
            let mut file = self.file.lock();
            let bytes = std::fs::read(source)?;
            let mut incoming = String::from_utf8_lossy(&bytes).into_owned();
            if incoming.starts_with(BOM) {
                incoming.remove(0);
            }
            let raw = parse_toml(&incoming).map_err(|error| {
                ConfigError::Import(format!(
                    "Cannot import: the file is not valid TOML (line {}). {}",
                    error.line,
                    error_message(&error)
                ))
            })?;
            if !raw.is_empty() && !raw.keys().any(|key| CONFIG_KEYS.contains(&key.as_str())) {
                return Err(ConfigError::Import("Cannot import: the file does not look like a Flow config.".into()));
            }
            let next = strip_api_keys(&incoming);

            match std::fs::read(&self.path) {
                Ok(current) => std::fs::write(self.sibling(&format!(".bak-{}", timestamp())), current)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            self.write(&mut file, &next)?;
            self.ingest(&mut file, next);
            self.announce(&mut file)
        };
        self.dispatch();
        Ok(snapshot)
    }

    /// `<path><suffix>`, e.g. `config.toml.bak-v1`.
    fn sibling(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    /// Marks the store loaded and queues a `changed` event with the current snapshot.
    fn announce(&self, file: &mut FileState) -> ConfigSnapshot {
        file.loaded = true;
        let snapshot = self.snapshot();
        self.events.lock().queue.push_back(snapshot.clone());
        snapshot
    }

    /// Delivers queued `changed` events, in order, without holding any store lock.
    fn dispatch(&self) {
        loop {
            let snapshot = {
                let mut events = self.events.lock();
                if events.dispatching {
                    // Whoever is dispatching delivers this one too, after the current one.
                    return;
                }
                let Some(snapshot) = events.queue.pop_front() else { return };
                events.dispatching = true;
                snapshot
            };
            let _guard = DispatchGuard(&self.events);
            let listeners: Vec<Listener> = self.listeners.read().clone();
            for listener in &listeners {
                listener(&snapshot);
            }
        }
    }

    fn reload(&self, file: &mut FileState, from_watcher: bool) {
        let first = !file.loaded;
        let changed = match self.read(file) {
            Ok(text) => {
                // Our own writes come back through the watcher: same content, nothing to do.
                if from_watcher && file.text.as_deref() == Some(text.as_str()) {
                    return;
                }
                self.ingest(file, text)
            }
            Err(error) => {
                let issues = vec![ConfigIssue {
                    path: Vec::new(),
                    message: format!("Could not read config.toml: {}", error_message(&error)),
                    line: None,
                }];
                let mut current = self.current.write();
                let changed = serde_json::to_string(&issues).ok() != serde_json::to_string(&current.issues).ok();
                current.issues = issues;
                changed
            }
        };
        file.loaded = true;
        if changed && !first {
            let snapshot = self.snapshot();
            self.events.lock().queue.push_back(snapshot);
        }
    }

    /// Reads the file, creating it with the default header if it does not exist.
    fn read(&self, file: &mut FileState) -> io::Result<String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                file.bom = false;
                write_file_atomic(&self.path, DEFAULT_CONFIG_TEXT.as_bytes())?;
                return Ok(DEFAULT_CONFIG_TEXT.to_string());
            }
            Err(error) => return Err(error),
        };
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        file.bom = text.starts_with(BOM);
        if file.bom {
            text.remove(0);
        }
        Ok(text)
    }

    fn with_bom(file: &FileState, text: &str) -> String {
        if file.bom { format!("{BOM}{text}") } else { text.to_string() }
    }

    fn write(&self, file: &mut FileState, text: &str) -> io::Result<()> {
        write_file_atomic(&self.path, Self::with_bom(file, text).as_bytes())?;
        file.text = Some(text.to_string());
        Ok(())
    }

    /// Keeps the first backup for a version: a re-run must not overwrite the true original.
    fn backup(&self, path: &Path, file: &FileState, text: &str) -> io::Result<()> {
        use std::io::Write;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut out) => out.write_all(Self::with_bom(file, text).as_bytes()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Runs the migrations from `version` and writes the result back in place.
    /// Returns the version reached.
    fn migrate(
        &self,
        file: &mut FileState,
        text: &mut String,
        raw: &mut PlainObject,
        version: f64,
    ) -> Result<f64, String> {
        let (migrated, reached) = run_migrations(raw, version, self.version, &self.migrations)?;
        if reached != version {
            let next = patch_toml(text, &diff_patches(raw, &migrated, &[])).map_err(|error| error_message(&error))?;
            *raw = migrated;
            let backup = self.sibling(&format!(".bak-v{}", js_number_string(version)));
            self.backup(&backup, file, text).map_err(|error| error_message(&error))?;
            self.write(file, &next).map_err(|error| error_message(&error))?;
            *text = next;
            let parsed = parse_toml(text).map_err(|error| error_message(&error))?;
            *raw = as_object(toml_to_json(toml::Value::Table(parsed)));
        }
        Ok(reached)
    }

    /// Turns file text into the current config and issues. Returns whether
    /// either changed. A syntax error keeps the last good config.
    fn ingest(&self, file: &mut FileState, file_text: String) -> bool {
        let before = fingerprint(&self.current.read());
        let mut text = file_text;
        file.text = Some(text.clone());

        let parsed = match parse_toml(&text) {
            Ok(parsed) => parsed,
            Err(error) => {
                let mut current = self.current.write();
                current.issues =
                    vec![ConfigIssue { path: Vec::new(), message: error_message(&error), line: Some(error.line) }];
                return before != fingerprint(&current);
            }
        };
        let mut raw = as_object(toml_to_json(toml::Value::Table(parsed)));

        let mut issues: Vec<ConfigIssue> = Vec::new();
        let version_issue = |message: String| ConfigIssue { path: vec![PathSeg::from("version")], message, line: None };
        match file_version(&raw) {
            Some(version) if version < self.version as f64 => {
                match self.migrate(file, &mut text, &mut raw, version) {
                    Ok(reached) if reached < self.version as f64 => issues.push(version_issue(format!(
                        "No migration from config version {} to {}",
                        js_number_string(reached),
                        self.version
                    ))),
                    Ok(_) => {}
                    Err(message) => issues.push(version_issue(format!(
                        "Could not migrate config from version {}: {message}",
                        js_number_string(version)
                    ))),
                }
            }
            Some(version) if version > self.version as f64 => issues.push(version_issue(format!(
                "This file is at config version {}, newer than this version of Flow understands ({}). Settings it does not know are ignored.",
                js_number_string(version),
                self.version
            ))),
            _ => {}
        }

        let result = validate_config(&Json::Object(raw));
        let locate = create_line_locator(&text);
        issues.extend(result.issues);
        let issues = issues
            .into_iter()
            .map(|mut issue| {
                if let Some(line) = locate.locate(&issue.path) {
                    issue.line = Some(line);
                }
                issue
            })
            .collect();
        let mut current = self.current.write();
        current.config = Arc::new(result.config);
        current.issues = issues;
        before != fingerprint(&current)
    }
}

fn as_object(value: Json) -> PlainObject {
    match value {
        Json::Object(map) => map,
        _ => PlainObject::new(),
    }
}
