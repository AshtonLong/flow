//! PORT: tests/unit/models/manager.test.ts

use super::*;
use crate::test_support::{
    FakeServer, ServedFile, TestKey, encode, encode_value, local_entry, offline_client, random_bytes, sha256,
    sign_bytes, test_catalog, test_key,
};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    root: tempfile::TempDir,
    models_dir: PathBuf,
    user_data_dir: PathBuf,
    bundled_catalog_path: PathBuf,
    config: Arc<Mutex<Arc<Config>>>,
    secrets: Arc<Mutex<HashSet<String>>>,
    loaded: Arc<Mutex<Option<String>>>,
    server: Arc<FakeServer>,
    key: TestKey,
    catalog_url: String,
    alpha: Vec<u8>,
    entry_a: LocalModelEntry,
    entry_b: LocalModelEntry,
    base_catalog: Catalog,
}

#[derive(Default)]
struct Overrides {
    bundled_catalog_path: Option<PathBuf>,
    http: Option<reqwest::Client>,
    free_bytes: Option<FreeBytesFn>,
}

/// Events seen since the last `collect`.
#[derive(Default)]
struct Seen {
    progress: Mutex<Vec<DownloadProgress>>,
    changed: AtomicUsize,
}

impl Seen {
    fn record(&self, event: ModelEvent) {
        match event {
            ModelEvent::Progress(progress) => self.progress.lock().push(progress),
            ModelEvent::Changed => {
                self.changed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    /// Starts counting afresh (the TS attaches its listeners at this point).
    fn collect(&self) {
        self.progress.lock().clear();
        self.changed.store(0, Ordering::SeqCst);
    }
    fn changed(&self) -> usize {
        self.changed.load(Ordering::SeqCst)
    }
    fn progress(&self) -> Vec<DownloadProgress> {
        self.progress.lock().clone()
    }
    fn last(&self) -> DownloadProgress {
        self.progress.lock().last().cloned().expect("a progress event")
    }
}

async fn fixture() -> Fixture {
    let root = tempfile::Builder::new().prefix("flow-models-").tempdir().unwrap();
    let models_dir = root.path().join("models");
    let user_data_dir = root.path().join("userData");
    let bundled_catalog_path = root.path().join("resources").join("catalog.json");
    let server = FakeServer::start().await;
    let base = server.url("models.test", "");
    let alpha = random_bytes(40_000, 1);
    let beta = random_bytes(30_000, 2);
    let entry_a = local_entry(&base, "alpha", &alpha);
    let entry_b = local_entry(&base, "beta", &beta);
    let base_catalog = test_catalog(vec![entry_a.clone(), entry_b.clone()], "2026-10-03");
    fs::create_dir_all(root.path().join("resources")).unwrap();
    fs::write(&bundled_catalog_path, encode(&base_catalog)).unwrap();
    server.serve(&entry_a.url, ServedFile::new(alpha.clone()));
    server.serve(&entry_b.url, ServedFile::new(beta));
    let catalog_url = server.url("catalog.test", "/catalog.json");
    Fixture {
        root,
        models_dir,
        user_data_dir,
        bundled_catalog_path,
        config: Arc::new(Mutex::new(Arc::new(Config::default()))),
        secrets: Arc::new(Mutex::new(HashSet::new())),
        loaded: Arc::new(Mutex::new(None)),
        server,
        key: test_key(),
        catalog_url,
        alpha,
        entry_a,
        entry_b,
        base_catalog,
    }
}

impl Fixture {
    fn deps(&self, overrides: Overrides) -> ModelManagerDeps {
        let config = self.config.clone();
        let secrets = self.secrets.clone();
        let loaded = self.loaded.clone();
        ModelManagerDeps {
            models_dir: self.models_dir.clone(),
            user_data_dir: self.user_data_dir.clone(),
            bundled_catalog_path: overrides.bundled_catalog_path.unwrap_or_else(|| self.bundled_catalog_path.clone()),
            get_config: Arc::new(move || config.lock().clone()),
            has_secret: Arc::new(move |id| secrets.lock().contains(id)),
            loaded_model_id: Arc::new(move || loaded.lock().clone()),
            http: overrides.http.unwrap_or_else(|| self.server.client()),
            catalog_url: Some(self.catalog_url.clone()),
            catalog_public_key: Some(self.key.public_pem.clone()),
            free_bytes: Some(overrides.free_bytes.unwrap_or_else(|| Arc::new(|_| Some(1_000_000_000_000)))),
        }
    }

    fn create_with(&self, overrides: Overrides) -> (Arc<ModelManager>, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let sink = seen.clone();
        let manager = ModelManager::new(self.deps(overrides), Arc::new(move |event| sink.record(event)));
        (manager, seen)
    }

    fn create(&self) -> (Arc<ModelManager>, Arc<Seen>) {
        self.create_with(Overrides::default())
    }

    async fn ready_with(&self, overrides: Overrides) -> (Arc<ModelManager>, Arc<Seen>) {
        let (manager, seen) = self.create_with(overrides);
        manager.init().await;
        manager.when_idle().await;
        (manager, seen)
    }

    async fn ready(&self) -> Arc<ModelManager> {
        self.ready_with(Overrides::default()).await.0
    }

    /// Publishes a catalog (and its signature) on the server.
    fn publish(&self, catalog: &Catalog, signature: Option<String>) -> Vec<u8> {
        self.publish_bytes(encode(catalog), signature)
    }

    fn publish_bytes(&self, bytes: Vec<u8>, signature: Option<String>) -> Vec<u8> {
        let signature = signature.unwrap_or_else(|| sign_bytes(&bytes, &self.key));
        self.server.serve(&self.catalog_url, ServedFile::new(bytes.clone()));
        self.server.serve(&format!("{}.sig", self.catalog_url), ServedFile::new(signature.into_bytes()));
        bytes
    }

    fn cache_catalog(&self, catalog: &Catalog, signature: Option<String>) {
        let bytes = encode(catalog);
        let signature = signature.unwrap_or_else(|| sign_bytes(&bytes, &self.key));
        fs::create_dir_all(&self.user_data_dir).unwrap();
        fs::write(self.user_data_dir.join("catalog.json"), &bytes).unwrap();
        fs::write(self.user_data_dir.join("catalog.json.sig"), signature).unwrap();
    }

    fn set_config(&self, value: Value) {
        *self.config.lock() = Arc::new(serde_json::from_value(value).expect("config"));
    }

    fn write_model(&self, file: &str, bytes: &[u8]) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(self.models_dir.join(file), bytes).unwrap();
    }

    fn index(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.models_dir.join("installed.json")).unwrap()).unwrap()
    }

    fn part_path(&self, entry: &LocalModelEntry) -> PathBuf {
        self.models_dir.join(format!("{}.part", entry.file))
    }
}

/// Something an `on_chunk` callback starts and the test awaits afterwards.
type Slot<T> = Arc<Mutex<Option<T>>>;

fn status(id: &str, ready: bool, active: bool, loaded: bool) -> ModelStatus {
    ModelStatus { id: id.into(), ready, active, loaded, download: None, custom: false }
}

// ── Catalog loading ─────────────────────────────────────────────────────────

#[tokio::test]
async fn loads_the_bundled_catalog() {
    let f = fixture().await;
    assert_eq!(*f.ready().await.catalog(), f.base_catalog);
}

#[tokio::test]
async fn prefers_a_cached_catalog_that_verifies_and_is_at_least_as_new() {
    let f = fixture().await;
    let newer = test_catalog(vec![f.entry_a.clone()], "2026-11-01");
    f.cache_catalog(&newer, None);
    assert_eq!(*f.ready().await.catalog(), newer);

    let same_day = test_catalog(vec![f.entry_b.clone()], &f.base_catalog.as_of);
    f.cache_catalog(&same_day, None);
    assert_eq!(*f.ready().await.catalog(), same_day);
}

#[tokio::test]
async fn ignores_a_cached_catalog_that_is_older_than_the_bundled_one() {
    let f = fixture().await;
    f.cache_catalog(&test_catalog(vec![f.entry_a.clone()], "2026-01-01"), None);
    assert_eq!(*f.ready().await.catalog(), f.base_catalog);
}

#[tokio::test]
async fn ignores_a_cached_catalog_with_a_bad_or_missing_signature() {
    let f = fixture().await;
    let newer = test_catalog(vec![f.entry_a.clone()], "2026-11-01");
    f.cache_catalog(&newer, Some(sign_bytes(&encode(&newer), &test_key())));
    assert_eq!(*f.ready().await.catalog(), f.base_catalog);

    f.cache_catalog(&newer, Some("garbage".into()));
    assert_eq!(*f.ready().await.catalog(), f.base_catalog);

    // Signed bytes, then edited on disk.
    f.cache_catalog(&newer, None);
    let evil = LocalModelEntry { url: "https://evil.test/a.gguf".into(), ..f.entry_a.clone() };
    let tampered = Catalog { local: vec![evil], ..newer };
    fs::write(f.user_data_dir.join("catalog.json"), encode(&tampered)).unwrap();
    assert_eq!(*f.ready().await.catalog(), f.base_catalog);
}

#[tokio::test]
async fn does_not_throw_when_the_bundled_catalog_is_missing_or_broken() {
    let f = fixture().await;
    fs::write(&f.bundled_catalog_path, "{ not json").unwrap();
    let manager = f.ready().await;
    assert!(manager.catalog().local.is_empty());
    assert!(manager.snapshot().status.is_empty());

    let overrides = Overrides { bundled_catalog_path: Some(f.root.path().join("nope.json")), ..Overrides::default() };
    let (missing, _) = f.ready_with(overrides).await;
    assert!(missing.catalog().cloud.is_empty());
}

// ── refresh_catalog ─────────────────────────────────────────────────────────

#[tokio::test]
async fn adopts_and_caches_a_signed_newer_catalog() {
    let f = fixture().await;
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    seen.collect();
    let newer = test_catalog(vec![f.entry_a.clone()], "2026-12-01");
    let bytes = f.publish(&newer, None);

    assert!(manager.refresh_catalog().await);
    assert_eq!(*manager.catalog(), newer);
    assert_eq!(seen.changed(), 1);
    assert_eq!(fs::read(f.user_data_dir.join("catalog.json")).unwrap(), bytes);
    assert!(f.user_data_dir.join("catalog.json.sig").exists());

    // The cache survives a restart, and an identical remote is not an update.
    assert_eq!(*f.ready().await.catalog(), newer);
    assert!(!manager.refresh_catalog().await);
    assert_eq!(seen.changed(), 1);
}

#[tokio::test]
async fn rejects_tampered_bytes_a_wrong_key_and_a_missing_signature() {
    let f = fixture().await;
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    seen.collect();
    let newer = test_catalog(vec![f.entry_a.clone()], "2026-12-01");

    let signature = sign_bytes(&encode(&newer), &f.key);
    let evil = LocalModelEntry { url: "https://evil.test/a.gguf".into(), ..f.entry_a.clone() };
    f.publish(&Catalog { local: vec![evil], ..newer.clone() }, Some(signature));
    assert!(!manager.refresh_catalog().await);

    f.publish(&newer, Some(sign_bytes(&encode(&newer), &test_key())));
    assert!(!manager.refresh_catalog().await);

    f.publish(&newer, None);
    f.server.unserve(&format!("{}.sig", f.catalog_url));
    assert!(!manager.refresh_catalog().await);

    assert_eq!(*manager.catalog(), f.base_catalog);
    assert_eq!(seen.changed(), 0);
    assert!(!f.user_data_dir.join("catalog.json").exists());
}

#[tokio::test]
async fn never_rolls_back_to_an_older_signed_catalog() {
    let f = fixture().await;
    let manager = f.ready().await;
    f.publish(&test_catalog(vec![f.entry_a.clone()], "2026-01-01"), None);
    assert!(!manager.refresh_catalog().await);
    assert_eq!(*manager.catalog(), f.base_catalog);
}

#[tokio::test]
async fn rejects_a_signed_catalog_that_fails_the_schema() {
    let f = fixture().await;
    let manager = f.ready().await;
    f.publish(&Catalog { version: 2, ..test_catalog(vec![f.entry_a.clone()], "2026-12-01") }, None);
    assert!(!manager.refresh_catalog().await);
    f.publish_bytes(encode_value(&serde_json::json!({ "asOf": "2026-12-01" })), None);
    assert!(!manager.refresh_catalog().await);
    assert_eq!(*manager.catalog(), f.base_catalog);
}

#[tokio::test]
async fn returns_false_on_http_and_network_errors() {
    let f = fixture().await;
    let manager = f.ready().await;
    f.server.serve(&f.catalog_url, ServedFile { status: Some(500), ..ServedFile::default() });
    assert!(!manager.refresh_catalog().await);

    let (offline, _) = f.ready_with(Overrides { http: Some(offline_client()), ..Overrides::default() }).await;
    assert!(!offline.refresh_catalog().await);
}

/// The published catalog verifies with the embedded key. Needs the network:
/// `cargo test -p flow-store -- --ignored`.
#[tokio::test]
#[ignore = "fetches the real catalog from GitHub"]
async fn the_published_catalog_verifies_with_the_embedded_key() {
    let f = fixture().await;
    let overrides = Overrides { http: Some(reqwest::Client::new()), ..Overrides::default() };
    let mut deps = f.deps(overrides);
    deps.catalog_url = None;
    deps.catalog_public_key = None;
    // An empty bundled catalog, so any valid remote one is an update.
    deps.bundled_catalog_path = f.root.path().join("none.json");
    let manager = ModelManager::new(deps, Arc::new(|_| {}));
    manager.init().await;
    assert!(manager.refresh_catalog().await, "the remote catalog was not adopted");
    assert!(!manager.catalog().local.is_empty());
    assert!(f.user_data_dir.join("catalog.json.sig").exists());
}

#[tokio::test]
async fn concurrent_refreshes_share_one_fetch() {
    let f = fixture().await;
    let manager = f.ready().await;
    f.publish(&test_catalog(vec![f.entry_a.clone()], "2026-12-01"), None);
    let (first, second) = tokio::join!(manager.refresh_catalog(), manager.refresh_catalog());
    assert!(first && second);
    let fetches = f.server.requests().iter().filter(|r| r.url == f.catalog_url).count();
    assert_eq!(fetches, 1);
}

// ── Files on disk ───────────────────────────────────────────────────────────

#[tokio::test]
async fn adopts_a_correct_file_that_was_copied_in_by_hand() {
    let f = fixture().await;
    f.write_model(&f.entry_a.file, &f.alpha);

    let (manager, seen) = f.create();
    manager.init().await;
    // Installed by size at once; the hash is checked in the background.
    assert!(manager.is_installed("alpha"));
    assert_eq!(manager.model_path("alpha"), Some(f.models_dir.join(&f.entry_a.file)));

    manager.when_idle().await;
    assert_eq!(seen.changed(), 1);
    assert!(manager.is_installed("alpha"));
    assert_eq!(manager.installed_ids(), ["alpha"]);
    assert_eq!(f.index()["files"][&f.entry_a.file]["sha256"], f.entry_a.sha256.as_str());
    assert!(f.server.requests().is_empty());
}

#[tokio::test]
async fn quarantines_a_file_with_the_right_size_but_the_wrong_contents() {
    let f = fixture().await;
    f.write_model(&f.entry_a.file, &random_bytes(f.alpha.len(), 55));

    let (manager, seen) = f.create();
    manager.init().await;
    manager.when_idle().await;

    assert!(!manager.is_installed("alpha"));
    assert_eq!(manager.model_path("alpha"), None);
    assert_eq!(seen.changed(), 1);
    assert!(!f.models_dir.join(&f.entry_a.file).exists());
    assert!(f.models_dir.join(format!("{}.corrupt", f.entry_a.file)).exists());
}

#[tokio::test]
async fn does_not_hash_a_file_again_once_it_is_recorded_as_verified() {
    let f = fixture().await;
    // Wrong contents, but a record saying it was verified: proof that no re-hash happens.
    f.write_model(&f.entry_a.file, &random_bytes(f.alpha.len(), 55));
    let metadata = fs::metadata(f.models_dir.join(&f.entry_a.file)).unwrap();
    // As Node writes it: `JSON.stringify` of `stats.size` and `stats.mtimeMs`.
    let index = serde_json::json!({
        "version": 1,
        "files": { f.entry_a.file.clone(): {
            "sha256": f.entry_a.sha256, "sizeBytes": metadata.len(), "mtimeMs": mtime_ms(&metadata)
        } }
    });
    fs::write(f.models_dir.join("installed.json"), index.to_string()).unwrap();

    let (manager, seen) = f.create();
    manager.init().await;
    manager.when_idle().await;
    assert!(manager.is_installed("alpha"));
    assert_eq!(seen.changed(), 0);
}

#[tokio::test]
async fn treats_a_file_of_the_wrong_size_as_not_installed() {
    let f = fixture().await;
    f.write_model(&f.entry_a.file, &f.alpha[..100]);
    let manager = f.ready().await;
    assert!(!manager.is_installed("alpha"));
    assert!(manager.installed_ids().is_empty());
}

#[tokio::test]
async fn reports_a_leftover_partial_download_as_resumable() {
    let f = fixture().await;
    f.write_model(&format!("{}.part", f.entry_a.file), &f.alpha[..10_000]);
    let manager = f.ready().await;
    let alpha = manager.snapshot().status["alpha"].clone();
    assert!(!alpha.ready);
    let download = alpha.download.expect("a download");
    assert_eq!(download.state, DownloadState::Cancelled);
    assert_eq!(download.received_bytes, 10_000);
    assert_eq!(download.total_bytes, f.alpha.len() as u64);
}

// ── download ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn downloads_verifies_and_installs_a_model() {
    let f = fixture().await;
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    seen.collect();

    manager.download("alpha").await.unwrap();

    assert!(manager.is_installed("alpha"));
    assert_eq!(sha256(&fs::read(manager.model_path("alpha").unwrap()).unwrap()), f.entry_a.sha256);
    assert_eq!(seen.changed(), 1);
    let progress = seen.progress();
    let states: Vec<DownloadState> = progress.iter().map(|p| p.state).collect();
    assert_eq!(states[0], DownloadState::Downloading);
    assert_eq!(states[states.len() - 2..], [DownloadState::Verifying, DownloadState::Done]);
    let mut distinct = states.clone();
    distinct.dedup();
    distinct.sort_by_key(|s| *s as u8);
    distinct.dedup();
    assert_eq!(distinct, [DownloadState::Downloading, DownloadState::Verifying, DownloadState::Done]);
    assert!(progress.iter().all(|p| p.id == "alpha" && p.total_bytes == f.alpha.len() as u64));
    assert_eq!(seen.last().received_bytes, f.alpha.len() as u64);
    assert_eq!(manager.snapshot().status["alpha"], status("alpha", true, false, false));

    // Recorded as verified, so the next launch does not hash it again.
    assert_eq!(f.index()["files"][&f.entry_a.file]["sha256"], f.entry_a.sha256.as_str());
    let (next, next_seen) = f.create();
    next.init().await;
    next.when_idle().await;
    assert!(next.is_installed("alpha"));
    assert_eq!(next_seen.changed(), 0);
}

#[tokio::test]
async fn throttles_progress_events() {
    let f = fixture().await;
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    seen.collect();
    manager.download("alpha").await.unwrap();
    // 40 chunks arrive: far fewer events than chunks.
    assert_eq!(f.server.requests().len(), 1);
    assert!(seen.progress().len() < 8, "{} events", seen.progress().len());
}

#[tokio::test]
async fn runs_one_download_per_id_and_shares_it_between_callers() {
    let f = fixture().await;
    let manager = f.ready().await;
    let first = manager.download("alpha");
    let second = manager.download("alpha");
    let (a, b, c) = tokio::join!(first, second, manager.download("beta"));
    a.unwrap();
    b.unwrap();
    c.unwrap();
    let mut urls: Vec<String> = f.server.requests().into_iter().map(|r| r.url).collect();
    urls.sort();
    assert_eq!(urls, [f.entry_a.url.clone(), f.entry_b.url.clone()]);
    assert_eq!(manager.installed_ids(), ["alpha", "beta"]);

    // Already installed: nothing to do.
    manager.download("alpha").await.unwrap();
    assert_eq!(f.server.requests().len(), 2);
}

#[tokio::test]
async fn reports_an_error_state_with_a_one_line_message() {
    let f = fixture().await;
    f.server.serve(&f.entry_a.url, ServedFile { status: Some(503), ..ServedFile::new(f.alpha.clone()) });
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    seen.collect();

    let error = manager.download("alpha").await.unwrap_err();
    assert!(error.to_string().contains("HTTP 503"), "{error}");
    let last = seen.last();
    assert_eq!(last.state, DownloadState::Error);
    let message = last.error.unwrap();
    assert!(message.contains("HTTP 503"), "{message}");
    assert!(!message.contains('\n'));
    let alpha = manager.snapshot().status["alpha"].clone();
    assert!(!alpha.ready);
    assert_eq!(alpha.download.map(|d| d.state), Some(DownloadState::Error));
    assert_eq!(seen.changed(), 0);

    // A retry clears the error.
    f.server.serve(&f.entry_a.url, ServedFile::new(f.alpha.clone()));
    manager.download("alpha").await.unwrap();
    assert_eq!(manager.snapshot().status["alpha"].download, None);
}

#[tokio::test]
async fn reports_a_checksum_mismatch_and_leaves_nothing_installed() {
    let f = fixture().await;
    f.server.serve(&f.entry_a.url, ServedFile::new(random_bytes(f.alpha.len(), 77)));
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    seen.collect();

    let error = manager.download("alpha").await.unwrap_err();
    assert!(error.to_string().contains("Checksum mismatch"), "{error}");
    let last = seen.last();
    assert_eq!((last.state, last.received_bytes), (DownloadState::Error, 0));
    assert!(last.error.unwrap().contains("checksum"));
    assert!(!manager.is_installed("alpha"));
    assert!(!f.part_path(&f.entry_a).exists());
}

#[tokio::test]
async fn cancels_keeps_the_partial_file_and_resumes() {
    let f = fixture().await;
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    seen.collect();
    let target = manager.clone();
    f.server.on_chunk(Some(Arc::new(move |sent| {
        if sent >= 10_000 {
            target.cancel_download("alpha");
        }
    })));

    let error = manager.download("alpha").await.unwrap_err();
    assert!(error.is_cancelled(), "{error}");
    let cancelled = seen.last();
    assert_eq!(cancelled.state, DownloadState::Cancelled);
    assert!(cancelled.received_bytes > 0);
    assert!(cancelled.received_bytes < f.alpha.len() as u64);
    assert_eq!(fs::metadata(f.part_path(&f.entry_a)).unwrap().len(), cancelled.received_bytes);
    assert!(!manager.is_installed("alpha"));
    assert_eq!(manager.snapshot().status["alpha"].download.as_ref().map(|d| d.state), Some(DownloadState::Cancelled));

    f.server.on_chunk(None);
    manager.download("alpha").await.unwrap();
    let range = format!("bytes={}-", cancelled.received_bytes);
    assert_eq!(f.server.requests().last().unwrap().range.as_deref(), Some(range.as_str()));
    assert!(manager.is_installed("alpha"));
    assert_eq!(seen.last().state, DownloadState::Done);
}

#[tokio::test]
async fn restarts_cleanly_when_download_is_called_right_after_cancel() {
    let f = fixture().await;
    let manager = f.ready().await;
    let retry: Slot<BoxFuture<'static, Result<(), ModelError>>> = Arc::default();
    let (target, slot) = (manager.clone(), retry.clone());
    f.server.on_chunk(Some(Arc::new(move |sent| {
        let mut slot = slot.lock();
        if sent >= 10_000 && slot.is_none() {
            target.cancel_download("alpha");
            *slot = Some(target.download("alpha"));
        }
    })));
    let error = manager.download("alpha").await.unwrap_err();
    assert!(error.is_cancelled());
    f.server.on_chunk(None);
    let retry = retry.lock().take().expect("the retry started");
    retry.await.unwrap();
    assert!(manager.is_installed("alpha"));
}

#[tokio::test]
async fn fails_early_when_the_disk_is_too_full() {
    let f = fixture().await;
    let full: FreeBytesFn = Arc::new(|_| Some(1000));
    let (manager, seen) = f.ready_with(Overrides { free_bytes: Some(full), ..Overrides::default() }).await;
    seen.collect();
    let error = manager.download("alpha").await.unwrap_err();
    assert_eq!(error.to_string(), "Not enough disk space: 1 MB needed, 1 MB free");
    assert_eq!(seen.last().state, DownloadState::Error);
    assert!(f.server.requests().is_empty());
}

#[tokio::test]
async fn rejects_unknown_ids_and_user_registered_files() {
    let f = fixture().await;
    f.set_config(serde_json::json!({ "local_models": [{ "id": "mine", "path": "C:\\mine.gguf" }] }));
    let manager = f.ready().await;
    let unknown = manager.download("nope").await.unwrap_err();
    assert!(unknown.to_string().contains("Unknown model"), "{unknown}");
    let mine = manager.download("mine").await.unwrap_err();
    assert!(mine.to_string().contains("nothing to download"), "{mine}");
}

#[tokio::test]
async fn only_downloads_from_the_https_url_in_the_catalog() {
    // Bypass parse_catalog's own check to prove the downloader enforces it too.
    let f = fixture().await;
    let manager = f.ready().await;
    let mut catalog = (*manager.catalog()).clone();
    catalog.local[0].url = "http://models.test/alpha.gguf".into();
    manager.replace_catalog(catalog);
    let error = manager.download("alpha").await.unwrap_err();
    assert!(error.to_string().contains("not https"), "{error}");
    assert!(f.server.requests().is_empty());
}

// ── remove ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn deletes_the_file_its_partial_download_and_its_record() {
    let f = fixture().await;
    let (manager, seen) = f.ready_with(Overrides::default()).await;
    manager.download("alpha").await.unwrap();
    let path = manager.model_path("alpha").unwrap();
    seen.collect();

    manager.remove("alpha").await.unwrap();
    assert!(!path.exists());
    assert!(!manager.is_installed("alpha"));
    assert!(manager.installed_ids().is_empty());
    assert_eq!(seen.changed(), 1);
    assert_eq!(f.index()["files"], serde_json::json!({}));
}

#[tokio::test]
async fn cancels_a_running_download_and_clears_the_partial_file() {
    let f = fixture().await;
    let manager = f.ready().await;
    let removal: Slot<tokio::task::JoinHandle<Result<(), ModelError>>> = Arc::default();
    let (target, slot) = (manager.clone(), removal.clone());
    f.server.on_chunk(Some(Arc::new(move |sent| {
        let mut slot = slot.lock();
        if sent >= 10_000 && slot.is_none() {
            let target = target.clone();
            *slot = Some(tokio::spawn(async move { target.remove("alpha").await }));
        }
    })));
    let error = manager.download("alpha").await.unwrap_err();
    assert!(error.is_cancelled(), "{error}");
    let removal = removal.lock().take().expect("the removal started");
    removal.await.unwrap().unwrap();
    assert!(!f.part_path(&f.entry_a).exists());
    assert_eq!(manager.snapshot().status["alpha"].download, None);
}

#[tokio::test]
async fn refuses_to_delete_a_user_registered_file() {
    let f = fixture().await;
    let mine = f.root.path().join("mine.gguf");
    fs::write(&mine, random_bytes(500, 1)).unwrap();
    f.set_config(serde_json::json!({ "local_models": [{ "id": "mine", "path": mine }] }));
    let manager = f.ready().await;

    let error = manager.remove("mine").await.unwrap_err();
    assert!(error.to_string().contains("registered yourself"), "{error}");
    assert!(mine.exists());
    let unknown = manager.remove("nope").await.unwrap_err();
    assert!(unknown.to_string().contains("Unknown model"), "{unknown}");
}

// ── snapshot ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reports_local_cloud_and_key_status() {
    let f = fixture().await;
    f.write_model(&f.entry_a.file, &f.alpha);
    f.set_config(serde_json::json!({
        "model": { "active": "alpha" },
        "providers": { "openai": { "api_key": "secret:work-openai" } },
    }));
    *f.secrets.lock() = HashSet::from(["groq".to_string(), "openai".to_string()]);
    *f.loaded.lock() = Some("alpha".into());
    let manager = f.ready().await;
    let snapshot = manager.snapshot();

    assert_eq!(snapshot.catalog, f.base_catalog);
    assert!(snapshot.custom.is_empty());
    let mut ids: Vec<&str> = snapshot.status.keys().map(String::as_str).collect();
    ids.sort();
    let mut expected: Vec<&str> =
        ["alpha", "beta"].into_iter().chain(f.base_catalog.cloud.iter().map(|e| e.id.as_str())).collect();
    expected.sort();
    assert_eq!(ids, expected);
    assert_eq!(snapshot.status["alpha"], status("alpha", true, true, true));
    assert_eq!(snapshot.status["beta"], status("beta", false, false, false));
    assert!(snapshot.status["groq/whisper-large-v3-turbo"].ready);
    // The key is looked up under the id the config references, not the provider id.
    assert!(!snapshot.status["openai/gpt-4o-transcribe"].ready);
    assert!(snapshot.status["ollama/whisper"].ready);
    let keys: Vec<(&str, bool)> = snapshot.keys.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_eq!(keys, [("groq", true), ("openai", false), ("ollama", false)]);

    f.secrets.lock().insert("work-openai".into());
    f.set_config(serde_json::json!({
        "model": { "active": "openai/gpt-4o-transcribe" },
        "providers": { "openai": { "api_key": "secret:work-openai" } },
    }));
    let next = manager.snapshot();
    assert_eq!(next.status["openai/gpt-4o-transcribe"], status("openai/gpt-4o-transcribe", true, true, false));
    assert!(!next.status["alpha"].active);
    assert!(next.keys["openai"]);
}

#[tokio::test]
async fn shows_a_download_in_progress() {
    let f = fixture().await;
    let manager = f.ready().await;
    let during: Slot<Option<DownloadProgress>> = Arc::default();
    let (target, slot) = (manager.clone(), during.clone());
    f.server.on_chunk(Some(Arc::new(move |_| {
        let mut slot = slot.lock();
        if slot.is_none() {
            *slot = Some(target.snapshot().status["alpha"].download.clone());
        }
    })));
    manager.download("alpha").await.unwrap();
    let during = during.lock().clone().flatten().expect("progress during the download");
    assert_eq!((during.id.as_str(), during.state), ("alpha", DownloadState::Downloading));
}

#[tokio::test]
async fn lists_user_registered_files_and_custom_endpoints() {
    let f = fixture().await;
    let mine = f.root.path().join("mine.gguf");
    fs::write(&mine, random_bytes(500, 1)).unwrap();
    f.write_model("dropped.gguf", &random_bytes(700, 1));
    let gone = f.root.path().join("gone.gguf");
    f.set_config(serde_json::json!({
        "model": { "active": "custom/lan" },
        "providers": {
            "groq": { "base_url": "https://proxy.test/groq/v1" },
            "lan": { "base_url": "http://192.168.1.20:8000/v1", "model": "large-v3" },
            "vpn": { "base_url": "https://stt.example.com/v1", "api_key": "secret:vpn-key" },
            "nourl": { "api_key": "secret:nourl" },
        },
        "local_models": [
            { "id": "mine", "name": "My model", "path": mine },
            { "id": "dropped", "path": "dropped.gguf" },
            { "id": "gone", "path": gone },
            { "id": "alpha", "path": mine },
        ],
    }));
    *f.loaded.lock() = Some("mine".into());
    let manager = f.ready().await;
    let snapshot = manager.snapshot();

    let custom: Vec<&str> = snapshot.custom.iter().map(ModelEntry::id).collect();
    assert_eq!(custom, ["mine", "dropped", "gone", "custom/lan", "custom/vpn"]);
    let entry = manager.local_entry("mine").unwrap();
    assert_eq!(entry.id, "mine");
    assert_eq!(entry.name, "My model");
    assert_eq!(entry.file, mine.to_string_lossy());
    assert_eq!(entry.size_bytes, 500);
    assert_eq!(entry.sha256, "0".repeat(64));
    assert_eq!(entry.licence, "User-provided");
    let dropped = manager.local_entry("dropped").unwrap();
    assert_eq!((dropped.name.as_str(), dropped.size_bytes), ("dropped.gguf", 700));
    // A catalog id cannot be shadowed by a registered file.
    assert_eq!(manager.local_entry("alpha"), Some(f.entry_a.clone()));
    assert_eq!(manager.local_entry("nope"), None);

    assert_eq!(snapshot.status["mine"], ModelStatus { custom: true, ..status("mine", true, false, true) });
    assert!(!snapshot.status["gone"].ready && snapshot.status["gone"].custom);
    assert_eq!(manager.model_path("mine"), Some(mine.clone()));
    assert_eq!(manager.model_path("dropped"), Some(f.models_dir.join("dropped.gguf")));
    assert_eq!(manager.model_path("gone"), None);
    assert!(!manager.is_installed("gone"));
    assert_eq!(manager.installed_ids(), ["mine", "dropped"]);

    assert_eq!(
        manager.cloud_entry("custom/lan"),
        Some(CloudModelEntry {
            id: "custom/lan".into(),
            provider: "lan".into(),
            model: "large-v3".into(),
            name: "lan".into(),
            description: "Custom endpoint at http://192.168.1.20:8000/v1".into(),
            price_per_hour: 0.0,
            recommended: false,
            hints: None,
            min_billed_seconds: None,
            docs_url: None,
        })
    );
    assert_eq!(manager.cloud_entry("custom/vpn").unwrap().model, "whisper-1");
    assert_eq!(manager.cloud_entry("custom/groq"), None);
    assert_eq!(manager.cloud_entry("custom/nourl"), None);
    assert_eq!(manager.cloud_entry("groq/whisper-large-v3-turbo").unwrap().provider, "groq");

    // No `api_key` line means the endpoint needs no key; otherwise the key must be stored.
    assert_eq!(snapshot.status["custom/lan"], ModelStatus { custom: true, ..status("custom/lan", true, true, false) });
    assert!(!snapshot.status["custom/vpn"].ready && snapshot.status["custom/vpn"].custom);
    let keys: Vec<(&str, bool)> = snapshot.keys.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_eq!(
        keys,
        [("groq", false), ("openai", false), ("ollama", false), ("lan", false), ("vpn", false), ("nourl", false)]
    );
    f.secrets.lock().insert("vpn-key".into());
    assert!(manager.snapshot().status["custom/vpn"].ready);
    assert!(manager.snapshot().keys["vpn"]);
}

// ── Helpers ─────────────────────────────────────────────────────────────────

#[test]
fn formats_bytes_like_the_ts() {
    assert_eq!(format_bytes(1000), "1 MB");
    assert_eq!(format_bytes(412 * 1024 * 1024 + 1), "413 MB");
    assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    assert_eq!(format_bytes(1_717_986_918), "1.6 GB");
    // `toFixed(1)` rounds an exact half up.
    assert_eq!(format_bytes(1024 * 1024 * 1024 / 4 * 5), "1.3 GB");
}

#[test]
fn describes_errors_in_one_line() {
    let checksum = ModelError::Checksum { expected: "a".into(), actual: "b".into() };
    assert_eq!(describe_error(&checksum), "The downloaded file failed its checksum and was discarded. Try again.");
    assert_eq!(describe_error(&ModelError::Network("ENOTFOUND".into())), "Network error: ENOTFOUND");
    assert_eq!(describe_error(&ModelError::Failed("first\nsecond".into())), "first");
}

#[test]
fn writes_numbers_as_json_stringify_does() {
    assert_eq!(js_number(40_000.0).to_string(), "40000");
    assert_eq!(js_number(1_727_956_800_123.0).to_string(), "1727956800123");
    // Fractional times are written in full and read back exactly.
    let mtime = 1_727_956_800_123.456_7;
    let text = js_number(mtime).to_string();
    assert!(text.starts_with("1727956800123.45") && !text.contains('e'), "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap().as_f64(), Some(mtime));
}

#[tokio::test]
async fn reuses_an_index_written_by_the_electron_build() {
    // The exact text `JSON.stringify({ version: 1, files }, null, 2)` produced, with the file's
    // real size and mtimeMs: the file is adopted without hashing (it holds the wrong bytes).
    let f = fixture().await;
    f.write_model(&f.entry_a.file, &random_bytes(f.alpha.len(), 55));
    let metadata = fs::metadata(f.models_dir.join(&f.entry_a.file)).unwrap();
    let text = format!(
        "{{\n  \"version\": 1,\n  \"files\": {{\n    \"{}\": {{\n      \"sha256\": \"{}\",\n      \"sizeBytes\": {},\n      \"mtimeMs\": {}\n    }}\n  }}\n}}\n",
        f.entry_a.file,
        f.entry_a.sha256,
        metadata.len(),
        js_number(mtime_ms(&metadata))
    );
    fs::write(f.models_dir.join("installed.json"), &text).unwrap();
    let (manager, seen) = f.create();
    manager.init().await;
    manager.when_idle().await;
    assert!(manager.is_installed("alpha"));
    assert_eq!(seen.changed(), 0);
    // Saving it again writes the same text.
    manager.save_index();
    assert_eq!(fs::read_to_string(f.models_dir.join("installed.json")).unwrap(), text);
}
