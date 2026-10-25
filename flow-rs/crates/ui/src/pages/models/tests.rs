//! Behaviour tests for the Models page: real egui passes against a
//! synchronous backend with the bundled catalog. Widgets are found by the
//! text they paint and clicked with the simulated pointer.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::epaint::{ClippedShape, Shape};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use flow_core::catalog::{
    Catalog, CatalogSnapshot, DownloadProgress, DownloadState, ModelEntry, ModelStatus, parse_catalog,
};
use flow_core::config::value::apply_patches;
use flow_core::config::{ConfigSnapshot, validate_config};
use flow_core::protocol::settings::{BenchResult, Event as BackendEvent, KeyTestResult, Reply, Request};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::Models;
use crate::backend::{Backend, Link};
use crate::pages::{Cx, Page, PageId};
use crate::stores::{AppInfoStore, CatalogStore, ConfigStore, Microphones};
use crate::testing::Harness;

const CATALOG_JSON: &[u8] = include_bytes!("../../../../../../resources/catalog.json");

/// Answers at once, like `testing::Instant`, with a catalog: two local models
/// installed and a Groq key stored. Records every request.
struct Fake {
    raw: Mutex<Value>,
    catalog: Catalog,
    installed: BTreeSet<String>,
    keys: Mutex<Vec<String>>,
    key_test: Mutex<KeyTestResult>,
    requests: Mutex<Vec<Request>>,
}

impl Fake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            raw: Mutex::new(
                json!({ "general": { "onboarded": true }, "providers": { "groq": { "api_key": "secret:groq" } } }),
            ),
            catalog: parse_catalog(CATALOG_JSON).expect("bundled catalog"),
            installed: ["parakeet-tdt-0.6b-v2", "moonshine-v2-tiny"].iter().map(|s| s.to_string()).collect(),
            keys: Mutex::new(vec!["groq".into()]),
            key_test: Mutex::new(KeyTestResult { ok: true, message: String::new() }),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn config(&self) -> ConfigSnapshot {
        let validated = validate_config(&self.raw.lock());
        ConfigSnapshot { config: validated.config, issues: vec![], path: "config.toml".into() }
    }

    fn snapshot(&self) -> CatalogSnapshot {
        let active = self.config().config.model.active;
        let keys = self.keys.lock().clone();
        let mut snapshot = CatalogSnapshot {
            catalog: self.catalog.clone(),
            status: Default::default(),
            keys: Default::default(),
            custom: Vec::<ModelEntry>::new(),
        };
        let status = &mut snapshot.status;
        for m in &self.catalog.local {
            let ready = self.installed.contains(&m.id);
            let s = ModelStatus {
                id: m.id.clone(),
                ready,
                active: active == m.id,
                loaded: false,
                download: None,
                custom: false,
            };
            status.insert(m.id.clone(), s);
        }
        for m in &self.catalog.cloud {
            let ready = keys.contains(&m.provider);
            let s = ModelStatus {
                id: m.id.clone(),
                ready,
                active: active == m.id,
                loaded: false,
                download: None,
                custom: false,
            };
            status.insert(m.id.clone(), s);
        }
        snapshot.keys = self.catalog.providers.iter().map(|p| (p.id.clone(), keys.contains(&p.id))).collect();
        snapshot
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().clone()
    }
}

impl Backend for Fake {
    fn request(&self, request: Request, reply: Box<dyn FnOnce(Reply) + Send>) {
        self.requests.lock().push(request.clone());
        let answer = match request {
            Request::ConfigGet => Reply::ok(&self.config()),
            Request::ConfigSet(patches) => {
                let next = apply_patches(&self.raw.lock(), &patches);
                *self.raw.lock() = next;
                Reply::ok(&self.config())
            }
            Request::ModelsList => Reply::ok(&self.snapshot()),
            Request::AppInfo => Reply::ok(&flow_core::types::AppInfo::default()),
            Request::SecretsSet { provider, .. } => {
                self.keys.lock().push(provider);
                Reply::ok(&())
            }
            Request::SecretsTest(_) => Reply::ok(&*self.key_test.lock()),
            Request::BenchRun { .. } => Reply::ok(&7u64),
            _ => Reply::ok(&()),
        };
        reply(answer);
    }
    fn subscribe(&self, _on_event: Box<dyn Fn(BackendEvent) + Send + Sync>) {}
}

/// The Models page in a tall 696 px column, with the text each frame painted.
struct ModelsHarness {
    h: Harness,
    backend: Arc<Fake>,
    link: Link,
    config: ConfigStore,
    catalog: CatalogStore,
    app_info: AppInfoStore,
    mics: Microphones,
    page: Models,
    input: Vec<Event>,
    /// Backend events delivered on the next frame.
    events: Vec<BackendEvent>,
    texts: Vec<(String, Rect)>,
}

fn collect(shape: &Shape, out: &mut Vec<(String, Rect)>) {
    match shape {
        Shape::Text(text) => {
            out.push((text.galley.text().to_string(), Rect::from_min_size(text.pos, text.galley.size())))
        }
        Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
        _ => {}
    }
}

impl ModelsHarness {
    fn new() -> Self {
        let h = Harness::new();
        let backend = Fake::new();
        let link = Link::new(backend.clone(), h.ctx.clone());
        let mut harness = Self {
            config: ConfigStore::new(link.clone()),
            catalog: CatalogStore::new(link.clone()),
            app_info: AppInfoStore::new(link.clone()),
            mics: Microphones::new(),
            page: Models::default(),
            link,
            backend,
            h,
            input: Vec::new(),
            events: Vec::new(),
            texts: Vec::new(),
        };
        for _ in 0..4 {
            harness.frame();
        }
        harness
    }

    fn frame(&mut self) {
        self.h.time += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 5000.0))),
            time: Some(self.h.time),
            focused: true,
            events: std::mem::take(&mut self.input),
            ..RawInput::default()
        };
        let events = std::mem::take(&mut self.events);
        let Self { h, config, catalog, app_info, mics, page, link, .. } = self;
        let mut nav: Option<PageId> = None;
        let mut output = h.ctx.run_ui(input, |ui| {
            config.pump(&events);
            catalog.pump(&events);
            app_info.pump(false);
            if !config.loaded() {
                return;
            }
            let column = Rect::from_min_size(pos2(0.0, 0.0), vec2(696.0, 5000.0));
            let mut col = ui.new_child(egui::UiBuilder::new().max_rect(column));
            let mut cx = Cx { config, catalog, app_info, mics, link, events: &events, nav: &mut nav };
            page.ui(&mut col, &mut cx);
        });
        output.textures_delta.clear();
        self.texts.clear();
        for ClippedShape { shape, .. } in &output.shapes {
            collect(shape, &mut self.texts);
        }
    }

    /// The rect of the `nth` painted text equal to `text`.
    fn find(&self, text: &str, nth: usize) -> Option<Rect> {
        self.texts.iter().filter(|(t, _)| t == text).nth(nth).map(|(_, r)| *r)
    }

    fn has(&self, text: &str) -> bool {
        self.find(text, 0).is_some()
    }

    fn click_at(&mut self, pos: Pos2) {
        self.input.push(Event::PointerMoved(pos));
        self.input.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.frame();
        self.input.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        self.frame();
        self.frame();
    }

    fn click(&mut self, text: &str, nth: usize) {
        let rect = self.find(text, nth).unwrap_or_else(|| panic!("no {text:?} on the page"));
        self.click_at(rect.center());
    }

    fn type_text(&mut self, text: &str) {
        self.input.push(Event::Text(text.into()));
        self.frame();
        self.frame();
    }

    fn key(&mut self, key: Key) {
        for pressed in [true, false] {
            self.input.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        }
        self.frame();
        self.frame();
    }

    fn deliver(&mut self, event: BackendEvent) {
        self.events.push(event);
        self.frame();
        self.frame();
    }

    fn sent(&self, request: &Request) -> bool {
        self.backend.requests().contains(request)
    }
}

#[test]
fn downloads_and_cancels_a_local_model() {
    let mut p = ModelsHarness::new();
    assert!(p.has("Parakeet TDT 0.6B v2") && p.has("Active"));
    let lists = p.backend.requests().iter().filter(|r| **r == Request::ModelsList).count();
    // Whisper large-v3-turbo is the first model that is not installed.
    p.click("Download", 0);
    assert!(p.sent(&Request::ModelsDownload("whisper-large-v3-turbo".into())));
    let after = p.backend.requests().iter().filter(|r| **r == Request::ModelsList).count();
    assert_eq!(after, lists + 1, "the catalog is re-read after the call");

    p.deliver(BackendEvent::ModelsProgress(DownloadProgress {
        id: "whisper-large-v3-turbo".into(),
        received_bytes: 400_000_000,
        total_bytes: 1_600_000_000,
        speed: 2_500_000.0,
        state: DownloadState::Downloading,
        error: None,
    }));
    assert!(p.has("25%, 2.5 MB/s"));
    assert!(p.has("400 MB of 1.6 GB"));
    p.click("Cancel", 0);
    assert!(p.sent(&Request::ModelsCancel("whisper-large-v3-turbo".into())));
}

#[test]
fn a_failed_download_offers_retry() {
    let mut p = ModelsHarness::new();
    p.deliver(BackendEvent::ModelsProgress(DownloadProgress {
        id: "whisper-small-en".into(),
        received_bytes: 10,
        total_bytes: 100,
        speed: 0.0,
        state: DownloadState::Error,
        error: Some("Not enough disk space".into()),
    }));
    assert!(p.has("The download failed: Not enough disk space Retry picks up where it stopped."));
    p.click("Retry", 0);
    assert!(p.sent(&Request::ModelsDownload("whisper-small-en".into())));
    assert!(!p.has("Retry"), "retrying clears the old error");
}

#[test]
fn removing_a_model_asks_first() {
    let mut p = ModelsHarness::new();
    // The trash button sits 4 px right of "Set active" (12 px padding), 32 px wide.
    let set_active = p.find("Set active", 0).expect("Moonshine v2 Tiny is installed");
    p.click_at(pos2(set_active.right() + 12.0 + 4.0 + 16.0, set_active.center().y));
    assert!(p.has("Remove Moonshine v2 Tiny?"));
    assert!(!p.backend.requests().iter().any(|r| matches!(r, Request::ModelsRemove(_))));
    p.click("Cancel", 0);
    assert!(!p.has("Remove Moonshine v2 Tiny?"));
    assert!(!p.backend.requests().iter().any(|r| matches!(r, Request::ModelsRemove(_))));

    p.click_at(pos2(set_active.right() + 12.0 + 4.0 + 16.0, set_active.center().y));
    p.click("Remove model", 0);
    assert!(p.sent(&Request::ModelsRemove("moonshine-v2-tiny".into())));
    assert!(!p.has("Remove Moonshine v2 Tiny?"));
}

#[test]
fn set_active_activates_a_model() {
    let mut p = ModelsHarness::new();
    p.click("Set active", 0);
    assert!(p.sent(&Request::ModelsActivate("moonshine-v2-tiny".into())));
}

#[test]
fn a_key_is_saved_then_tested() {
    let mut p = ModelsHarness::new();
    // OpenAI has no key: its card offers Connect, its models cannot be activated.
    assert!(p.has("Add your OpenAI key to use these models."));
    p.click("Connect", 0);
    assert!(p.has("OpenAI API key. It is encrypted on this PC and never written to the config file."));
    p.click("Paste the key", 0);
    p.type_text("  sk-test  ");
    p.key(Key::Enter);
    assert!(p.sent(&Request::SecretsSet { provider: "openai".into(), key: "sk-test".into() }));
    assert!(p.sent(&Request::SecretsTest("openai".into())));
    assert!(p.has("The key works."));
    assert!(!p.has("Paste the key"), "the form closes");
}

#[test]
fn a_failing_key_test_is_reported() {
    let mut p = ModelsHarness::new();
    *p.backend.key_test.lock() = KeyTestResult { ok: false, message: "401 Unauthorized".into() };
    p.click("Test key", 0);
    assert!(p.sent(&Request::SecretsTest("groq".into())));
    assert!(p.has("The key was saved, but the test failed: 401 Unauthorized"));
    assert!(p.has("Key not working"));
}

#[test]
fn removing_a_key_asks_first() {
    let mut p = ModelsHarness::new();
    p.click("Remove key", 0);
    assert!(p.has("Remove the Groq key?"));
    assert!(!p.sent(&Request::SecretsDelete("groq".into())));
    p.click("Remove key", 1);
    assert!(p.sent(&Request::SecretsDelete("groq".into())));
}

#[test]
fn a_custom_endpoint_is_validated_then_saved() {
    let mut p = ModelsHarness::new();
    assert!(p.has("No custom endpoints yet."));
    p.click("Add endpoint", 0);
    assert!(p.has("A short id for this server."));
    // The header button is gone; this is the form's submit.
    p.click("Add endpoint", 0);
    assert!(p.has("Give the endpoint a short name."));

    p.click("Name", 0);
    p.type_text("Groq");
    p.click("Add endpoint", 0);
    assert!(p.has("\u{201c}groq\u{201d} is already in use."));

    p.click("Name", 0);
    p.key(Key::End);
    p.key(Key::Backspace);
    p.type_text(" box");
    let hint = p.texts.iter().find(|(t, _)| t.starts_with("Saved as")).map(|(t, _)| t.clone());
    assert_eq!(hint.as_deref(), Some("Saved as \u{201c}gro-box\u{201d} in the config file."));
    p.click("Base URL", 0);
    p.type_text("192.168.1.20:8000/v1");
    p.click("Add endpoint", 0);
    assert!(p.has("Enter a full URL, such as http://192.168.1.20:8000/v1."));
    p.click("Base URL", 0);
    p.key(Key::End);
    for _ in 0.."192.168.1.20:8000/v1".len() {
        p.key(Key::Backspace);
    }
    p.type_text("http://192.168.1.20:8000/v1//");
    p.click("Add endpoint", 0);
    assert!(p.has("Enter the model name the server expects."));
    assert!(!p.backend.requests().iter().any(|r| matches!(r, Request::ConfigSet(_))));

    p.click("Model name", 0);
    p.type_text("whisper-1");
    p.click("API key (optional)", 0);
    p.type_text("secret-value");
    p.click("Add endpoint", 0);
    assert!(p.sent(&Request::SecretsSet { provider: "gro-box".into(), key: "secret-value".into() }));
    assert_eq!(
        p.backend.raw.lock()["providers"]["gro-box"],
        json!({ "base_url": "http://192.168.1.20:8000/v1", "model": "whisper-1", "api_key": "secret:gro-box" })
    );
    assert!(p.has("gro-box"), "the endpoint is listed");
    assert!(!p.has("A short id for this server."), "the form closed");
}

#[test]
fn removing_the_active_endpoint_falls_back_first() {
    let mut p = ModelsHarness::new();
    *p.backend.raw.lock() = json!({
        "general": { "onboarded": true },
        "model": { "active": "custom/lan", "fallback": "moonshine-v2-tiny" },
        "providers": { "lan": { "base_url": "http://10.0.0.2/v1", "model": "w", "api_key": "secret:lan" } },
    });
    p.backend.keys.lock().push("lan".into());
    let snapshot = p.backend.config();
    p.deliver(BackendEvent::ConfigChanged(snapshot));
    let mut catalog = p.backend.snapshot();
    catalog.keys.insert("lan".into(), true);
    p.deliver(BackendEvent::ModelsChanged(catalog));
    assert!(p.has("lan") && p.has("Key stored"));
    // The endpoint is active, so its row shows the badge and only the trash button.
    let url = p.find("http://10.0.0.2/v1", 0).expect("the endpoint row");
    p.click_at(pos2(696.0 - 16.0 - 16.0, url.top() - 1.0));
    assert!(p.has(
        "This endpoint is the active model. Flow switches to your fallback model and forgets the endpoint and its key."
    ));
    p.click("Remove endpoint", 0);
    let requests = p.backend.requests();
    let activate = requests.iter().position(|r| *r == Request::ModelsActivate("moonshine-v2-tiny".into()));
    let delete = requests.iter().position(|r| *r == Request::SecretsDelete("lan".into()));
    let patch = requests.iter().position(|r| matches!(r, Request::ConfigSet(p) if p[0].value.is_none()));
    assert!(activate < patch && patch < delete && activate.is_some(), "{requests:?}");
}

#[test]
fn the_bench_runs_the_ticked_models_and_shows_results() {
    let mut p = ModelsHarness::new();
    assert!(p.has("Record a phrase first."));
    p.page.bench.set_clip(vec![0.0; 16_000]);
    p.frame();
    assert!(p.has("Recorded 1.0 s of audio. It stays in memory and is never saved."));
    // Local models are ticked; the Groq models are opt-in.
    assert!(p.has("Runs the recording through 2 models."));
    p.click("Run test", 0);
    let run = p.backend.requests().into_iter().find_map(|r| match r {
        Request::BenchRun { model_ids, audio } => Some((model_ids, audio.0.len())),
        _ => None,
    });
    assert_eq!(run, Some((vec!["parakeet-tdt-0.6b-v2".to_string(), "moonshine-v2-tiny".to_string()], 16_000)));
    assert!(p.has("Running…"));
    assert_eq!(p.texts.iter().filter(|(t, _)| t == "Waiting…").count(), 2);

    let result = |model: &str, run_id, done| BenchResult {
        run_id,
        model_id: model.into(),
        ok: true,
        text: Some(format!("heard by {model}")),
        elapsed_ms: Some(1234.0),
        cost: None,
        error: None,
        done,
    };
    p.deliver(BackendEvent::BenchResult(result("parakeet-tdt-0.6b-v2", 3, true)));
    assert!(!p.has("heard by parakeet-tdt-0.6b-v2"), "results of another run are dropped");
    p.deliver(BackendEvent::BenchResult(result("parakeet-tdt-0.6b-v2", 7, false)));
    assert!(p.has("heard by parakeet-tdt-0.6b-v2") && p.has("1,234 ms") && p.has("Free"));
    assert!(p.has("Running…"));
    p.deliver(BackendEvent::BenchResult(BenchResult {
        ok: false,
        text: None,
        error: Some("Model failed to load".into()),
        ..result("moonshine-v2-tiny", 7, true)
    }));
    assert!(p.has("Model failed to load"));
    assert!(p.has("Run test"), "the run is over");
}

#[test]
fn a_too_short_clip_cannot_be_run() {
    let mut p = ModelsHarness::new();
    p.page.bench.set_clip(vec![0.0; 1600]);
    p.frame();
    assert!(p.has("That was too short. Record a full sentence."));
    p.click("Run test", 0);
    assert!(!p.backend.requests().iter().any(|r| matches!(r, Request::BenchRun { .. })));
}

#[test]
fn unticking_every_model_disables_the_run() {
    let mut p = ModelsHarness::new();
    p.page.bench.set_clip(vec![0.0; 16_000]);
    p.frame();
    // The bench's labels are the last of each name on the page.
    let last = |p: &ModelsHarness, name: &str| p.texts.iter().filter(|(t, _)| t == name).count() - 1;
    let n = last(&p, "Parakeet TDT 0.6B v2");
    p.click("Parakeet TDT 0.6B v2", n);
    let n = last(&p, "Moonshine v2 Tiny");
    p.click("Moonshine v2 Tiny", n);
    assert!(p.has("Tick at least one model."));
    p.click("Groq: Whisper large-v3", 0);
    assert!(p.has("Runs the recording through the ticked model."));
}

#[test]
fn the_memory_select_writes_whole_minutes() {
    let mut p = ModelsHarness::new();
    p.config.set(flow_core::path!["model", "keep_loaded_minutes"], 15);
    p.frame();
    p.frame();
    assert!(p.has("15 minutes"), "a value from the file is listed");
    assert_eq!(super::minutes_value(30.0), json!(30));
    assert_eq!(super::minutes_value(2.5), json!(2.5));
}

#[test]
fn announces_download_milestones() {
    let mut a = super::Announcer::default();
    let p = |received, state| {
        let progress = DownloadProgress {
            id: "parakeet-tdt-0.6b-v2".into(),
            received_bytes: received,
            total_bytes: 100,
            speed: 0.0,
            state,
            error: None,
        };
        [(progress.id.clone(), progress)].into_iter().collect::<std::collections::HashMap<_, _>>()
    };
    a.update(None, &p(30, DownloadState::Downloading));
    assert_eq!(a.message, "parakeet-tdt-0.6b-v2: 25% downloaded.");
    a.message.clear();
    a.update(None, &p(40, DownloadState::Downloading));
    assert_eq!(a.message, "", "no repeat within a step");
    a.update(None, &p(100, DownloadState::Verifying));
    assert_eq!(a.message, "parakeet-tdt-0.6b-v2: verifying the download.");
}
