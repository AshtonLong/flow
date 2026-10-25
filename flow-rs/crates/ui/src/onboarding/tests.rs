//! Behaviour tests for the first-run flow: real egui passes with simulated
//! clicks against an in-memory backend that has the bundled catalog.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::epaint::{ClippedShape, Shape};
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use flow_core::catalog::{CatalogSnapshot, ModelStatus, parse_catalog};
use flow_core::config::value::apply_patches;
use flow_core::config::{ConfigPatch, ConfigSnapshot, validate_config};
use flow_core::path;
use flow_core::protocol::settings::{Event as BackendEvent, KeyTestResult, Reply, Request};
use flow_core::types::{DictationEvent, Transcript};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::Onboarding;
use super::model::Choice;
use crate::backend::{Backend, Link};
use crate::pages::{Cx, Page, PageId};
use crate::stores::{AppInfoStore, CatalogStore, ConfigStore, Microphones};
use crate::testing::Harness;

const CATALOG_JSON: &[u8] = include_bytes!("../../../../../resources/catalog.json");

/// Answers at once: config in memory, the bundled catalog with `installed`
/// local models and stored `keys`. Records every request.
struct Backend1 {
    raw: Mutex<Value>,
    installed: Mutex<BTreeSet<String>>,
    keys: Mutex<BTreeSet<String>>,
    requests: Mutex<Vec<Request>>,
}

impl Backend1 {
    fn snapshot(&self) -> ConfigSnapshot {
        let validated = validate_config(&self.raw.lock());
        ConfigSnapshot { config: validated.config, issues: validated.issues, path: "config.toml".into() }
    }

    fn catalog(&self) -> CatalogSnapshot {
        let catalog = parse_catalog(CATALOG_JSON).expect("the bundled catalog parses");
        let active = self.snapshot().config.model.active;
        let mut snapshot =
            CatalogSnapshot { catalog, status: Default::default(), keys: Default::default(), custom: Vec::new() };
        for m in &snapshot.catalog.local {
            let ready = self.installed.lock().contains(&m.id);
            let status = ModelStatus {
                id: m.id.clone(),
                ready,
                active: active == m.id,
                loaded: false,
                download: None,
                custom: false,
            };
            snapshot.status.insert(m.id.clone(), status);
        }
        for p in &snapshot.catalog.providers {
            snapshot.keys.insert(p.id.clone(), self.keys.lock().contains(&p.id));
        }
        snapshot
    }

    fn sent(&self, f: impl Fn(&Request) -> bool) -> usize {
        self.requests.lock().iter().filter(|r| f(r)).count()
    }
}

impl Backend for Backend1 {
    fn request(&self, request: Request, reply: Box<dyn FnOnce(Reply) + Send>) {
        self.requests.lock().push(request.clone());
        let answer = match request {
            Request::ConfigGet => Reply::ok(&self.snapshot()),
            Request::ConfigSet(patches) => {
                let next = apply_patches(&self.raw.lock(), &patches);
                *self.raw.lock() = next;
                Reply::ok(&self.snapshot())
            }
            Request::ModelsList => Reply::ok(&self.catalog()),
            Request::ModelsActivate(id) => {
                let next = apply_patches(&self.raw.lock(), &[ConfigPatch::set(path!["model", "active"], id)]);
                *self.raw.lock() = next;
                Reply::ok(&())
            }
            Request::SecretsSet { provider, .. } => {
                self.keys.lock().insert(provider);
                Reply::ok(&())
            }
            Request::SecretsTest(_) => Reply::ok(&KeyTestResult { ok: false, message: "401 Unauthorized".into() }),
            Request::DictationTest(_) => Reply::ok(&Transcript {
                text: "hello".into(),
                model: "parakeet-tdt-0.6b-v2".into(),
                audio_ms: 1000.0,
                elapsed_ms: 640.0,
            }),
            Request::AppInfo => Reply::ok(&flow_core::types::AppInfo::default()),
            Request::ThemeGet => Reply::ok(&flow_core::types::ThemeInfo::default()),
            _ => Reply::ok(&()),
        };
        reply(answer);
    }
    fn subscribe(&self, _on_event: Box<dyn Fn(BackendEvent) + Send + Sync>) {}
}

/// The onboarding drawn below a 40 px title strip in a 1000×700 window, like the shell.
struct Flow {
    h: Harness,
    pending: Vec<Event>,
    backend: Arc<Backend1>,
    link: Link,
    config: ConfigStore,
    catalog: CatalogStore,
    app_info: AppInfoStore,
    mics: Microphones,
    page: Onboarding,
    /// Shapes of the last frame, to find text on screen.
    shapes: Vec<ClippedShape>,
}

impl Flow {
    fn new(page: Onboarding, installed: &[&str], keys: &[&str]) -> Self {
        let h = Harness::new();
        let backend = Arc::new(Backend1 {
            raw: Mutex::new(json!({ "general": { "onboarded": false } })),
            installed: Mutex::new(installed.iter().map(|s| s.to_string()).collect()),
            keys: Mutex::new(keys.iter().map(|s| s.to_string()).collect()),
            requests: Mutex::new(Vec::new()),
        });
        let link = Link::new(backend.clone(), h.ctx.clone());
        let mut flow = Self {
            config: ConfigStore::new(link.clone()),
            catalog: CatalogStore::new(link.clone()),
            app_info: AppInfoStore::new(link.clone()),
            mics: Microphones::new(),
            page,
            link,
            backend,
            h,
            pending: Vec::new(),
            shapes: Vec::new(),
        };
        for _ in 0..3 {
            flow.frame(&[]);
        }
        flow
    }

    fn frame(&mut self, events: &[BackendEvent]) {
        self.h.time += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 700.0))),
            time: Some(self.h.time),
            focused: true,
            events: std::mem::take(&mut self.pending),
            ..RawInput::default()
        };
        let Self { config, catalog, app_info, mics, page, link, h, .. } = self;
        let mut nav: Option<PageId> = None;
        let mut output = h.ctx.run_ui(input, |ui| {
            config.pump(events);
            catalog.pump(events);
            app_info.pump(false);
            if !config.loaded() {
                return;
            }
            let rect = Rect::from_min_max(pos2(0.0, 40.0), pos2(1000.0, 700.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            let mut cx = Cx { config, catalog, app_info, mics, link, events, nav: &mut nav };
            page.ui(&mut child, &mut cx);
        });
        output.textures_delta.clear();
        self.shapes = output.shapes;
    }

    /// Where `text` was drawn last frame (a whole galley, or one that contains it).
    fn find(&self, text: &str) -> Option<Rect> {
        fn walk(shape: &Shape, text: &str) -> Option<Rect> {
            match shape {
                Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect()),
                Shape::Vec(shapes) => shapes.iter().find_map(|s| walk(s, text)),
                _ => None,
            }
        }
        self.shapes.iter().find_map(|c| walk(&c.shape, text))
    }

    fn click_text(&mut self, text: &str) {
        let rect = self.find(text).unwrap_or_else(|| panic!("“{text}” is not on screen"));
        let pos = rect.center();
        self.pending.push(Event::PointerMoved(pos));
        self.pending.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.frame(&[]);
        self.pending.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        self.frame(&[]);
        self.frame(&[]);
    }

    fn onboarded(&self) -> bool {
        self.backend.raw.lock()["general"]["onboarded"] == json!(true)
    }
}

#[test]
fn next_and_back_walk_the_steps_and_finish_sets_onboarded() {
    let mut f = Flow::new(Onboarding::with_demo(Some("hotkey")), &["parakeet-tdt-0.6b-v2"], &[]);
    assert!(f.find("Set your hotkey").is_some());
    f.click_text("Back");
    assert_eq!(f.page.step(), 0);
    assert!(f.find("Pick your microphone").is_some());
    assert!(f.find("Back").is_none(), "the first step has no Back");
    f.click_text("Next");
    f.click_text("Next");
    assert_eq!(f.page.step(), 2);
    assert!(f.find("Choose a model").is_some());
    assert!(!f.onboarded());
    f.click_text("Finish");
    assert!(f.onboarded(), "Finish writes general.onboarded = true");
}

#[test]
fn skip_setup_finishes_from_any_step() {
    let mut f = Flow::new(Onboarding::with_demo(Some("model")), &[], &[]);
    f.click_text("Skip setup");
    assert!(f.onboarded());
}

#[test]
fn choosing_the_local_model_downloads_it() {
    let mut f = Flow::new(Onboarding::with_demo(Some("model")), &[], &[]);
    assert!(f.find("Free and offline. Your audio never leaves this PC. A one-time 475 MB download.").is_some());
    f.click_text("Download 475 MB");
    assert_eq!(f.backend.sent(|r| matches!(r, Request::ModelsDownload(id) if id == "parakeet-tdt-0.6b-v2")), 1);
    // No dictation test until the model is installed.
    assert!(f.find("Record a test instead").is_none());
}

#[test]
fn a_download_shows_progress_and_can_be_cancelled() {
    let mut f = Flow::new(Onboarding::with_demo(Some("model")), &[], &[]);
    let progress = flow_core::catalog::DownloadProgress {
        id: "parakeet-tdt-0.6b-v2".into(),
        received_bytes: 237_745_920,
        total_bytes: 475_491_840,
        speed: 3_100_000.0,
        state: flow_core::catalog::DownloadState::Downloading,
        error: None,
    };
    f.frame(&[BackendEvent::ModelsProgress(progress.clone())]);
    f.frame(&[]);
    assert!(f.find("50% of 475 MB, 3.1 MB/s").is_some());
    f.click_text("Cancel");
    assert_eq!(f.backend.sent(|r| matches!(r, Request::ModelsCancel(_))), 1);

    let failed = flow_core::catalog::DownloadProgress {
        state: flow_core::catalog::DownloadState::Error,
        error: Some("disk full".into()),
        ..progress
    };
    f.frame(&[BackendEvent::ModelsProgress(failed)]);
    f.frame(&[]);
    assert!(f.find("Retry download").is_some());
    assert!(f.find("The download failed: disk full").is_some());
}

#[test]
fn a_ready_choice_becomes_active_and_shows_the_test() {
    // Groq has a key: picking the cloud choice activates it at once.
    let mut f = Flow::new(Onboarding::with_demo(Some("model")), &["parakeet-tdt-0.6b-v2"], &["groq"]);
    assert!(f.find("Record a test instead").is_some(), "the installed local model can be tried");
    f.click_text("In the cloud: Groq Whisper large-v3-turbo");
    let super::Body::Model(step) = &f.page.body else { panic!("model step") };
    assert_eq!(step.choice(), Choice::Cloud);
    assert_eq!(f.backend.sent(|r| matches!(r, Request::ModelsActivate(id) if id == "groq/whisper-large-v3-turbo")), 1);
    assert!(f.find("Groq connected").is_some());
}

#[test]
fn the_cloud_key_form_stores_and_tests_the_key() {
    let mut f = Flow::new(Onboarding::with_demo(Some("cloud")), &[], &[]);
    assert!(f.find("Groq API key. It is encrypted on this PC and never written to the config file.").is_some());
    let input = f.find("Paste the key").expect("the key box");
    f.pending.push(Event::PointerMoved(input.center()));
    f.pending.push(Event::PointerButton {
        pos: input.center(),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    f.frame(&[]);
    f.pending.push(Event::PointerButton {
        pos: input.center(),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    f.frame(&[]);
    f.pending.push(Event::Text("gsk_bad".into()));
    f.frame(&[]);
    f.frame(&[]);
    f.click_text("Save and test");
    for _ in 0..3 {
        f.frame(&[]);
    }
    assert_eq!(
        f.backend
            .sent(|r| matches!(r, Request::SecretsSet { provider, key } if provider == "groq" && key == "gsk_bad")),
        1
    );
    assert_eq!(f.backend.sent(|r| matches!(r, Request::SecretsTest(p) if p == "groq")), 1);
    assert!(f.find("The key was saved, but the test failed: 401 Unauthorized").is_some());
}

#[test]
fn a_dictation_event_reports_and_fills_the_box() {
    let mut f = Flow::new(Onboarding::with_demo(Some("model")), &["parakeet-tdt-0.6b-v2"], &[]);
    let event = DictationEvent {
        id: 1,
        ok: true,
        text: " Hello there ".into(),
        raw_text: "hello there".into(),
        model: "parakeet-tdt-0.6b-v2".into(),
        cloud: false,
        audio_ms: 1200.0,
        elapsed_ms: 640.0,
        outcome: None,
        fell_back: true,
        error: None,
    };
    f.frame(&[BackendEvent::DictationEvent(event.clone())]);
    f.frame(&[]);
    assert!(f.find("It works. Parakeet TDT 0.6B v2 took 0.6 s (the fallback model was used).").is_some());
    let text = |f: &Flow| match &f.page.body {
        super::Body::Model(step) => step.test().map(|t| t.text().to_string()),
        _ => None,
    };
    assert_eq!(text(&f).as_deref(), Some(""), "the text gets 250 ms to land in the box");
    f.h.wait(0.3);
    f.frame(&[]);
    assert_eq!(text(&f).as_deref(), Some("Hello there"));
    // Already in the box: not added twice.
    f.frame(&[BackendEvent::DictationEvent(event)]);
    f.h.wait(0.3);
    f.frame(&[]);
    assert_eq!(text(&f).as_deref(), Some("Hello there"));

    let failed = DictationEvent { ok: false, error: None, ..Default::default() };
    f.frame(&[BackendEvent::DictationEvent(failed)]);
    f.frame(&[]);
    assert!(f.find("That dictation failed.").is_some());
}

#[test]
fn appending_adds_a_space_only_when_needed() {
    let mut text = String::new();
    super::try_it::append(&mut text, "one");
    super::try_it::append(&mut text, "two");
    assert_eq!(text, "one two");
    text.push('\n');
    super::try_it::append(&mut text, "three");
    assert_eq!(text, "one two\nthree");
}
