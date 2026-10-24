//! App startup: single-instance lock, services, tray, overlay, hotkeys.
//! Mirrors `src/main/index.ts`.
//!
//! Threads: the main thread runs the hidden system window's message loop; a
//! small tokio runtime runs the session, the engine client, downloads and
//! cloud calls; the shell thread owns the tray and the overlay pill; the input
//! hook and microphone capture have their own threads. Everything inbound is
//! funnelled through one channel to a dispatcher task, so no callback ever
//! calls back into the thread that produced it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use flow_audio::{CaptureEvent, DictationCapture};
use flow_cloud::CloudDeps;
use flow_core::config::{Config, ConfigSnapshot, ConfigStore, ThemeSetting};
use flow_core::matcher::HotkeyEvent;
use flow_core::protocol::engine::EngineModelRef;
use flow_core::protocol::settings::Event;
use flow_core::types::{CaptureStart, DegradedModule, InsertResult, OverlayAppearance, OverlayState, ThemeInfo};
use flow_engine::{EngineClient, EngineOptions};
use flow_shell::{Overlay, Tray, TrayAction, TrayModel, TrayState};
use flow_store::{HistoryStore, ModelEvent, ModelManager, ModelManagerDeps, SecretStore};
use flow_win::{InputHook, InputHookDeps, InsertOptions, Inserter, Platform};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::paths::Paths;
use crate::router::Router;
use crate::session::{
    BoxFuture, DictationSession, InsertRequest, SessionCapture, SessionDeps, SessionEvent, SessionOverlay,
};
use crate::settings_host::SettingsHost;
use crate::system::{self, SystemEvent};
use crate::updater::Updater;

/// Everything that reaches the dispatcher.
pub enum AppMsg {
    Hotkey(HotkeyEvent),
    Capture(CaptureEvent),
    Session(SessionEvent),
    Config(ConfigSnapshot),
    Models(ModelEvent),
    EngineLoaded,
    Tray(TrayAction),
    System(SystemEvent),
}

/// The services, shared by the dispatcher and the settings-window handlers.
pub struct App {
    pub paths: Paths,
    pub rt: tokio::runtime::Handle,
    pub http: reqwest::Client,
    pub config: Arc<ConfigStore>,
    pub secrets: Arc<SecretStore>,
    pub history: Option<Arc<Mutex<HistoryStore>>>,
    pub models: Arc<ModelManager>,
    pub engine: Arc<EngineClient>,
    pub router: Arc<Router>,
    pub session: Arc<DictationSession>,
    pub hook: Arc<InputHook>,
    pub hook_started: AtomicBool,
    pub platform: Arc<dyn Platform>,
    pub inserter: Arc<Inserter>,
    pub capture: Arc<DictationCapture>,
    pub overlay: Arc<Overlay>,
    pub tray: Arc<Tray>,
    pub settings: Arc<SettingsHost>,
    pub updater: Arc<Updater>,
    pub degraded: Mutex<Vec<DegradedModule>>,
    pub theme: Mutex<ThemeInfo>,
    pub bench_run: AtomicU64,
    pub tx: mpsc::UnboundedSender<AppMsg>,
    pub quitting: AtomicBool,
}

impl App {
    /// API keys for providers, decrypted on demand. Main process only.
    pub fn secret_getter(&self) -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
        let secrets = self.secrets.clone();
        Arc::new(move |id: &str| secrets.get(id))
    }

    pub fn cloud_deps(&self, config: Arc<Config>) -> CloudDeps {
        CloudDeps {
            catalog: self.models.catalog(),
            config,
            get_secret: self.secret_getter(),
            http: self.http.clone(),
        }
    }

    pub fn theme(&self) -> ThemeInfo {
        self.theme.lock().clone()
    }

    fn tray_models(&self) -> Vec<TrayModel> {
        let snapshot = self.models.snapshot();
        let mut entries: Vec<(String, String, bool)> = Vec::new();
        for m in &snapshot.catalog.local {
            entries.push((m.id.clone(), m.name.clone(), false));
        }
        for m in &snapshot.catalog.cloud {
            entries.push((m.id.clone(), m.name.clone(), true));
        }
        for m in &snapshot.custom {
            entries.push((m.id().to_string(), m.name().to_string(), m.is_cloud()));
        }
        entries
            .into_iter()
            .filter(|(id, _, _)| snapshot.status.get(id).is_some_and(|s| s.ready))
            .map(|(id, name, cloud)| TrayModel { id, name, cloud })
            .collect()
    }

    pub fn update_tray(&self) {
        self.tray.update(TrayState {
            active_model_id: self.config.get().model.active.clone(),
            models: self.tray_models(),
            paused: self.session.is_paused(),
            has_last: self.session.last_text().is_some(),
            can_retry: self.session.can_retry(),
            hook_failed: !self.hook_started.load(Ordering::Relaxed),
        });
    }

    fn overlay_appearance(&self, cfg: &Config) -> OverlayAppearance {
        OverlayAppearance {
            style: cfg.overlay.style,
            size: cfg.overlay.size,
            show_waveform: cfg.overlay.show_waveform,
            sounds: cfg.audio.sounds,
            theme: self.theme(),
        }
    }

    pub fn apply_config(&self, cfg: &Config) {
        crate::logger::set_debug(cfg.general.debug_logging);
        *self.theme.lock() = system::system_theme(cfg.general.theme);
        self.hook.set_bindings(&cfg.hotkeys);
        self.engine.set_keep_loaded_minutes(cfg.model.keep_loaded_minutes);
        self.overlay.configure(self.overlay_appearance(cfg), cfg.overlay.position);
        if self.paths.packaged {
            if let Err(err) = system::set_launch_at_login(cfg.general.launch_at_login, &self.paths.exe) {
                log::warn!(target: "app", "launch at login: {err}");
            }
        }
        if cfg.history.enabled {
            if let Some(history) = &self.history {
                if let Err(err) = history.lock().prune(cfg.history.retention_days) {
                    log::warn!(target: "history", "prune failed: {err}");
                }
            }
        }
        // "Always loaded" keeps the model in memory from launch.
        if cfg.model.keep_loaded_minutes < 0.0 {
            self.router.warm_up_config(Arc::new(cfg.clone()));
        }
        self.update_tray();
    }

    fn on_theme_changed(&self) {
        let cfg = self.config.get();
        let next = system::system_theme(cfg.general.theme);
        let changed = *self.theme.lock() != next;
        *self.theme.lock() = next.clone();
        if changed {
            self.overlay.configure(self.overlay_appearance(&cfg), cfg.overlay.position);
            self.settings.send(Event::ThemeChanged(next));
        }
    }

    pub fn quit(&self) {
        if self.quitting.swap(true, Ordering::SeqCst) {
            return;
        }
        log::info!(target: "app", "quitting");
        self.hook.stop();
        self.engine.shutdown();
        self.settings.close();
        self.overlay.destroy();
        self.tray.destroy();
        if let Some(history) = &self.history {
            history.lock().close();
        }
        self.config.close();
        system::quit();
    }

    async fn dispatch(self: Arc<Self>, msg: AppMsg) {
        match msg {
            AppMsg::Hotkey(event) => self.session.handle_hotkey(event),
            AppMsg::Capture(event) => match event {
                CaptureEvent::Started => self.session.on_capture_started(),
                CaptureEvent::Chunk(pcm) => self.session.on_capture_chunk(pcm),
                CaptureEvent::Stopped => self.session.on_capture_stopped(),
                CaptureEvent::Error { reason, message } => self.session.on_capture_error(reason, Some(message)),
            },
            AppMsg::Session(event) => match event {
                SessionEvent::Dictation(event) => {
                    if crate::test_mode::enabled() {
                        crate::test_mode::dictation(&event);
                    }
                    self.settings.send(Event::DictationEvent(event));
                    self.update_tray();
                }
                SessionEvent::Paused(_) => self.update_tray(),
                SessionEvent::State(_) => {}
            },
            AppMsg::Config(snapshot) => {
                self.apply_config(&snapshot.config);
                self.settings.send(Event::ConfigChanged(snapshot));
                self.settings.send(Event::ModelsChanged(self.models.snapshot()));
            }
            AppMsg::Models(ModelEvent::Progress(progress)) => self.settings.send(Event::ModelsProgress(progress)),
            AppMsg::Models(ModelEvent::Changed) => {
                self.settings.send(Event::ModelsChanged(self.models.snapshot()));
                self.update_tray();
            }
            AppMsg::EngineLoaded => self.settings.send(Event::ModelsChanged(self.models.snapshot())),
            AppMsg::Tray(action) => match action {
                TrayAction::ActivateModel(id) => {
                    let patch = flow_core::config::ConfigPatch::set(flow_core::path!["model", "active"], id);
                    if let Err(err) = self.config.set(&[patch]) {
                        log::warn!(target: "tray", "could not switch model: {err}");
                    }
                }
                TrayAction::SetPaused(paused) => self.session.set_paused(paused),
                TrayAction::CopyLast => {
                    self.session.copy_last();
                }
                TrayAction::RetryLast => {
                    self.session.retry_last();
                }
                TrayAction::OpenSettings => self.settings.open(None),
                TrayAction::Quit => self.quit(),
            },
            AppMsg::System(event) => match event {
                SystemEvent::SecondInstance(args) => {
                    let page = page_arg(&args);
                    self.settings.open(page.as_deref());
                }
                // Keys released while asleep never produce a key-up, so forget what was held.
                SystemEvent::Resumed => {
                    self.hook.reset();
                    self.session.cancel();
                }
                SystemEvent::Locked => self.session.cancel(),
                SystemEvent::SettingsChanged => self.on_theme_changed(),
            },
        }
    }
}

/// `--page models` from a command line.
pub fn page_arg(args: &[String]) -> Option<String> {
    args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).cloned()
}

// ── Session adapters ───────────────────────────────────────────────────────

struct CaptureAdapter(Arc<DictationCapture>);
impl SessionCapture for CaptureAdapter {
    fn start_capture(&self, options: CaptureStart) {
        self.0.start(&options);
    }
    fn stop_capture(&self) {
        self.0.stop();
    }
}

struct OverlayAdapter(Arc<Overlay>);
impl SessionOverlay for OverlayAdapter {
    fn set_state(&self, state: OverlayState) {
        if crate::test_mode::enabled() {
            crate::test_mode::overlay(&state);
        }
        self.0.set_state(state);
    }
}

/// Runs the main process. Returns the exit code.
pub fn run(args: Vec<String>) -> i32 {
    system::init_process();
    if !system::acquire_single_instance(&args) {
        return 0;
    }
    let paths = Paths::resolve();
    crate::logger::init(&paths.logs);
    log::info!(target: "app", "Flow {} starting (Rust)", env!("CARGO_PKG_VERSION"));
    let started = Instant::now();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("flow-rt")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            log::error!(target: "app", "could not start the runtime: {err}");
            return 1;
        }
    };
    let rt = runtime.handle().clone();
    let (tx, mut rx) = mpsc::unbounded_channel::<AppMsg>();

    let app = match rt.block_on(build(paths, rt.clone(), tx.clone())) {
        Ok(app) => app,
        Err(err) => {
            log::error!(target: "app", "startup failed: {err:#}");
            return 1;
        }
    };

    // The dispatcher: one message at a time, in order.
    {
        let app = app.clone();
        rt.spawn(async move {
            while let Some(msg) = rx.recv().await {
                app.clone().dispatch(msg).await;
            }
        });
    }

    let window_tx = tx.clone();
    if let Err(err) = system::create_main_window(
        Arc::new(move |event| {
            let _ = window_tx.send(AppMsg::System(event));
        }),
        Box::new(|| {}),
    ) {
        log::error!(target: "app", "could not create the main window: {err}");
    }

    app.apply_config(&app.config.get());
    let test_mode = crate::test_mode::enabled();
    if test_mode {
        let commands = app.clone();
        crate::test_mode::spawn_reader(tx.clone(), move |command| match command {
            crate::test_mode::Command::OpenSettings(page) => commands.settings.open(page.as_deref()),
            crate::test_mode::Command::Quit => commands.quit(),
        });
    }
    // Tests drive the session directly: the real keyboard is left alone.
    let hook_started = if test_mode { true } else { app.hook.start() };
    app.hook_started.store(hook_started, Ordering::Relaxed);
    if !hook_started {
        log::error!(target: "input", "the global input hook failed to start; hotkeys are unavailable");
        app.degraded.lock().push(DegradedModule { module: "input hook".into(), effect: "Hotkeys are unavailable".into() });
    }
    app.update_tray();

    let hidden = args.iter().any(|a| a == "--hidden");
    if args.iter().any(|a| a == "--settings") || (!app.config.get().general.onboarded && !hidden) {
        app.settings.open(page_arg(&args).as_deref());
    }

    // Release CI launches the packaged app with this flag to prove the native parts load.
    if args.iter().any(|a| a == "--smoke-test") {
        let code = rt.block_on(async {
            match app.engine.probe().await {
                Ok(probe) => {
                    let ok = hook_started && app.platform.available() && app.history.is_some();
                    log::info!(target: "smoke", "engine {}, vad {:?}, ok={ok}", probe.version, probe.vad);
                    if ok { 0 } else { 1 }
                }
                Err(err) => {
                    log::error!(target: "smoke", "{err}");
                    1
                }
            }
        });
        app.quit();
        return code;
    }

    {
        let models = app.models.clone();
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            models.refresh_catalog().await;
        });
    }
    {
        // The cleanup rules compile their patterns on first use (~30 ms, more
        // with a long dictionary); do it now rather than in the first dictation.
        let config = app.config.get();
        rt.spawn_blocking(move || {
            flow_core::cleanup::apply_rules("warm up", &config, &flow_core::cleanup::RulesContext::default())
        });
    }
    app.updater.schedule_check();
    log::info!(target: "app", "tray ready in {} ms", started.elapsed().as_millis());
    if test_mode {
        crate::test_mode::ready();
    }

    system::run_message_loop();
    let pending_update = app.updater.take_pending_install();
    drop(app);
    runtime.shutdown_timeout(Duration::from_secs(2));
    if let Some(install) = pending_update {
        install();
    }
    0
}

async fn build(paths: Paths, rt: tokio::runtime::Handle, tx: mpsc::UnboundedSender<AppMsg>) -> anyhow::Result<Arc<App>> {
    let mut degraded = Vec::new();
    let http = reqwest::Client::builder().user_agent(concat!("Flow/", env!("CARGO_PKG_VERSION"))).no_proxy().build()?;

    // ── Services ─────────────────────────────────────────────────────────────
    let config = ConfigStore::new(paths.config_file());
    config.load();
    config.watch();

    let secrets = Arc::new(SecretStore::new(paths.user_data.join("secrets.bin"), flow_store::system_cipher(&paths.user_data)));
    secrets.load();

    let history = match HistoryStore::open(&paths.user_data.join("history.db")) {
        Ok(store) => Some(Arc::new(Mutex::new(store))),
        Err(err) => {
            log::error!(target: "history", "unavailable: {err}");
            degraded.push(DegradedModule { module: "SQLite".into(), effect: "History is disabled".into() });
            None
        }
    };

    // Optional GPU pack: a CUDA build of the speech library plus the CUDA runtime,
    // dropped into this folder. Without it, GPUs are used through Vulkan.
    let gpu_library = paths.gpu_pack.join("transcribe.dll");
    let has_gpu_pack = gpu_library.is_file();
    if has_gpu_pack {
        log::info!(target: "engine", "using the GPU pack at {}", paths.gpu_pack.display());
    }
    let engine = EngineClient::new(EngineOptions {
        exe: paths.exe.clone(),
        args: vec!["--engine".into()],
        library_path: Some(if has_gpu_pack { gpu_library } else { paths.engine_library.clone() }),
        cuda_runtime_dir: has_gpu_pack.then(|| paths.gpu_pack.clone()),
        debug: config.get().general.debug_logging,
    });
    {
        let tx = tx.clone();
        engine.on_loaded(move |_id| {
            let _ = tx.send(AppMsg::EngineLoaded);
        });
    }

    let models = {
        let get_config = {
            let config = config.clone();
            Arc::new(move || config.get())
        };
        let has_secret = {
            let secrets = secrets.clone();
            Arc::new(move |id: &str| secrets.has(id))
        };
        let loaded = {
            let engine = engine.clone();
            Arc::new(move || engine.loaded_model_id())
        };
        let tx = tx.clone();
        ModelManager::new(
            ModelManagerDeps {
                models_dir: paths.models.clone(),
                user_data_dir: paths.user_data.clone(),
                bundled_catalog_path: paths.resource("catalog.json"),
                get_config,
                has_secret,
                loaded_model_id: loaded,
                http: http.clone(),
                catalog_url: None,
                catalog_public_key: None,
                free_bytes: None,
            },
            Arc::new(move |event| {
                let _ = tx.send(AppMsg::Models(event));
            }),
        )
    };
    models.init().await;

    let platform = flow_win::create_platform();
    if !platform.available() {
        degraded.push(DegradedModule {
            module: "Win32".into(),
            effect: "Typing method and per-app profiles are disabled".into(),
        });
    }

    let hook = {
        let tx = tx.clone();
        let mask_platform = platform.clone();
        let key_platform = platform.clone();
        InputHook::new(
            InputHookDeps {
                mask_start_menu: Some(Arc::new(move || mask_platform.mask_start_menu())),
                // Key-ups that go to an elevated window are never seen by the hook; this lets it notice.
                is_key_down: Some(Arc::new(move |key: &str| key_platform.is_key_down(key))),
            },
            Arc::new(move |event| {
                let _ = tx.send(AppMsg::Hotkey(event));
            }),
        )
    };
    let inserter = Arc::new(Inserter::new(platform.clone(), flow_win::system_clipboard()));

    let router = {
        let secrets = secrets.clone();
        Router::new(
            Arc::new(crate::router::EngineAdapter(engine.clone())),
            models.clone(),
            Arc::new(move |id: &str| secrets.get(id)),
            http.clone(),
        )
    };

    // ── Windows ──────────────────────────────────────────────────────────────
    let overlay = Arc::new(Overlay::create());
    let capture = {
        let tx = tx.clone();
        Arc::new(DictationCapture::new(Arc::new(move |event| {
            let _ = tx.send(AppMsg::Capture(event));
        })))
    };
    overlay.set_analyser(Some(capture.analyser()));
    let settings = SettingsHost::new(paths.exe.clone(), rt.clone());

    let session = {
        let get_config = {
            let config = config.clone();
            Arc::new(move || config.get())
        };
        let get_catalog = {
            let models = models.clone();
            Arc::new(move || models.catalog())
        };
        let get_focused_app = {
            let platform = platform.clone();
            Arc::new(move || platform.get_focused_app())
        };
        let insert = {
            let inserter = inserter.clone();
            Arc::new(move |text: String, request: InsertRequest| -> BoxFuture<InsertResult> {
                let inserter = inserter.clone();
                Box::pin(async move {
                    if crate::test_mode::enabled() {
                        crate::test_mode::inserted(&text, request.target.as_ref().map(|t| t.process_name.as_str()));
                        return InsertResult::ok(flow_core::types::InsertOutcome::Pasted);
                    }
                    let options = InsertOptions {
                        method: request.method,
                        paste_shortcut: request.paste_shortcut,
                        restore_clipboard: request.restore_clipboard,
                        target: request.target,
                        cancel: None,
                    };
                    // Insertion can wait seconds for modifiers to be released: keep it off the runtime.
                    tokio::task::spawn_blocking(move || inserter.insert(&text, &options)).await.unwrap_or_else(|_| {
                        InsertResult::clipboard(flow_core::types::InsertFailure::Failed)
                    })
                })
            })
        };
        let copy = {
            let inserter = inserter.clone();
            Arc::new(move |text: &str| inserter.copy(text))
        };
        let trim = {
            let engine = engine.clone();
            Arc::new(move |audio: Arc<Vec<f32>>| -> BoxFuture<Result<Option<Vec<f32>>, String>> {
                let engine = engine.clone();
                Box::pin(async move { engine.trim(&audio).await.map(|t| t.audio).map_err(|e| e.to_string()) })
            })
        };
        let cleanup_llm = {
            let secrets = secrets.clone();
            let http = http.clone();
            Arc::new(
                move |text: String, config: Arc<Config>, catalog: Arc<flow_core::catalog::Catalog>, cancel: CancellationToken| -> BoxFuture<Result<String, String>> {
                    let secrets = secrets.clone();
                    let http = http.clone();
                    Box::pin(async move {
                        let deps = CloudDeps {
                            catalog,
                            config,
                            get_secret: Arc::new(move |id: &str| secrets.get(id)),
                            http,
                        };
                        flow_cloud::cleanup_with_llm(&text, &deps, Some(&cancel)).await.map_err(|e| e.to_string())
                    })
                },
            )
        };
        let set_cancel_armed = {
            let hook = hook.clone();
            Arc::new(move |armed: bool| hook.set_cancel_armed(armed))
        };
        let add_history = {
            let history = history.clone();
            Arc::new(move |entry: flow_core::types::NewHistoryEntry| {
                if let Some(history) = &history {
                    if let Err(err) = history.lock().add(&entry) {
                        log::warn!(target: "session", "history write failed: {err}");
                    }
                }
            })
        };
        let on_event = {
            let tx = tx.clone();
            Arc::new(move |event: SessionEvent| {
                let _ = tx.send(AppMsg::Session(event));
            })
        };
        let clock = Instant::now();
        DictationSession::new(SessionDeps {
            get_config,
            get_catalog,
            get_focused_app,
            insert,
            copy,
            router: router.clone(),
            trim,
            cleanup_llm,
            capture: match crate::test_mode::FakeMic::from_env(tx.clone()) {
                Some(fake) if crate::test_mode::enabled() => Arc::new(fake),
                _ => Arc::new(CaptureAdapter(capture.clone())),
            },
            overlay: Arc::new(OverlayAdapter(overlay.clone())),
            set_cancel_armed,
            add_history,
            on_event,
            now: Arc::new(move || clock.elapsed().as_secs_f64() * 1000.0),
            epoch_ms: Arc::new(|| chrono::Utc::now().timestamp_millis()),
        })
    };

    let tray = {
        let tx = tx.clone();
        Arc::new(Tray::create(Arc::new(move |action| {
            let _ = tx.send(AppMsg::Tray(action));
        })))
    };

    let updater = Updater::new(http.clone(), paths.packaged);
    let theme = system::system_theme(config.get().general.theme);

    let app = Arc::new(App {
        paths,
        rt: rt.clone(),
        http,
        config: config.clone(),
        secrets,
        history,
        models,
        engine,
        router,
        session,
        hook: hook.clone(),
        hook_started: AtomicBool::new(false),
        platform,
        inserter,
        capture,
        overlay,
        tray,
        settings: settings.clone(),
        updater,
        degraded: Mutex::new(degraded),
        theme: Mutex::new(theme),
        bench_run: AtomicU64::new(0),
        tx: tx.clone(),
        quitting: AtomicBool::new(false),
    });

    // ── Wiring ───────────────────────────────────────────────────────────────
    {
        let tx = tx.clone();
        config.on_changed(move |snapshot| {
            let _ = tx.send(AppMsg::Config(snapshot.clone()));
        });
    }
    settings.set_handler(crate::ipc::handler(Arc::downgrade(&app)));
    {
        let hook = hook.clone();
        settings.on_closed(Arc::new(move || hook.cancel_capture()));
    }
    Ok(app)
}

#[allow(dead_code)]
fn _model_ref_used(_: EngineModelRef) {}

#[allow(dead_code)]
fn _theme_setting_used(_: ThemeSetting) {}
