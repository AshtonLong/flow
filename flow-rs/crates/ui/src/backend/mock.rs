//! An in-memory backend for running the settings window without Flow: the
//! preview example, screenshots and tests. Port of
//! `src/renderer/settings/mock.ts`.
//!
//! [`MockOptions`] shape the starting state, like the TS query parameters:
//! `theme`, `contrast=high`, `motion=reduced`, `accent`, `onboarding` (first-run
//! flow), `fresh` (nothing installed, no keys, empty lists), `history=off`,
//! `downloading=<id>` (a download in flight), `degraded` (a failed native
//! module in `app.info()`), `noissues` (no config problems).

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use flow_core::catalog::{
    CUSTOM_PREFIX, Catalog, CatalogSnapshot, CloudModelEntry, DownloadProgress, DownloadState, ModelEntry, ModelStatus,
    cloud_cost, parse_catalog,
};
use flow_core::config::value::apply_patches;
use flow_core::config::{ConfigPatch, ConfigSnapshot, PathSeg, validate_config};
use flow_core::path;
use flow_core::protocol::settings::{BenchResult, Event, KeyTestResult, Reply, Request};
use flow_core::types::{AppInfo, BackendDevice, DegradedModule, HistoryEntry, SAMPLE_RATE, ThemeInfo, Transcript};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::Backend;
use crate::adapters::hotkeys::{binding_from_keys, format_binding};

const CATALOG_JSON: &[u8] = include_bytes!("../../../../../resources/catalog.json");

/// The starting state of the mock (the TS query parameters).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MockOptions {
    /// `?theme=dark|light`: the system theme. `None` follows Windows.
    pub dark: Option<bool>,
    /// `?contrast=high`
    pub high_contrast: bool,
    /// `?motion=reduced`
    pub reduced_motion: bool,
    /// `?accent=c2410c`
    pub accent: Option<String>,
    /// `?onboarding`: first-run flow (`general.onboarded = false`).
    pub onboarding: bool,
    /// `?fresh`: nothing installed, no keys, empty lists.
    pub fresh: bool,
    /// `?history=off`
    pub history_off: bool,
    /// `?downloading=<id>`: a download in flight for that model.
    pub downloading: Option<String>,
    /// `?degraded`: a failed native module in `app.info()`.
    pub degraded: bool,
    /// `?noissues`: no config problems.
    pub no_issues: bool,
}

/// The mock with its default state.
pub fn mock_backend() -> Arc<dyn Backend> {
    mock_backend_with(MockOptions::default())
}

/// The mock with a starting state.
pub fn mock_backend_with(options: MockOptions) -> Arc<dyn Backend> {
    Arc::new(Mock::new(options))
}

const SENTENCES: [&str; 12] = [
    "Can you send me the quarterly numbers before the stand-up tomorrow?",
    "Remind me to renew the domain for the Tauri prototype on Friday.",
    "I think the retry logic should back off exponentially, starting at two hundred milliseconds.",
    "Thanks for the review. I have pushed a fix for the flaky test and rebased onto main.",
    "The meeting with Priya moved to three thirty, same room.",
    "Add oat milk, coffee filters and a pack of AA batteries to the list.",
    "Let us ship the settings window first and tidy the overlay animation afterwards.",
    "Dear Sam, it was good to meet you last week. I have attached the proposal we discussed.",
    "TODO: handle the case where the microphone is unplugged halfway through a recording.",
    "Sounds good, see you at seven.",
    "The parser drops trailing commas, which is why the config failed to load.",
    "Could you check whether the installer is still under one hundred and twenty megabytes?",
];

const APPS: [&str; 6] = ["Code.exe", "slack.exe", "chrome.exe", "WindowsTerminal.exe", "OUTLOOK.EXE", "notepad.exe"];

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn sample_history(count: usize) -> Vec<HistoryEntry> {
    let now = now_ms();
    let models = ["parakeet-tdt-0.6b-v2", "groq/whisper-large-v3-turbo"];
    (0..count)
        .map(|i| {
            let text = SENTENCES[i % SENTENCES.len()];
            let audio_ms = 1800 + ((i * 937) % 9000) as i64;
            let ii = i as i64;
            let mut first = text.chars();
            let lowered = match first.next() {
                Some(c) => format!("{}{}", c.to_lowercase(), first.as_str()),
                None => String::new(),
            };
            HistoryEntry {
                id: (count - i) as i64,
                created_at: now
                    - ii * 47 * 60_000
                    - if i > 6 { 86_400_000 } else { 0 }
                    - if i > 20 { 40 * 86_400_000 } else { 0 },
                text: text.to_string(),
                raw_text: format!("um {lowered}"),
                model: models[if i % 3 == 0 { 1 } else { 0 }].to_string(),
                app: APPS[i % APPS.len()].to_string(),
                audio_ms,
                elapsed_ms: 380 + ((i * 173) % 500) as i64,
            }
        })
        .collect()
}

// ── A tiny timer thread standing in for setTimeout / setInterval ───────────

type Job = Box<dyn FnOnce() + Send>;
type Listener = Box<dyn Fn(Event) + Send + Sync>;

struct Scheduler {
    tx: Sender<(Instant, Job)>,
}

impl Scheduler {
    fn start() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<(Instant, Job)>();
        let _ = std::thread::Builder::new().name("settings-mock".into()).spawn(move || timer_loop(&rx));
        Self { tx }
    }

    fn after(&self, ms: u64, job: impl FnOnce() + Send + 'static) {
        let _ = self.tx.send((Instant::now() + Duration::from_millis(ms), Box::new(job)));
    }
}

fn timer_loop(rx: &Receiver<(Instant, Job)>) {
    let mut queue: BinaryHeap<Reverse<(Instant, u64)>> = BinaryHeap::new();
    let mut jobs: HashMap<u64, Job> = HashMap::new();
    let mut seq = 0u64;
    loop {
        let next = queue.peek().map(|Reverse((at, _))| *at);
        let received = match next {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok((at, job)) => {
                seq += 1;
                queue.push(Reverse((at, seq)));
                jobs.insert(seq, job);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if queue.is_empty() {
                    return;
                }
            }
        }
        while let Some(Reverse((at, id))) = queue.peek().copied() {
            if at > Instant::now() {
                break;
            }
            queue.pop();
            if let Some(job) = jobs.remove(&id) {
                job();
            }
        }
    }
}

/// xorshift, for the made-up download speeds and bench timings.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ── The mock itself ─────────────────────────────────────────────────────────

const CONFIG_PATH: &str = "C:\\Users\\Ashton\\AppData\\Roaming\\Flow\\config.toml";

struct State {
    raw: Value,
    installed: BTreeSet<String>,
    /// Providers with a stored key, in the order they were added.
    keys: Vec<String>,
    key_values: HashMap<String, String>,
    downloads: HashMap<String, DownloadProgress>,
    /// Download "intervals" running, by model id, with a generation to stop them.
    timers: HashMap<String, u64>,
    timer_generation: u64,
    history: Vec<HistoryEntry>,
    next_history_id: i64,
    bench_run: u64,
    /// The generation of the hotkey capture in progress, if any.
    capture: Option<u64>,
    capture_generation: u64,
    paused: bool,
    rng: Rng,
}

struct Inner {
    options: MockOptions,
    catalog: Catalog,
    state: Mutex<State>,
    listeners: Mutex<Vec<Listener>>,
    timers: Scheduler,
    theme: ThemeInfo,
}

struct Mock {
    inner: Arc<Inner>,
}

impl Mock {
    fn new(options: MockOptions) -> Self {
        let fresh = options.fresh;
        let catalog = parse_catalog(CATALOG_JSON).unwrap_or_else(|err| {
            log::error!(target: "settings", "bundled catalog does not parse: {err}");
            Catalog {
                version: 1,
                as_of: String::new(),
                providers: vec![],
                local: vec![],
                cloud: vec![],
                cleanup: vec![],
            }
        });
        let mut raw = json!({
            "version": 1,
            "general": { "launch_at_login": true, "theme": "system", "onboarded": !options.onboarding },
            "audio": if options.no_issues || fresh { json!({}) } else { json!({ "max_recording_seconds": 2 }) },
            "model": { "active": "parakeet-tdt-0.6b-v2", "fallback": "parakeet-tdt-0.6b-v2" },
            "history": { "enabled": !options.history_off && !fresh, "retention_days": 30 },
        });
        if !fresh {
            let extra = json!({
                "providers": {
                    "groq": { "api_key": "secret:groq" },
                    "custom-lan": {
                        "base_url": "http://192.168.1.20:8000/v1",
                        "model": "whisper-large-v3",
                        "api_key": "secret:custom-lan",
                    },
                },
                "prompts": {
                    "standup": { "text": "Rewrite as short stand-up notes: what I did, what is next, blockers." },
                },
                "dictionary": [
                    { "heard": ["tory", "towery"], "write": "Tauri" },
                    { "heard": ["pre a", "priya"], "write": "Priya" },
                    { "heard": ["k eight s", "kates"], "write": "k8s" },
                ],
                "snippets": [
                    { "trigger": "my address", "text": "221B Baker Street\nLondon NW1 6XE" },
                    { "trigger": "sign off", "text": "Best regards,\nAshton" },
                ],
                "profiles": [
                    {
                        "name": "Terminals",
                        "match_process": ["WindowsTerminal.exe", "wezterm-gui.exe"],
                        "insert": { "paste_shortcut": "Ctrl+Shift+V" },
                        "cleanup": { "llm_enabled": false },
                    },
                    {
                        "name": "Slack",
                        "match_process": ["slack.exe"],
                        "model": { "active": "groq/whisper-large-v3-turbo" },
                        "cleanup": { "llm_enabled": true, "llm_prompt": "casual" },
                    },
                ],
            });
            if let (Some(target), Value::Object(extra)) = (raw.as_object_mut(), extra) {
                target.extend(extra);
            }
        }

        let system = crate::platform::system_theme();
        let accent = options
            .accent
            .as_deref()
            .filter(|a| a.len() == 6 && a.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|a| format!("#{a}"))
            .unwrap_or_else(|| "#0067c0".into());
        let theme = ThemeInfo {
            dark: options.dark.unwrap_or(system.dark),
            accent,
            high_contrast: options.high_contrast,
            reduced_motion: options.reduced_motion,
        };

        let history = if fresh { Vec::new() } else { sample_history(95) };
        let next_history_id = history.len() as i64 + 1;
        let seed = now_ms() as u64 | 1;
        let state = State {
            raw,
            installed: if fresh {
                BTreeSet::new()
            } else {
                ["parakeet-tdt-0.6b-v2", "moonshine-v2-tiny"].iter().map(|s| s.to_string()).collect()
            },
            keys: if fresh { Vec::new() } else { vec!["groq".into(), "custom-lan".into()] },
            key_values: HashMap::new(),
            downloads: HashMap::new(),
            timers: HashMap::new(),
            timer_generation: 0,
            history,
            next_history_id,
            bench_run: 0,
            capture: None,
            capture_generation: 0,
            paused: false,
            rng: Rng(seed),
        };
        let inner = Arc::new(Inner {
            options: options.clone(),
            catalog,
            state: Mutex::new(state),
            listeners: Mutex::new(Vec::new()),
            timers: Scheduler::start(),
            theme,
        });
        if let Some(id) = &options.downloading {
            start_download(&inner, id, 0.42, 3e6);
        }
        Self { inner }
    }
}

fn emit(inner: &Inner, event: Event) {
    for listener in inner.listeners.lock().iter() {
        listener(event.clone());
    }
}

fn config_snapshot(state: &State) -> ConfigSnapshot {
    let validated = validate_config(&state.raw);
    let issues = validated
        .issues
        .into_iter()
        .enumerate()
        .map(|(i, mut issue)| {
            issue.line = Some(14 + i as u32 * 9);
            issue
        })
        .collect();
    ConfigSnapshot { config: validated.config, issues, path: CONFIG_PATH.into() }
}

fn custom_entries(inner: &Inner, state: &State) -> Vec<ModelEntry> {
    let config = validate_config(&state.raw).config;
    config
        .providers
        .iter()
        .filter(|(id, p)| inner.catalog.provider(id).is_none() && p.base_url.as_deref().is_some_and(|u| !u.is_empty()))
        .map(|(id, p)| {
            ModelEntry::Cloud(CloudModelEntry {
                id: format!("{CUSTOM_PREFIX}{id}"),
                provider: id.clone(),
                model: p.model.clone().unwrap_or_default(),
                name: format!("{id} ({})", p.model.as_deref().unwrap_or("custom")),
                description: p.base_url.clone().unwrap_or_default(),
                price_per_hour: 0.0,
                recommended: false,
                hints: None,
                min_billed_seconds: None,
                docs_url: None,
            })
        })
        .collect()
}

fn catalog_snapshot(inner: &Inner, state: &State) -> CatalogSnapshot {
    let active = validate_config(&state.raw).config.model.active;
    let custom = custom_entries(inner, state);
    let mut snapshot = CatalogSnapshot {
        catalog: inner.catalog.clone(),
        status: Default::default(),
        keys: Default::default(),
        custom: Vec::new(),
    };
    let status = &mut snapshot.status;
    for m in &inner.catalog.local {
        let ready = state.installed.contains(&m.id);
        status.insert(
            m.id.clone(),
            ModelStatus {
                id: m.id.clone(),
                ready,
                active: active == m.id,
                loaded: active == m.id && ready,
                download: state.downloads.get(&m.id).cloned(),
                custom: false,
            },
        );
    }
    for m in &inner.catalog.cloud {
        let keyless = inner.catalog.provider(&m.provider).is_some_and(|p| p.keyless);
        status.insert(
            m.id.clone(),
            ModelStatus {
                id: m.id.clone(),
                ready: state.keys.contains(&m.provider) || keyless,
                active: active == m.id,
                loaded: false,
                download: None,
                custom: false,
            },
        );
    }
    for m in &custom {
        status.insert(
            m.id().to_string(),
            ModelStatus {
                id: m.id().to_string(),
                ready: true,
                active: active == m.id(),
                loaded: false,
                download: None,
                custom: true,
            },
        );
    }
    for id in inner.catalog.providers.iter().map(|p| p.id.clone()).chain(state.keys.iter().cloned()) {
        let has = state.keys.contains(&id);
        snapshot.keys.insert(id, has);
    }
    snapshot.custom = custom;
    snapshot
}

fn emit_models(inner: &Inner) {
    let snapshot = catalog_snapshot(inner, &inner.state.lock());
    emit(inner, Event::ModelsChanged(snapshot));
}

fn stop_download(state: &mut State, id: &str) {
    state.timers.remove(id);
}

fn start_download(inner: &Arc<Inner>, id: &str, from: f64, bytes_per_second: f64) {
    let Some(entry) = inner.catalog.local_entry(id).cloned() else {
        return;
    };
    let generation = {
        let mut state = inner.state.lock();
        if state.timers.contains_key(id) {
            return;
        }
        state.timer_generation += 1;
        let generation = state.timer_generation;
        state.timers.insert(id.to_string(), generation);
        generation
    };
    let total = entry.size_bytes as f64;
    let received = total * from;
    publish(inner, id, received, total, DownloadState::Downloading, 0.0);
    schedule_tick(Arc::clone(inner), id.to_string(), generation, received, total, bytes_per_second);
}

fn publish(inner: &Inner, id: &str, received: f64, total: f64, state_kind: DownloadState, speed: f64) {
    let progress = DownloadProgress {
        id: id.to_string(),
        received_bytes: received.min(total) as u64,
        total_bytes: total as u64,
        speed,
        state: state_kind,
        error: None,
    };
    {
        let mut state = inner.state.lock();
        if matches!(state_kind, DownloadState::Done | DownloadState::Cancelled) {
            state.downloads.remove(id);
        } else {
            state.downloads.insert(id.to_string(), progress.clone());
        }
    }
    emit(inner, Event::ModelsProgress(progress));
}

fn schedule_tick(inner: Arc<Inner>, id: String, generation: u64, received: f64, total: f64, bps: f64) {
    let timers = &inner.timers;
    let next = Arc::clone(&inner);
    timers.after(200, move || {
        let inner = next;
        let speed = {
            let mut state = inner.state.lock();
            if state.timers.get(&id) != Some(&generation) {
                return;
            }
            bps * (0.8 + state.rng.next() * 0.4)
        };
        let received = received + speed * 0.2;
        if received < total {
            publish(&inner, &id, received, total, DownloadState::Downloading, speed);
            schedule_tick(Arc::clone(&inner), id, generation, received, total, bps);
            return;
        }
        stop_download(&mut inner.state.lock(), &id);
        publish(&inner, &id, received, total, DownloadState::Verifying, 0.0);
        let later = Arc::clone(&inner);
        inner.timers.after(900, move || {
            later.state.lock().installed.insert(id.clone());
            publish(&later, &id, total, total, DownloadState::Done, 0.0);
            emit_models(&later);
        });
    });
}

fn info(inner: &Inner, state: &State) -> AppInfo {
    AppInfo {
        version: "0.1.0".into(),
        runtime: "Rust".into(),
        platform: "win32".into(),
        config_path: CONFIG_PATH.into(),
        logs_path: "C:\\Users\\Ashton\\AppData\\Roaming\\Flow\\logs".into(),
        models_path: "C:\\Users\\Ashton\\AppData\\Roaming\\Flow\\models".into(),
        devices: if inner.options.fresh {
            Vec::new()
        } else {
            vec![
                BackendDevice {
                    name: "CPU".into(),
                    kind: "cpu".into(),
                    device_type: "cpu".into(),
                    description: "AMD Ryzen 7 7800X3D".into(),
                    memory_total: 0,
                },
                BackendDevice {
                    name: "CUDA0".into(),
                    kind: "cuda".into(),
                    device_type: "gpu".into(),
                    description: "NVIDIA GeForce RTX 4070".into(),
                    memory_total: 12 * 1024u64.pow(3),
                },
            ]
        },
        degraded: if inner.options.degraded {
            vec![DegradedModule { module: "better-sqlite3".into(), effect: "History is unavailable.".into() }]
        } else {
            Vec::new()
        },
        paused: state.paused,
    }
}

/// Replies after `ms`, like the TS mock's `await delay(ms)`.
fn reply_after(
    inner: &Arc<Inner>,
    ms: u64,
    reply: Box<dyn FnOnce(Reply) + Send>,
    make: impl FnOnce(&Arc<Inner>) -> Reply + Send + 'static,
) {
    let target = Arc::clone(inner);
    inner.timers.after(ms, move || {
        let result = make(&target);
        reply(result);
    });
}

impl Backend for Mock {
    fn subscribe(&self, on_event: Box<dyn Fn(Event) + Send + Sync>) {
        self.inner.listeners.lock().push(on_event);
    }

    fn request(&self, request: Request, reply: Box<dyn FnOnce(Reply) + Send>) {
        let inner = &self.inner;
        match request {
            // ── Config ──
            Request::ConfigGet => reply_after(inner, 20, reply, |i| Reply::ok(&config_snapshot(&i.state.lock()))),
            Request::ConfigSet(patches) => {
                let active_before = {
                    let mut state = inner.state.lock();
                    let before = validate_config(&state.raw).config.model.active;
                    state.raw = apply_patches(&state.raw, &patches);
                    before
                };
                let touches_providers = patches.iter().any(|p| p.path.first() == Some(&PathSeg::from("providers")));
                reply_after(inner, 40, reply, move |i| {
                    let snapshot = config_snapshot(&i.state.lock());
                    emit(i, Event::ConfigChanged(snapshot.clone()));
                    if touches_providers || snapshot.config.model.active != active_before {
                        emit_models(i);
                    }
                    Reply::ok(&snapshot)
                });
            }
            Request::ConfigOpenFile => reply(Reply::ok(&())),
            Request::ConfigExport(_) => reply_after(inner, 150, reply, |_| Reply::ok(&())),
            Request::ConfigImport(_) => {
                reply_after(inner, 150, reply, |i| Reply::ok(&config_snapshot(&i.state.lock())))
            }

            // ── Models ──
            Request::ModelsList => reply_after(inner, 30, reply, |i| Reply::ok(&catalog_snapshot(i, &i.state.lock()))),
            Request::ModelsDownload(id) => {
                start_download(inner, &id, 0.0, 46e6);
                reply(Reply::ok(&()));
            }
            Request::ModelsCancel(id) => {
                let current = {
                    let mut state = inner.state.lock();
                    stop_download(&mut state, &id);
                    state.downloads.remove(&id)
                };
                if let Some(current) = current {
                    emit(
                        inner,
                        Event::ModelsProgress(DownloadProgress {
                            state: DownloadState::Cancelled,
                            speed: 0.0,
                            ..current
                        }),
                    );
                }
                emit_models(inner);
                reply(Reply::ok(&()));
            }
            Request::ModelsRemove(id) => {
                inner.state.lock().installed.remove(&id);
                emit_models(inner);
                reply(Reply::ok(&()));
            }
            Request::ModelsActivate(id) => {
                {
                    let mut state = inner.state.lock();
                    state.raw = apply_patches(&state.raw, &[ConfigPatch::set(path!["model", "active"], id)]);
                }
                reply_after(inner, 40, reply, |i| {
                    let snapshot = config_snapshot(&i.state.lock());
                    emit(i, Event::ConfigChanged(snapshot));
                    emit_models(i);
                    Reply::ok(&())
                });
            }
            Request::ModelsRefresh => {
                reply_after(inner, 300, reply, |i| Reply::ok(&catalog_snapshot(i, &i.state.lock())))
            }

            // ── Secrets ──
            Request::SecretsSet { provider, key } => reply_after(inner, 60, reply, move |i| {
                {
                    let mut state = i.state.lock();
                    if key.is_empty() {
                        state.keys.retain(|k| k != &provider);
                    } else if !state.keys.contains(&provider) {
                        state.keys.push(provider.clone());
                    }
                    state.key_values.insert(provider, key);
                }
                emit_models(i);
                Reply::ok(&())
            }),
            Request::SecretsHas(provider) => reply(Reply::ok(&inner.state.lock().keys.contains(&provider))),
            Request::SecretsDelete(provider) => {
                inner.state.lock().keys.retain(|k| k != &provider);
                emit_models(inner);
                reply(Reply::ok(&()));
            }
            Request::SecretsTest(provider) => reply_after(inner, 700, reply, move |i| {
                let state = i.state.lock();
                let result = if !state.keys.contains(&provider) {
                    KeyTestResult { ok: false, message: "No key is stored.".into() }
                } else if state.key_values.get(&provider).is_some_and(|k| k.contains("bad")) {
                    // Any key containing "bad" fails, to show the error state.
                    KeyTestResult { ok: false, message: "401 Unauthorized: invalid API key.".into() }
                } else {
                    KeyTestResult { ok: true, message: "The key works.".into() }
                };
                Reply::ok(&result)
            }),

            // ── Test bench ──
            Request::BenchRun { audio, model_ids } => {
                let run_id = {
                    let mut state = inner.state.lock();
                    state.bench_run += 1;
                    state.bench_run
                };
                let audio_ms = audio.0.len() as f64 / SAMPLE_RATE as f64 * 1000.0;
                let variants = [
                    "The quick brown fox jumps over the lazy dog, and Priya ships the Tauri build on Friday.",
                    "The quick brown fox jumps over the lazy dog and Priya ships the Tory build on Friday.",
                    "The quick brown fox jumps over the lazy dog, and Pria ships the Tauri build on Friday.",
                ];
                let mut at = 0.0;
                let count = model_ids.len();
                for (i, model_id) in model_ids.into_iter().enumerate() {
                    let cloud = inner.catalog.cloud_entry(&model_id).cloned();
                    let jitter = inner.state.lock().rng.next();
                    at += if cloud.is_some() { 260.0 + jitter * 240.0 } else { 420.0 + jitter * 700.0 };
                    let elapsed_ms = (at / (i as f64 + 1.0) + if cloud.is_some() { 180.0 } else { 320.0 }).round();
                    let result = BenchResult {
                        run_id,
                        model_id,
                        ok: true,
                        text: Some(variants[i % variants.len()].to_string()),
                        elapsed_ms: Some(elapsed_ms),
                        cost: cloud.map(|c| cloud_cost(&c, audio_ms)),
                        error: None,
                        done: i + 1 == count,
                    };
                    let target = Arc::clone(inner);
                    inner.timers.after(at as u64, move || emit(&target, Event::BenchResult(result)));
                }
                reply(Reply::ok(&run_id));
            }

            // ── Hotkey recorder ──
            Request::HotkeysCapture => start_capture(inner, reply),
            Request::HotkeysCaptureCancel => {
                inner.state.lock().capture = None;
                reply(Reply::ok(&()));
            }

            // ── History ──
            Request::HistorySearch { query, limit, offset } => reply_after(inner, 60, reply, move |i| {
                let state = i.state.lock();
                let needle = query.to_lowercase();
                let matches: Vec<&HistoryEntry> = state
                    .history
                    .iter()
                    .filter(|e| needle.is_empty() || e.text.to_lowercase().contains(&needle))
                    .collect();
                let page: Vec<HistoryEntry> =
                    matches.into_iter().skip(offset as usize).take(limit as usize).cloned().collect();
                Reply::ok(&page)
            }),
            Request::HistoryDelete(id) => {
                inner.state.lock().history.retain(|e| e.id != id);
                reply(Reply::ok(&()));
            }
            Request::HistoryClear => {
                inner.state.lock().history.clear();
                reply(Reply::ok(&()));
            }
            Request::HistoryRerun(id) => reply_after(inner, 500, reply, move |i| {
                let mut state = i.state.lock();
                let Some(entry) = state.history.iter_mut().find(|e| e.id == id) else {
                    return Reply::ok(&Option::<HistoryEntry>::None);
                };
                entry.text = entry.text.split_whitespace().collect::<Vec<_>>().join(" ");
                Reply::ok(&Some(entry.clone()))
            }),

            // ── Dictation ──
            Request::DictationTest(audio) => reply_after(inner, 650, reply, move |i| {
                let audio_ms = audio.0.len() as f64 / SAMPLE_RATE as f64 * 1000.0;
                let mut state = i.state.lock();
                let model = validate_config(&state.raw).config.model.active;
                let text = "This is a test of Flow. It heard me the first time.";
                let id = state.next_history_id;
                state.next_history_id += 1;
                state.history.insert(
                    0,
                    HistoryEntry {
                        id,
                        created_at: now_ms(),
                        text: text.into(),
                        raw_text: text.into(),
                        model: model.clone(),
                        app: "Flow".into(),
                        audio_ms: audio_ms as i64,
                        elapsed_ms: 640,
                    },
                );
                Reply::ok(&Transcript { text: text.into(), model, audio_ms, elapsed_ms: 640.0 })
            }),

            // ── App ──
            Request::AppInfo => {
                let result = info(inner, &inner.state.lock());
                reply(Reply::ok(&result));
            }
            Request::AppOpenLogs | Request::AppOpenModels => reply(Reply::ok(&())),
            Request::AppOpenExternal(url) => {
                log::info!(target: "settings", "[mock] openExternal {url}");
                reply(Reply::ok(&()));
            }
            Request::AppCopy(text) => {
                crate::platform::clipboard::set_text(&text);
                reply(Reply::ok(&()));
            }
            Request::AppSetPaused(next) => {
                inner.state.lock().paused = next;
                reply(Reply::ok(&()));
            }
            Request::AppCheckUpdates => {
                reply_after(inner, 900, reply, |_| Reply::ok(&"Flow 0.1.0 is the latest version."))
            }
            Request::ThemeGet => reply(Reply::ok(&inner.theme)),
        }
    }
}

/// The TS mock listens to the page's key events. Here the keyboard is read
/// with `GetAsyncKeyState` while a capture runs, so Win and the mouse side
/// buttons work like they do with the real hook.
fn start_capture(inner: &Arc<Inner>, reply: Box<dyn FnOnce(Reply) + Send>) {
    let generation = {
        let mut state = inner.state.lock();
        state.capture_generation += 1;
        state.capture = Some(state.capture_generation);
        state.capture_generation
    };
    let target = Arc::clone(inner);
    let spawned = std::thread::Builder::new().name("settings-mock-capture".into()).spawn(move || {
        let mut down: BTreeSet<&'static str> = BTreeSet::new();
        let mut chord: Vec<&'static str> = Vec::new();
        // Keys already held when recording starts (the click on Record) are ignored until released.
        let mut ignored: BTreeSet<&'static str> = crate::platform::keys_down().into_iter().collect();
        loop {
            std::thread::sleep(Duration::from_millis(12));
            if target.state.lock().capture != Some(generation) {
                // Cancelled, or another capture took over: resolves null, like the TS mock.
                reply(Reply::ok(&Option::<String>::None));
                return;
            }
            let now: BTreeSet<&'static str> = crate::platform::keys_down().into_iter().collect();
            ignored.retain(|k| now.contains(k));
            let mut changed = false;
            for key in now.iter().filter(|k| !ignored.contains(*k)) {
                if down.insert(key) {
                    if !chord.contains(key) {
                        chord.push(key);
                    }
                    changed = true;
                }
            }
            if changed {
                emit(
                    &target,
                    Event::HotkeysCaptureProgress(format_binding(&binding_from_keys(chord.iter().copied(), true))),
                );
            }
            down.retain(|k| now.contains(k));
            if down.is_empty() && !chord.is_empty() {
                target.state.lock().capture = None;
                let result = format_binding(&binding_from_keys(chord.iter().copied(), true));
                reply(Reply::ok(&Some(result)));
                return;
            }
        }
    });
    if spawned.is_err() {
        // The reply was dropped with the closure; the caller sees an error.
        inner.state.lock().capture = None;
    }
}
