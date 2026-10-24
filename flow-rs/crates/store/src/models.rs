//! The model manager: owns the catalog (bundled, then refreshed from a signed remote file),
//! local model files (download, verify, remove) and the status snapshot the UI renders.
//! Paths, config, secrets and the HTTP client are injected.
//! PORT: src/main/models/manager.ts (and index.ts)
//!
//! Files on disk are the Electron build's: `<models>\<file>` (+ `.part` while downloading,
//! `.corrupt` when quarantined), `<models>\installed.json`, `<user data>\catalog.json` + `.sig`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use flow_core::catalog::{
    CATALOG_PUBLIC_KEY, CATALOG_URL, CUSTOM_PREFIX, Catalog, CatalogSnapshot, CloudModelEntry, DownloadProgress,
    DownloadState, EMPTY_CATALOG, LocalModelEntry, ModelEntry, ModelStatus, RunsOn, Tier, parse_catalog,
    verify_catalog,
};
use flow_core::config::Config;
use futures_util::FutureExt as _;
use futures_util::future::{BoxFuture, Shared};
use indexmap::IndexMap;
use parking_lot::Mutex;
use reqwest::header::{CACHE_CONTROL, PRAGMA};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::downloader::{DownloadOptions, ModelError, download_file, io_error, sha256_file};
use crate::fs_util::{file_size, is_busy, mtime_ms, remove_file_force, resolve_path, with_suffix, write_file_atomic};

/// What the manager reports. Stands in for the TS `ModelManagerEvents`.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    /// A download moved (throttled to one event per 200 ms while bytes flow).
    Progress(DownloadProgress),
    /// The catalog or the set of installed models changed: re-read the snapshot.
    Changed,
}

/// Free bytes on the volume holding a directory, or `None` if unknown.
pub type FreeBytesFn = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

pub struct ModelManagerDeps {
    /// Where model files live: `%LOCALAPPDATA%\Flow\models`.
    pub models_dir: PathBuf,
    /// `%APPDATA%\Flow`; the refreshed catalog is cached here.
    pub user_data_dir: PathBuf,
    /// `resources/catalog.json`; its `.sig` sits beside it.
    pub bundled_catalog_path: PathBuf,
    pub get_config: Arc<dyn Fn() -> Arc<Config> + Send + Sync>,
    pub has_secret: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Id of the local model currently loaded in the engine, if any.
    pub loaded_model_id: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub http: reqwest::Client,
    /// Overrides for tests.
    pub catalog_url: Option<String>,
    pub catalog_public_key: Option<String>,
    /// Defaults to `GetDiskFreeSpaceExW` (bytes available to this user).
    pub free_bytes: Option<FreeBytesFn>,
}

/// A file whose SHA-256 has been checked against the manifest, keyed by file name in `installed.json`.
#[derive(Debug, Clone)]
struct VerifiedFile {
    sha256: String,
    size_bytes: f64,
    mtime_ms: f64,
}

type SharedResult = Shared<BoxFuture<'static, Result<(), ModelError>>>;

struct ActiveDownload {
    generation: u64,
    cancel: CancellationToken,
    result: SharedResult,
}

struct State {
    catalog: Arc<Catalog>,
    verified: HashMap<String, VerifiedFile>,
    /// Files that failed verification but could not be renamed out of the way.
    corrupt: HashSet<String>,
    downloads: HashMap<String, ActiveDownload>,
    /// Latest progress per model id. Kept after an error or cancel; dropped once installed.
    progress: HashMap<String, DownloadProgress>,
    background: Shared<BoxFuture<'static, ()>>,
    refreshing: Option<Shared<BoxFuture<'static, bool>>>,
    next_generation: u64,
}

const INDEX_FILE: &str = "installed.json";
const CACHED_CATALOG_FILE: &str = "catalog.json";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const UNKNOWN_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Model name sent to a custom endpoint that does not set `model`.
const DEFAULT_CUSTOM_MODEL: &str = "whisper-1";
/// Length of `secret:` in `api_key = "secret:<id>"`.
const SECRET_REF_PREFIX_LEN: usize = "secret:".len();

fn default_free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// `1.6 GB` from a gigabyte up, else whole megabytes rounded up (`412 MB`).
pub fn format_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1024f64.powi(3);
    if gb >= 1.0 {
        // `toFixed(1)` rounds halves up.
        format!("{:.1} GB", (gb * 10.0).round() / 10.0)
    } else {
        format!("{} MB", (bytes as f64 / 1024f64.powi(2)).ceil())
    }
}

/// A one-line, user-facing reason for a failed download.
pub fn describe_error(error: &ModelError) -> String {
    match error {
        ModelError::Checksum { .. } => "The downloaded file failed its checksum and was discarded. Try again.".into(),
        other => other.to_string().split('\n').next().unwrap_or_default().to_string(),
    }
}

/// A JSON number as `JSON.stringify` writes it: integers without a fraction.
fn js_number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// Roughly `String.prototype.localeCompare`: case-insensitive first, lower case first on ties.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

/// Cheap install check: the file exists with the manifest's exact size and is not known-bad.
fn file_ok(models_dir: &Path, entry: &LocalModelEntry, corrupt: &HashSet<String>) -> bool {
    if corrupt.contains(&entry.file) {
        return false;
    }
    std::fs::metadata(models_dir.join(&entry.file)).is_ok_and(|m| m.is_file() && m.len() == entry.size_bytes)
}

/// The secret a provider's key is stored under: `api_key = "secret:<id>"`, else the provider id.
fn secret_id(provider_id: &str, config: &Config) -> String {
    let reference = config.providers.get(provider_id).and_then(|p| p.api_key.as_deref());
    match reference.map(|r| r.chars().skip(SECRET_REF_PREFIX_LEN).collect::<String>()) {
        Some(id) if !id.is_empty() => id,
        _ => provider_id.to_string(),
    }
}

pub struct ModelManager {
    deps: ModelManagerDeps,
    on_event: Arc<dyn Fn(ModelEvent) + Send + Sync>,
    state: Mutex<State>,
    /// Serialises writes of `installed.json`.
    index_lock: Mutex<()>,
    me: Weak<ModelManager>,
}

impl ModelManager {
    pub fn new(deps: ModelManagerDeps, on_event: Arc<dyn Fn(ModelEvent) + Send + Sync>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            deps,
            on_event,
            state: Mutex::new(State {
                catalog: Arc::new(EMPTY_CATALOG),
                verified: HashMap::new(),
                corrupt: HashSet::new(),
                downloads: HashMap::new(),
                progress: HashMap::new(),
                background: futures_util::future::ready(()).boxed().shared(),
                refreshing: None,
                next_generation: 1,
            }),
            index_lock: Mutex::new(()),
            me: me.clone(),
        })
    }

    fn emit(&self, event: ModelEvent) {
        (self.on_event)(event);
    }

    /// Emits [`ModelEvent::Changed`], for changes the manager cannot see (a provider key
    /// was stored or deleted, so cloud models became ready or not).
    pub fn notify_changed(&self) {
        self.emit(ModelEvent::Changed);
    }

    /// Loads the bundled catalog, then the cached refreshed one if it verifies and is at least as
    /// new. Adopts model files already on disk. Never fails (the bundled catalog is the floor).
    pub async fn init(&self) {
        self.load_catalog().await;
        self.load_index().await;
        self.note_partial_downloads().await;
        self.queue_verification();
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        self.state.lock().catalog.clone()
    }

    #[cfg(test)]
    pub(crate) fn replace_catalog(&self, catalog: Catalog) {
        self.state.lock().catalog = Arc::new(catalog);
    }

    /// Fetches the remote catalog and its signature; on a valid signature and schema, caches it and
    /// emits `Changed`. Returns whether the catalog was updated. Never fails. Concurrent calls
    /// share one fetch.
    pub async fn refresh_catalog(&self) -> bool {
        let pending = {
            let mut state = self.state.lock();
            match &state.refreshing {
                Some(pending) => pending.clone(),
                None => {
                    let Some(this) = self.me.upgrade() else {
                        return false;
                    };
                    let task = tokio::spawn(async move {
                        let updated = this.refresh().await;
                        this.state.lock().refreshing = None;
                        updated
                    });
                    let pending = async move { task.await.unwrap_or(false) }.boxed().shared();
                    state.refreshing = Some(pending.clone());
                    pending
                }
            }
        };
        pending.await
    }

    /// The catalog with the runtime status of every model, for the UI.
    pub fn snapshot(&self) -> CatalogSnapshot {
        let config = (self.deps.get_config)();
        let active = config.model.active.as_str();
        let loaded = (self.deps.loaded_model_id)();
        let (catalog, progress, corrupt) = {
            let state = self.state.lock();
            (state.catalog.clone(), state.progress.clone(), state.corrupt.clone())
        };
        let mut status: IndexMap<String, ModelStatus> = IndexMap::new();

        let local_status = |id: &str, ready: bool| ModelStatus {
            id: id.to_string(),
            ready,
            active: active == id,
            loaded: loaded.as_deref() == Some(id),
            download: progress.get(id).cloned(),
            custom: false,
        };

        for entry in &catalog.local {
            let ready = file_ok(&self.deps.models_dir, entry, &corrupt);
            status.insert(entry.id.clone(), local_status(&entry.id, ready));
        }
        for entry in &catalog.cloud {
            let ready = self.provider_ready(&entry.provider, &config, &catalog);
            status.insert(
                entry.id.clone(),
                ModelStatus { id: entry.id.clone(), ready, active: active == entry.id, loaded: false, download: None, custom: false },
            );
        }

        let mut custom: Vec<ModelEntry> = Vec::new();
        for entry in self.custom_local(&config, &catalog) {
            status.insert(entry.id.clone(), ModelStatus { custom: true, ..local_status(&entry.id, entry.size_bytes > 0) });
            custom.push(ModelEntry::Local(entry));
        }
        for entry in self.custom_cloud(&config, &catalog) {
            let ready = self.provider_ready(&entry.provider, &config, &catalog);
            status.insert(
                entry.id.clone(),
                ModelStatus { id: entry.id.clone(), ready, active: active == entry.id, loaded: false, download: None, custom: true },
            );
            custom.push(ModelEntry::Cloud(entry));
        }

        let mut keys: IndexMap<String, bool> = IndexMap::new();
        let provider_ids = catalog.providers.iter().map(|p| p.id.as_str()).chain(config.providers.keys().map(String::as_str));
        for id in provider_ids {
            keys.insert(id.to_string(), (self.deps.has_secret)(&secret_id(id, &config)));
        }

        CatalogSnapshot { catalog: (*catalog).clone(), status, keys, custom }
    }

    /// Catalog entry or a user-registered `[[local_models]]` file (presented as a `LocalModelEntry`).
    pub fn local_entry(&self, id: &str) -> Option<LocalModelEntry> {
        let catalog = self.catalog();
        if let Some(entry) = catalog.local_entry(id) {
            return Some(entry.clone());
        }
        let config = (self.deps.get_config)();
        self.custom_local(&config, &catalog).into_iter().find(|e| e.id == id)
    }

    /// Catalog entry, or the custom endpoint behind `custom/<provider-id>` from `config.providers`.
    pub fn cloud_entry(&self, id: &str) -> Option<CloudModelEntry> {
        let catalog = self.catalog();
        if let Some(entry) = catalog.cloud_entry(id) {
            return Some(entry.clone());
        }
        let config = (self.deps.get_config)();
        self.custom_cloud(&config, &catalog).into_iter().find(|e| e.id == id)
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.model_path(id).is_some()
    }

    /// Absolute path of an installed local model's file, else `None`.
    pub fn model_path(&self, id: &str) -> Option<PathBuf> {
        let (catalog, corrupt) = {
            let state = self.state.lock();
            (state.catalog.clone(), state.corrupt.clone())
        };
        if let Some(entry) = catalog.local_entry(id) {
            return file_ok(&self.deps.models_dir, entry, &corrupt).then(|| self.path_of(entry));
        }
        let config = (self.deps.get_config)();
        let custom = self.custom_local(&config, &catalog).into_iter().find(|e| e.id == id)?;
        (custom.size_bytes > 0).then(|| PathBuf::from(custom.file))
    }

    /// Downloads a catalog model, resuming a partial file if there is one. One download runs per id
    /// at a time; a second call joins the first. Emits throttled `Progress`. Resolves once the file
    /// is verified and in place; fails on failure, and with [`ModelError::Cancelled`] when cancelled.
    ///
    /// The download starts at once (on a Tokio task, so this must be called inside a runtime) and
    /// runs to the end whether or not the returned future is awaited.
    pub fn download(self: &Arc<Self>, id: &str) -> BoxFuture<'static, Result<(), ModelError>> {
        let mut state = self.state.lock();
        if let Some(running) = state.downloads.get(id) {
            let result = running.result.clone();
            if !running.cancel.is_cancelled() {
                return result.boxed();
            }
            // A cancelled download is still winding down: start afresh once it has.
            drop(state);
            let this = Arc::clone(self);
            let id = id.to_string();
            let restart = tokio::spawn(async move {
                let _ = result.await;
                this.download(&id).await
            });
            return async move { restart.await.unwrap_or_else(|error| Err(ModelError::Failed(error.to_string()))) }.boxed();
        }

        let Some(entry) = state.catalog.local_entry(id).cloned() else {
            drop(state);
            let message = match self.local_entry(id) {
                Some(custom) => format!("{} is a file you registered yourself; there is nothing to download", custom.name),
                None => format!("Unknown model: {id}"),
            };
            return futures_util::future::ready(Err(ModelError::Failed(message))).boxed();
        };
        if file_ok(&self.deps.models_dir, &entry, &state.corrupt) {
            return futures_util::future::ready(Ok(())).boxed();
        }

        let cancel = CancellationToken::new();
        let generation = state.next_generation;
        state.next_generation += 1;
        let this = Arc::clone(self);
        let token = cancel.clone();
        // The lock is held until the download is registered, so its first step waits for that.
        let task = tokio::spawn(async move { this.run_download(entry, token, generation).await });
        let result: SharedResult =
            async move { task.await.unwrap_or_else(|error| Err(ModelError::Failed(error.to_string()))) }.boxed().shared();
        state.downloads.insert(id.to_string(), ActiveDownload { generation, cancel, result: result.clone() });
        result.boxed()
    }

    /// Stops a download. The partial file is kept so the next `download` resumes it.
    pub fn cancel_download(&self, id: &str) {
        if let Some(running) = self.state.lock().downloads.get(id) {
            running.cancel.cancel();
        }
    }

    /// Deletes a catalog model's file and any partial download. Fails for a user-registered file,
    /// which Flow never deletes: remove its `[[local_models]]` entry instead.
    pub async fn remove(&self, id: &str) -> Result<(), ModelError> {
        let catalog = self.catalog();
        let Some(entry) = catalog.local_entry(id) else {
            let message = match self.local_entry(id) {
                Some(custom) => format!(
                    "{} is a file you registered yourself, so Flow will not delete it. \
                     Remove its [[local_models]] entry from the config instead.",
                    custom.name
                ),
                None => format!("Unknown model: {id}"),
            };
            return Err(ModelError::Failed(message));
        };
        let running = {
            let state = self.state.lock();
            state.downloads.get(id).map(|d| (d.cancel.clone(), d.result.clone()))
        };
        if let Some((cancel, result)) = running {
            cancel.cancel();
            let _ = result.await;
        }
        let path = self.path_of(entry);
        if let Err(error) = remove_file_force(&path).await {
            if is_busy(&error) {
                return Err(ModelError::Failed(format!(
                    "{} is in use. Switch models or wait for it to unload first.",
                    entry.name
                )));
            }
            return Err(io_error(error));
        }
        remove_file_force(&with_suffix(&path, ".part")).await.map_err(io_error)?;
        remove_file_force(&with_suffix(&path, ".corrupt")).await.map_err(io_error)?;
        {
            let mut state = self.state.lock();
            state.verified.remove(&entry.file);
            state.corrupt.remove(&entry.file);
            state.progress.remove(id);
        }
        self.save_index();
        self.emit(ModelEvent::Changed);
        Ok(())
    }

    /// Ids of local models that are installed (catalog + user-registered whose file exists).
    pub fn installed_ids(&self) -> Vec<String> {
        let (catalog, corrupt) = {
            let state = self.state.lock();
            (state.catalog.clone(), state.corrupt.clone())
        };
        let config = (self.deps.get_config)();
        let mut ids: Vec<String> = catalog
            .local
            .iter()
            .filter(|e| file_ok(&self.deps.models_dir, e, &corrupt))
            .map(|e| e.id.clone())
            .collect();
        ids.extend(self.custom_local(&config, &catalog).into_iter().filter(|e| e.size_bytes > 0).map(|e| e.id));
        ids
    }

    /// Resolves when background verification of files on disk has finished.
    pub async fn when_idle(&self) {
        let background = self.state.lock().background.clone();
        background.await;
    }

    // ── Catalog ────────────────────────────────────────────────────────────

    fn public_key(&self) -> &str {
        self.deps.catalog_public_key.as_deref().unwrap_or(CATALOG_PUBLIC_KEY)
    }

    fn cached_catalog_path(&self) -> PathBuf {
        self.deps.user_data_dir.join(CACHED_CATALOG_FILE)
    }

    async fn load_catalog(&self) {
        // The bundled file is trusted as part of the signed installer, so it loads even without a
        // matching signature (a contributor's build cannot re-sign it).
        let bundled = tokio::fs::read(&self.deps.bundled_catalog_path).await.ok().and_then(|b| parse_catalog(b).ok());
        let mut catalog = bundled.unwrap_or(EMPTY_CATALOG);
        let cached_path = self.cached_catalog_path();
        let (bytes, signature) =
            tokio::join!(tokio::fs::read(&cached_path), tokio::fs::read(with_suffix(&cached_path, ".sig")));
        // No cache, or an unreadable one: the bundled catalog stands.
        if let (Ok(bytes), Ok(signature)) = (bytes, signature)
            && verify_catalog(&bytes, &String::from_utf8_lossy(&signature), self.public_key())
            && let Ok(cached) = parse_catalog(&bytes)
            // An app update can ship a newer catalog than the one cached by the old version.
            && cached.as_of >= catalog.as_of
        {
            catalog = cached;
        }
        self.state.lock().catalog = Arc::new(catalog);
    }

    async fn refresh(&self) -> bool {
        let url = self.deps.catalog_url.clone().unwrap_or_else(|| CATALOG_URL.to_string());
        let sig_url = format!("{url}.sig");
        let get = |target: String| {
            let request = self.deps.http.get(target).header(CACHE_CONTROL, "no-cache").header(PRAGMA, "no-cache");
            async move {
                let response = request.send().await.map_err(drop)?;
                if !response.status().is_success() {
                    return Err(());
                }
                response.bytes().await.map_err(drop)
            }
        };
        let both = async { tokio::try_join!(get(url), get(sig_url)) };
        let Ok(Ok((bytes, signature))) = tokio::time::timeout(CATALOG_TIMEOUT, both).await else {
            return false;
        };
        if !verify_catalog(&bytes, &String::from_utf8_lossy(&signature), self.public_key()) {
            return false;
        }
        let Ok(next) = parse_catalog(&bytes) else {
            return false;
        };
        let current = self.catalog();
        // A correctly signed but older catalog must not replace a newer one (replay).
        if next.as_of < current.as_of || next == *current {
            return false;
        }

        self.state.lock().catalog = Arc::new(next);
        let cached_path = self.cached_catalog_path();
        let written = std::fs::create_dir_all(&self.deps.user_data_dir)
            .and_then(|()| write_file_atomic(&cached_path, &bytes))
            .and_then(|()| write_file_atomic(&with_suffix(&cached_path, ".sig"), &signature));
        if let Err(error) = written {
            // Still usable for this run; the next launch refreshes again.
            log::warn!(target: "models", "could not cache the catalog: {error}");
        }
        self.emit(ModelEvent::Changed);
        self.queue_verification();
        true
    }

    // ── Entries and status ─────────────────────────────────────────────────

    fn path_of(&self, entry: &LocalModelEntry) -> PathBuf {
        self.deps.models_dir.join(&entry.file)
    }

    /// `[[local_models]]` files as entries. `file` is the absolute path; `size_bytes` is 0 if missing.
    fn custom_local(&self, config: &Config, catalog: &Catalog) -> Vec<LocalModelEntry> {
        let mut entries = Vec::new();
        let mut seen: HashSet<&str> = catalog.local.iter().map(|e| e.id.as_str()).collect();
        for model in &config.local_models {
            if !seen.insert(model.id.as_str()) {
                continue; // a catalog id, or a duplicate: the first one wins
            }
            // A bare file name means a file the user dropped into the models directory.
            let path = resolve_path(&self.deps.models_dir, &model.path);
            // Missing: listed, but not ready.
            let size_bytes = std::fs::metadata(&path).ok().filter(|m| m.is_file()).map_or(0, |m| m.len());
            let file = path.to_string_lossy().into_owned();
            let name = model.name.clone().unwrap_or_else(|| {
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            });
            entries.push(LocalModelEntry {
                id: model.id.clone(),
                name,
                family: "custom".into(),
                description: "Your own model file".into(),
                url: String::new(),
                file: file.clone(),
                sha256: UNKNOWN_SHA256.into(),
                size_bytes,
                languages: vec![config.general.language.clone()],
                licence: "User-provided".into(),
                licence_url: None,
                runs_on: RunsOn::Cpu,
                tier: Tier::Core,
                wer: None,
                gated: false,
                note: Some(file),
            });
        }
        entries
    }

    /// `[providers.<id>]` tables with a `base_url` that are not catalog providers, as `custom/<id>`.
    fn custom_cloud(&self, config: &Config, catalog: &Catalog) -> Vec<CloudModelEntry> {
        let mut entries = Vec::new();
        for (id, provider) in &config.providers {
            let Some(base_url) = provider.base_url.as_deref().filter(|url| !url.is_empty()) else {
                continue;
            };
            if catalog.provider(id).is_some() {
                continue;
            }
            entries.push(CloudModelEntry {
                id: format!("{CUSTOM_PREFIX}{id}"),
                provider: id.clone(),
                model: provider.model.clone().unwrap_or_else(|| DEFAULT_CUSTOM_MODEL.into()),
                name: id.clone(),
                description: format!("Custom endpoint at {base_url}"),
                price_per_hour: 0.0,
                recommended: false,
                hints: false,
                min_billed_seconds: None,
                docs_url: None,
            });
        }
        entries
    }

    fn provider_ready(&self, provider_id: &str, config: &Config, catalog: &Catalog) -> bool {
        let provider = catalog.provider(provider_id);
        if provider.is_some_and(|p| p.keyless) {
            return true;
        }
        // A custom endpoint with no `api_key` line is taken to need no key.
        let has_key_line =
            config.providers.get(provider_id).and_then(|p| p.api_key.as_deref()).is_some_and(|key| !key.is_empty());
        if provider.is_none() && !has_key_line {
            return true;
        }
        (self.deps.has_secret)(&secret_id(provider_id, config))
    }

    // ── Downloads ──────────────────────────────────────────────────────────

    fn finish(&self, id: &str, generation: u64) {
        let mut state = self.state.lock();
        if state.downloads.get(id).is_some_and(|d| d.generation == generation) {
            state.downloads.remove(id);
        }
    }

    async fn run_download(&self, entry: LocalModelEntry, cancel: CancellationToken, generation: u64) -> Result<(), ModelError> {
        let id = entry.id.clone();
        let total = entry.size_bytes;
        let dest_path = self.path_of(&entry);
        let part_path = with_suffix(&dest_path, ".part");
        let publisher = Publisher {
            manager: self,
            current: Mutex::new(DownloadProgress {
                id: id.clone(),
                received_bytes: 0,
                total_bytes: total,
                speed: 0.0,
                state: DownloadState::Downloading,
                error: None,
            }),
        };

        let outcome: Result<(), ModelError> = async {
            tokio::fs::create_dir_all(&self.deps.models_dir).await.map_err(io_error)?;
            let have = file_size(&part_path).await.min(total);
            publisher.publish(|p| p.received_bytes = have);

            let free = match &self.deps.free_bytes {
                Some(free_bytes) => free_bytes(&self.deps.models_dir),
                None => default_free_bytes(&self.deps.models_dir),
            };
            if let Some(free) = free
                && free < total - have
            {
                return Err(ModelError::Failed(format!(
                    "Not enough disk space: {} needed, {} free",
                    format_bytes(total - have),
                    format_bytes(free)
                )));
            }

            // Speed is measured between emitted samples, starting from the first byte of this session
            // so that bytes already on disk from an earlier attempt do not count.
            let sampler = Mutex::new(Sampler { at: None, bytes: 0, speed: 0.0 });
            let on_progress = |received: u64, _total: u64| {
                if received >= total {
                    publisher.publish(|p| {
                        p.received_bytes = total;
                        p.speed = 0.0;
                        p.state = DownloadState::Verifying;
                    });
                    return;
                }
                let now = Instant::now();
                let mut sample = sampler.lock();
                let at = match sample.at {
                    Some(at) if received >= sample.bytes => at,
                    _ => {
                        sample.at = Some(now);
                        sample.bytes = received;
                        drop(sample);
                        // Differs from the first report only if the server made us start over.
                        if received != publisher.received() {
                            publisher.publish(|p| p.received_bytes = received);
                        }
                        return;
                    }
                };
                let elapsed = now.duration_since(at);
                if elapsed < PROGRESS_INTERVAL {
                    return;
                }
                let current = (received - sample.bytes) as f64 / elapsed.as_secs_f64();
                sample.speed = if sample.speed == 0.0 { current } else { sample.speed * 0.7 + current * 0.3 };
                sample.at = Some(now);
                sample.bytes = received;
                let speed = sample.speed.round();
                drop(sample);
                publisher.publish(|p| {
                    p.received_bytes = received;
                    p.speed = speed;
                });
            };
            download_file(DownloadOptions {
                url: &entry.url,
                dest_path: &dest_path,
                sha256: &entry.sha256,
                size_bytes: total,
                http: &self.deps.http,
                cancel: Some(&cancel),
                on_progress: Some(&on_progress),
                stall_timeout: None,
            })
            .await?;

            let metadata = tokio::fs::metadata(&dest_path).await.map_err(io_error)?;
            {
                let mut state = self.state.lock();
                state.verified.insert(
                    entry.file.clone(),
                    VerifiedFile {
                        sha256: entry.sha256.to_lowercase(),
                        size_bytes: metadata.len() as f64,
                        mtime_ms: mtime_ms(&metadata),
                    },
                );
                state.corrupt.remove(&entry.file);
            }
            self.save_index();
            Ok(())
        }
        .await;

        match outcome {
            Ok(()) => {
                self.finish(&id, generation);
                self.state.lock().progress.remove(&id);
                let mut done = publisher.current.lock().clone();
                done.received_bytes = total;
                done.speed = 0.0;
                done.state = DownloadState::Done;
                self.emit(ModelEvent::Progress(done));
                self.emit(ModelEvent::Changed);
                Ok(())
            }
            Err(error) => {
                self.finish(&id, generation);
                let received = file_size(&part_path).await.min(total);
                if cancel.is_cancelled() || error.is_cancelled() {
                    publisher.publish(|p| {
                        p.received_bytes = received;
                        p.speed = 0.0;
                        p.state = DownloadState::Cancelled;
                    });
                    return Err(ModelError::Cancelled);
                }
                let line = describe_error(&error);
                log::warn!(target: "models", "download {id} failed: {line}");
                publisher.publish(|p| {
                    p.received_bytes = received;
                    p.speed = 0.0;
                    p.state = DownloadState::Error;
                    p.error = Some(line);
                });
                Err(error)
            }
        }
    }

    /// Surfaces partial files from an earlier run as resumable (`cancelled`) downloads.
    async fn note_partial_downloads(&self) {
        let (catalog, corrupt) = {
            let state = self.state.lock();
            (state.catalog.clone(), state.corrupt.clone())
        };
        for entry in &catalog.local {
            let received_bytes = file_size(&with_suffix(&self.path_of(entry), ".part")).await;
            if received_bytes == 0
                || received_bytes > entry.size_bytes
                || file_ok(&self.deps.models_dir, entry, &corrupt)
            {
                continue;
            }
            self.state.lock().progress.insert(
                entry.id.clone(),
                DownloadProgress {
                    id: entry.id.clone(),
                    received_bytes,
                    total_bytes: entry.size_bytes,
                    speed: 0.0,
                    state: DownloadState::Cancelled,
                    error: None,
                },
            );
        }
    }

    // ── Verified-file index ────────────────────────────────────────────────

    fn index_path(&self) -> PathBuf {
        self.deps.models_dir.join(INDEX_FILE)
    }

    async fn load_index(&self) {
        let mut verified = HashMap::new();
        // Missing or unreadable: every file on disk is simply verified again.
        if let Ok(text) = tokio::fs::read_to_string(self.index_path()).await
            && let Ok(parsed) = serde_json::from_str::<Value>(&text)
            && let Some(files) = parsed.get("files").and_then(Value::as_object)
        {
            for (file, record) in files {
                let sha256 = record.get("sha256").and_then(Value::as_str);
                let size_bytes = record.get("sizeBytes").and_then(Value::as_f64);
                let mtime_ms = record.get("mtimeMs").and_then(Value::as_f64);
                if let (Some(sha256), Some(size_bytes), Some(mtime_ms)) = (sha256, size_bytes, mtime_ms) {
                    verified.insert(file.clone(), VerifiedFile { sha256: sha256.to_string(), size_bytes, mtime_ms });
                }
            }
        }
        self.state.lock().verified = verified;
    }

    /// Writes `installed.json` as the TS does: `{ version: 1, files }`, two-space indented,
    /// files sorted by name. Failures are ignored: the index is a cache of work already done;
    /// losing it only costs a re-hash.
    fn save_index(&self) {
        let _writing = self.index_lock.lock();
        let mut records: Vec<(String, VerifiedFile)> =
            self.state.lock().verified.iter().map(|(file, record)| (file.clone(), record.clone())).collect();
        records.sort_by(|(a, _), (b, _)| locale_compare(a, b));
        let mut files = Map::new();
        for (file, record) in records {
            files.insert(
                file,
                json!({
                    "sha256": record.sha256,
                    "sizeBytes": js_number(record.size_bytes),
                    "mtimeMs": js_number(record.mtime_ms),
                }),
            );
        }
        let index = json!({ "version": 1, "files": files });
        let Ok(text) = serde_json::to_string_pretty(&index) else {
            return;
        };
        let written = std::fs::create_dir_all(&self.deps.models_dir)
            .and_then(|()| write_file_atomic(&self.index_path(), format!("{text}\n").as_bytes()));
        if let Err(error) = written {
            log::warn!(target: "models", "could not write {INDEX_FILE}: {error}");
        }
    }

    fn queue_verification(&self) {
        let Some(this) = self.me.upgrade() else {
            return;
        };
        let mut state = self.state.lock();
        let previous = state.background.clone();
        let task = tokio::spawn(async move {
            previous.await;
            this.verify_files().await;
        });
        state.background = async move {
            let _ = task.await;
        }
        .boxed()
        .shared();
    }

    /// Hashes every catalog file on disk that has the right size but no matching record, such as a
    /// file copied in by hand. A mismatch is renamed to `<file>.corrupt` and stops counting as
    /// installed. Size alone decides `is_installed` in the meantime: hashing 1.6 GB per query is too slow.
    async fn verify_files(&self) {
        let mut changed = false;
        let mut pruned = false;
        let catalog = self.catalog();
        for entry in &catalog.local {
            if self.state.lock().downloads.contains_key(&entry.id) {
                continue;
            }
            let path = self.path_of(entry);
            let Ok(metadata) = tokio::fs::metadata(&path).await else {
                continue;
            };
            if !metadata.is_file() || metadata.len() != entry.size_bytes {
                continue;
            }
            let expected = entry.sha256.to_lowercase();
            let size_bytes = metadata.len() as f64;
            let modified = mtime_ms(&metadata);
            let recorded = self.state.lock().verified.get(&entry.file).is_some_and(|record| {
                record.sha256 == expected && record.size_bytes == size_bytes && record.mtime_ms == modified
            });
            if recorded {
                continue;
            }

            let Ok(actual) = sha256_file(&path, None).await else {
                continue; // unreadable right now; try again on the next launch
            };
            changed = true;
            if actual == expected {
                let mut state = self.state.lock();
                state.verified.insert(entry.file.clone(), VerifiedFile { sha256: expected, size_bytes, mtime_ms: modified });
                state.corrupt.remove(&entry.file);
                continue;
            }
            self.state.lock().verified.remove(&entry.file);
            let corrupt_path = with_suffix(&path, ".corrupt");
            let quarantined = match remove_file_force(&corrupt_path).await {
                Ok(()) => tokio::fs::rename(&path, &corrupt_path).await,
                Err(error) => Err(error),
            };
            if quarantined.is_err() {
                // Locked (for example loaded in the engine): remember it instead.
                self.state.lock().corrupt.insert(entry.file.clone());
            }
            log::warn!(target: "models", "{} failed verification", entry.file);
        }

        // Forget records of files that are gone.
        let files: Vec<String> = self.state.lock().verified.keys().cloned().collect();
        for file in files {
            if file_size(&self.deps.models_dir.join(&file)).await == 0 {
                self.state.lock().verified.remove(&file);
                pruned = true;
            }
        }

        if changed || pruned {
            self.save_index();
        }
        if changed {
            self.emit(ModelEvent::Changed);
        }
    }
}

struct Sampler {
    at: Option<Instant>,
    bytes: u64,
    speed: f64,
}

/// Keeps one download's progress and publishes each change.
struct Publisher<'a> {
    manager: &'a ModelManager,
    current: Mutex<DownloadProgress>,
}

impl Publisher<'_> {
    fn publish(&self, change: impl FnOnce(&mut DownloadProgress)) {
        let progress = {
            let mut current = self.current.lock();
            change(&mut current);
            current.clone()
        };
        self.manager.state.lock().progress.insert(progress.id.clone(), progress.clone());
        self.manager.emit(ModelEvent::Progress(progress));
    }

    fn received(&self) -> u64 {
        self.current.lock().received_bytes
    }
}

#[cfg(test)]
mod tests;
