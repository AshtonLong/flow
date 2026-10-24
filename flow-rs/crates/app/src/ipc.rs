//! Requests from the settings window. Every request is checked against the
//! limits in `flow_core::protocol::settings::limits` before it reaches a
//! service. Mirrors `src/main/ipc.ts`.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use flow_core::catalog::{cloud_cost, is_cloud_model_id};
use flow_core::cleanup::dictionary_hints;
use flow_core::config::{ConfigPatch, PathSeg};
use flow_core::protocol::settings::{BenchResult, Event, Reply, Request, RequestEnvelope, limits};
use flow_core::types::{AppInfo, SAMPLE_RATE};

use crate::app::App;
use crate::session::BoxFuture;
use crate::settings_host::RequestHandler;

/// Builds the request handler. Holds the app weakly so a late request after
/// quit is simply refused.
pub fn handler(app: Weak<App>) -> RequestHandler {
    Arc::new(move |envelope: RequestEnvelope| -> BoxFuture<Reply> {
        let app = app.clone();
        Box::pin(async move {
            let Some(app) = app.upgrade() else { return Reply::err("Flow is quitting") };
            handle(app, envelope.request).await
        })
    })
}

fn ok() -> Reply {
    Reply::ok(&())
}

fn check_model_id(id: &str) -> Result<(), Reply> {
    if id.is_empty() || id.len() > limits::MAX_MODEL_ID {
        return Err(Reply::err("Invalid model id"));
    }
    Ok(())
}

fn check_provider(id: &str) -> Result<(), Reply> {
    if limits::is_provider_id(id) { Ok(()) } else { Err(Reply::err("Invalid provider id")) }
}

async fn handle(app: Arc<App>, request: Request) -> Reply {
    match request {
        // ── Config ───────────────────────────────────────────────────────
        Request::ConfigGet => Reply::ok(&app.config.snapshot()),
        Request::ConfigSet(patches) => {
            if patches.len() > limits::MAX_PATCHES || patches.iter().any(|p| p.path.is_empty()) {
                return Reply::err("Invalid config patch");
            }
            let config = app.config.clone();
            match tokio::task::spawn_blocking(move || config.set(&patches)).await {
                Ok(Ok(snapshot)) => Reply::ok(&snapshot),
                Ok(Err(err)) => Reply::err(err.to_string()),
                Err(err) => Reply::err(err.to_string()),
            }
        }
        Request::ConfigOpenFile => {
            crate::system::shell_open(&app.config.path().to_string_lossy());
            ok()
        }
        Request::ConfigExport(path) => match app.config.export_to(std::path::Path::new(&path)) {
            Ok(()) => ok(),
            Err(err) => Reply::err(err.to_string()),
        },
        Request::ConfigImport(path) => match app.config.import_from(std::path::Path::new(&path)) {
            Ok(snapshot) => Reply::ok(&snapshot),
            Err(err) => Reply::err(err.to_string()),
        },

        // ── Models ───────────────────────────────────────────────────────
        Request::ModelsList => Reply::ok(&app.models.snapshot()),
        Request::ModelsDownload(id) => {
            if let Err(reply) = check_model_id(&id) {
                return reply;
            }
            // Failures are reported through progress events, which the UI already shows.
            let models = app.models.clone();
            app.rt.spawn(async move {
                if let Err(err) = models.download(&id).await {
                    log::warn!(target: "models", "download {id}: {err}");
                }
            });
            ok()
        }
        Request::ModelsCancel(id) => {
            if let Err(reply) = check_model_id(&id) {
                return reply;
            }
            app.models.cancel_download(&id);
            ok()
        }
        Request::ModelsRemove(id) => {
            if let Err(reply) = check_model_id(&id) {
                return reply;
            }
            if app.engine.loaded_model_id().as_deref() == Some(id.as_str()) {
                let _ = app.engine.unload().await;
            }
            match app.models.remove(&id).await {
                Ok(()) => ok(),
                Err(err) => Reply::err(err.to_string()),
            }
        }
        Request::ModelsActivate(id) => {
            if let Err(reply) = check_model_id(&id) {
                return reply;
            }
            set_config(&app, vec![ConfigPatch::set(flow_core::path!["model", "active"], id)]).await
        }
        Request::ModelsRefresh => {
            app.models.refresh_catalog().await;
            Reply::ok(&app.models.snapshot())
        }

        // ── Secrets: keys flow one way, into the main process ────────────
        Request::SecretsSet { provider, key } => {
            if let Err(reply) = check_provider(&provider) {
                return reply;
            }
            if key.len() > limits::MAX_SECRET {
                return Reply::err("Key too long");
            }
            let key = key.trim().to_string();
            if let Err(err) = app.secrets.set(&provider, &key) {
                return Reply::err(err.to_string());
            }
            // The file holds only a reference to the key.
            let reference = format!("secret:{provider}");
            let current = app.config.get().providers.get(&provider).and_then(|p| p.api_key.clone());
            if !key.is_empty() && current.as_deref() != Some(reference.as_str()) {
                let path = vec![PathSeg::from("providers"), PathSeg::from(provider.as_str()), PathSeg::from("api_key")];
                if let Err(err) = app.config.set(&[ConfigPatch::set(path, reference)]) {
                    return Reply::err(err.to_string());
                }
            }
            app.models_changed();
            ok()
        }
        Request::SecretsHas(provider) => {
            if let Err(reply) = check_provider(&provider) {
                return reply;
            }
            Reply::ok(&app.secrets.has(&provider))
        }
        Request::SecretsDelete(provider) => {
            if let Err(reply) = check_provider(&provider) {
                return reply;
            }
            if let Err(err) = app.secrets.delete(&provider) {
                return Reply::err(err.to_string());
            }
            if app.config.get().providers.get(&provider).is_some_and(|p| p.api_key.is_some()) {
                let path = vec![PathSeg::from("providers"), PathSeg::from(provider.as_str()), PathSeg::from("api_key")];
                if let Err(err) = app.config.set(&[ConfigPatch::remove(path)]) {
                    return Reply::err(err.to_string());
                }
            }
            app.models_changed();
            ok()
        }
        Request::SecretsTest(provider) => {
            if let Err(reply) = check_provider(&provider) {
                return reply;
            }
            let deps = app.cloud_deps(app.config.get());
            Reply::ok(&flow_cloud::test_provider_key(&provider, &deps).await)
        }

        // ── Test bench ───────────────────────────────────────────────────
        Request::BenchRun { audio, model_ids } => {
            if audio.0.len() > limits::MAX_PCM_SAMPLES {
                return Reply::err("audio too long");
            }
            if model_ids.is_empty() || model_ids.len() > limits::MAX_BENCH_MODELS {
                return Reply::err("Invalid model list");
            }
            if model_ids.iter().any(|id| check_model_id(id).is_err()) {
                return Reply::err("Invalid model id");
            }
            let run_id = app.bench_run.fetch_add(1, Ordering::SeqCst) + 1;
            let bench = app.clone();
            app.rt.spawn(async move { run_bench(bench, run_id, audio.0, model_ids).await });
            Reply::ok(&run_id)
        }

        // ── Hotkey recorder ──────────────────────────────────────────────
        Request::HotkeysCapture => {
            let settings = app.settings.clone();
            let receiver = app.hook.capture_binding(Arc::new(move |partial: String| {
                settings.send(Event::HotkeysCaptureProgress(partial));
            }));
            let binding = tokio::task::spawn_blocking(move || receiver.recv().ok().flatten()).await.ok().flatten();
            Reply::ok(&binding)
        }
        Request::HotkeysCaptureCancel => {
            app.hook.cancel_capture();
            ok()
        }

        // ── History ──────────────────────────────────────────────────────
        Request::HistorySearch { query, limit, offset } => {
            if query.len() > limits::MAX_HISTORY_QUERY || limit == 0 || limit > limits::MAX_HISTORY_LIMIT {
                return Reply::err("Invalid search");
            }
            let Some(history) = &app.history else { return Reply::ok(&Vec::<()>::new()) };
            match history.lock().search(&query, limit, offset) {
                Ok(rows) => Reply::ok(&rows),
                Err(err) => Reply::err(err.to_string()),
            }
        }
        Request::HistoryDelete(id) => {
            if let Some(history) = &app.history {
                if let Err(err) = history.lock().delete(id) {
                    return Reply::err(err.to_string());
                }
            }
            ok()
        }
        Request::HistoryClear => {
            if let Some(history) = &app.history {
                if let Err(err) = history.lock().clear() {
                    return Reply::err(err.to_string());
                }
            }
            ok()
        }
        Request::HistoryRerun(id) => {
            let Some(history) = app.history.clone() else { return Reply::ok(&Option::<()>::None) };
            let entry = history.lock().get(id).ok().flatten();
            let Some(entry) = entry else { return Reply::ok(&Option::<()>::None) };
            let text = app.session.clean_up(&entry.raw_text, app.config.get(), None).await;
            match history.lock().update_text(id, &text) {
                Ok(updated) => Reply::ok(&updated),
                Err(err) => Reply::err(err.to_string()),
            }
        }

        // ── Dictation ────────────────────────────────────────────────────
        Request::DictationTest(audio) => {
            if audio.0.len() > limits::MAX_PCM_SAMPLES {
                return Reply::err("audio too long");
            }
            match app.session.test(audio.0).await {
                Ok(transcript) => Reply::ok(&transcript),
                Err(failure) => Reply::err(failure.user_message),
            }
        }

        // ── App ──────────────────────────────────────────────────────────
        Request::AppInfo => {
            if app.engine.devices().is_empty() {
                // First ask: probe the engine, but do not hold the window up for long.
                let _ = tokio::time::timeout(Duration::from_secs(4), async {
                    if let Err(err) = app.engine.probe().await {
                        log::warn!(target: "engine", "probe failed: {err}");
                    }
                })
                .await;
            }
            Reply::ok(&AppInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                runtime: "Rust".into(),
                platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
                config_path: app.config.path().to_string_lossy().into_owned(),
                logs_path: app.paths.logs.to_string_lossy().into_owned(),
                models_path: app.paths.models.to_string_lossy().into_owned(),
                devices: app.engine.devices(),
                degraded: app.degraded.lock().clone(),
                paused: app.session.is_paused(),
            })
        }
        Request::AppOpenLogs => {
            crate::system::shell_open(&app.paths.logs.to_string_lossy());
            ok()
        }
        Request::AppOpenModels => {
            let _ = std::fs::create_dir_all(&app.paths.models);
            crate::system::shell_open(&app.paths.models.to_string_lossy());
            ok()
        }
        Request::AppOpenExternal(url) => {
            if !is_https_url(&url) {
                return Reply::err("Only https links can be opened");
            }
            crate::system::shell_open(&url);
            ok()
        }
        Request::AppCopy(text) => {
            if text.len() > limits::MAX_COPY {
                return Reply::err("Text too long");
            }
            app.inserter.copy(&text);
            ok()
        }
        Request::AppSetPaused(paused) => {
            app.session.set_paused(paused);
            ok()
        }
        Request::AppCheckUpdates => Reply::ok(&app.updater.check().await),
        Request::ThemeGet => Reply::ok(&app.theme()),
    }
}

fn is_https_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.scheme() == "https")
}

async fn set_config(app: &Arc<App>, patches: Vec<ConfigPatch>) -> Reply {
    let config = app.config.clone();
    match tokio::task::spawn_blocking(move || config.set(&patches)).await {
        Ok(Ok(_)) => ok(),
        Ok(Err(err)) => Reply::err(err.to_string()),
        Err(err) => Reply::err(err.to_string()),
    }
}

async fn run_bench(app: Arc<App>, run_id: u64, audio: Vec<f32>, model_ids: Vec<String>) {
    let cfg = app.config.get();
    let speech = match app.engine.trim(&audio).await {
        Ok(trimmed) => trimmed.audio.unwrap_or(audio),
        Err(_) => audio,
    };
    let audio_ms = speech.len() as f64 / SAMPLE_RATE as f64 * 1000.0;
    let speech = Arc::new(speech);
    let count = model_ids.len();
    for (i, model_id) in model_ids.into_iter().enumerate() {
        let done = i == count - 1;
        let started = Instant::now();
        let opts = crate::session::TranscribeOpts {
            language: cfg.general.language.clone(),
            hints: dictionary_hints(&cfg),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let result = match app.router.transcribe_with(&model_id, speech.clone(), cfg.clone(), opts).await {
            Ok(transcript) => {
                let entry = if is_cloud_model_id(&model_id) { app.models.cloud_entry(&model_id) } else { None };
                BenchResult {
                    run_id,
                    model_id,
                    ok: true,
                    text: Some(transcript.text),
                    elapsed_ms: Some(started.elapsed().as_secs_f64() * 1000.0),
                    cost: entry.map(|e| cloud_cost(&e, audio_ms)),
                    error: None,
                    done,
                }
            }
            Err(failure) => BenchResult {
                run_id,
                model_id,
                ok: false,
                text: None,
                elapsed_ms: None,
                cost: None,
                error: Some(failure.user_message),
                done,
            },
        };
        app.settings.send(Event::BenchResult(result));
    }
}

impl App {
    /// The catalog snapshot changed (a key was added or removed).
    pub fn models_changed(&self) {
        let _ = self.tx.send(crate::app::AppMsg::Models(flow_store::ModelEvent::Changed));
    }
}
