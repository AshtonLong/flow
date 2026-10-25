//! Headless tests: real egui passes with simulated pointer and keyboard input,
//! a synchronous in-memory backend, and the real components and pages.

use std::sync::Arc;

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, pos2, vec2};
use flow_core::config::value::apply_patches;
use flow_core::config::{ConfigSnapshot, validate_config};
use flow_core::protocol::settings::{Event as BackendEvent, Reply, Request};
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::backend::{Backend, Link};
use crate::design::{self, Tokens};
use crate::pages::{Cx, Page, PageId};
use crate::stores::{AppInfoStore, Appearance, CatalogStore, ConfigStore, Microphones};

/// A test's own answers, tried before the defaults (`None` falls through).
type Responder = Box<dyn Fn(&Request) -> Option<Reply> + Send + Sync>;

/// Answers at once: config reads and writes against an in-memory file, and
/// empty answers for everything else. Records every request.
pub(crate) struct Instant {
    raw: Mutex<Value>,
    pub requests: Mutex<Vec<Request>>,
    responder: Mutex<Option<Responder>>,
}

impl Instant {
    pub(crate) fn new(raw: Value) -> Arc<Self> {
        Arc::new(Self { raw: Mutex::new(raw), requests: Mutex::new(Vec::new()), responder: Mutex::new(None) })
    }

    /// Answers requests with `respond` first; `None` falls back to the defaults.
    pub(crate) fn respond_with(&self, respond: impl Fn(&Request) -> Option<Reply> + Send + Sync + 'static) {
        *self.responder.lock() = Some(Box::new(respond));
    }

    fn snapshot(&self) -> ConfigSnapshot {
        let validated = validate_config(&self.raw.lock());
        ConfigSnapshot { config: validated.config, issues: validated.issues, path: "config.toml".into() }
    }

    pub(crate) fn raw(&self) -> Value {
        self.raw.lock().clone()
    }
}

impl Backend for Instant {
    fn request(&self, request: Request, reply: Box<dyn FnOnce(Reply) + Send>) {
        self.requests.lock().push(request.clone());
        let custom = self.responder.lock().as_ref().and_then(|respond| respond(&request));
        if let Some(answer) = custom {
            reply(answer);
            return;
        }
        let answer = match request {
            Request::ConfigGet => Reply::ok(&self.snapshot()),
            Request::ConfigSet(patches) => {
                let next = apply_patches(&self.raw.lock(), &patches);
                *self.raw.lock() = next;
                Reply::ok(&self.snapshot())
            }
            Request::AppInfo => Reply::ok(&flow_core::types::AppInfo::default()),
            Request::ThemeGet => Reply::ok(&flow_core::types::ThemeInfo::default()),
            Request::ModelsList => Reply::err("no catalog in tests"),
            _ => Reply::ok(&()),
        };
        reply(answer);
    }
    fn subscribe(&self, _on_event: Box<dyn Fn(BackendEvent) + Send + Sync>) {}
}

/// An egui context with Flow's fonts and tokens, stepped frame by frame.
pub(crate) struct Harness {
    pub ctx: egui::Context,
    pub time: f64,
    pending: Vec<Event>,
    pointer: Pos2,
    /// What the last frame painted.
    pub shapes: Vec<egui::epaint::ClippedShape>,
    /// The window size (1000×700); make it taller to see a whole long page.
    pub screen: egui::Vec2,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let ctx = egui::Context::default();
        design::fonts::install(&ctx);
        let appearance = Appearance { dark: false, high_contrast: false, reduced_motion: true };
        design::apply(&ctx, Tokens::new(appearance, &crate::platform::system_colors()));
        Self {
            ctx,
            time: 0.0,
            pending: Vec::new(),
            pointer: Pos2::ZERO,
            shapes: Vec::new(),
            screen: vec2(1000.0, 700.0),
        }
    }

    /// Runs one frame (egui may run extra passes for measured rows).
    pub(crate) fn frame(&mut self, mut ui_fn: impl FnMut(&mut Ui)) {
        self.time += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen)),
            time: Some(self.time),
            focused: true,
            events: std::mem::take(&mut self.pending),
            ..RawInput::default()
        };
        let mut output = self.ctx.run_ui(input, |ui| ui_fn(ui));
        self.shapes = std::mem::take(&mut output.shapes);
        output.drop_without_applying_deltas();
    }

    /// Everywhere the last frame painted a run of text that reads exactly `text`, in paint order.
    pub(crate) fn text_rects(&self, text: &str) -> Vec<Rect> {
        fn find(shape: &egui::Shape, text: &str, out: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == text => out.push(t.galley.rect.translate(t.pos.to_vec2())),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| find(s, text, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        self.shapes.iter().for_each(|s| find(&s.shape, text, &mut out));
        out
    }

    /// Where the last frame first painted a run of text that reads exactly `text`.
    pub(crate) fn text_rect(&self, text: &str) -> Option<Rect> {
        self.text_rects(text).into_iter().next()
    }

    /// True when the last frame painted `text` as one run.
    pub(crate) fn shows(&self, text: &str) -> bool {
        self.text_rect(text).is_some()
    }

    pub(crate) fn wait(&mut self, seconds: f64) {
        self.time += seconds;
    }

    pub(crate) fn press(&mut self, pos: Pos2) {
        self.pointer = pos;
        self.pending.push(Event::PointerMoved(pos));
        self.pending.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
    }

    pub(crate) fn release(&mut self) {
        let pos = self.pointer;
        self.pending.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }

    pub(crate) fn key(&mut self, key: Key) {
        self.key_with(key, Modifiers::NONE);
    }

    /// A key with modifiers held: `key_with(Key::Tab, Modifiers::SHIFT)`.
    pub(crate) fn key_with(&mut self, key: Key, modifiers: Modifiers) {
        for pressed in [true, false] {
            self.pending.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        }
    }

    /// Where the widget with keyboard focus is, if any.
    pub(crate) fn focused_rect(&self) -> Option<Rect> {
        let id = self.ctx.memory(|m| m.focused())?;
        self.ctx.read_response(id).map(|r| r.rect)
    }

    pub(crate) fn type_text(&mut self, text: &str) {
        self.pending.push(Event::Text(text.into()));
    }

    /// Clicks over two frames, drawing `ui_fn` each time.
    pub(crate) fn click(&mut self, pos: Pos2, mut ui_fn: impl FnMut(&mut Ui)) {
        self.press(pos);
        self.frame(&mut ui_fn);
        self.release();
        self.frame(&mut ui_fn);
    }
}

/// The stores and a page, drawn in a 696 px column at (0, 0) like the shell's.
pub(crate) struct PageHarness {
    pub h: Harness,
    pub backend: Arc<Instant>,
    link: Link,
    config: ConfigStore,
    catalog: CatalogStore,
    app_info: AppInfoStore,
    mics: Microphones,
    page: Box<dyn Page>,
    pub nav: Option<PageId>,
    /// Pushes from the main process, delivered with the next frame.
    pub events: Vec<BackendEvent>,
}

impl PageHarness {
    pub(crate) fn new(page: PageId, raw: Value) -> Self {
        let h = Harness::new();
        let backend = Instant::new(raw);
        let link = Link::new(backend.clone(), h.ctx.clone());
        Self {
            config: ConfigStore::new(link.clone()),
            catalog: CatalogStore::new(link.clone()),
            app_info: AppInfoStore::new(link.clone()),
            mics: Microphones::new(),
            page: crate::pages::create(page),
            link,
            backend,
            h,
            nav: None,
            events: Vec::new(),
        }
    }

    pub(crate) fn frame(&mut self) {
        let Self { h, config, catalog, app_info, mics, page, link, nav, events, .. } = self;
        let events = std::mem::take(events);
        h.frame(|ui| {
            // As the shell does at the start of every frame.
            design::focus::update(ui.ctx());
            config.pump(&[]);
            catalog.pump(&[]);
            app_info.pump(false);
            if !config.loaded() {
                return;
            }
            let column = Rect::from_min_size(pos2(0.0, 0.0), vec2(696.0, 5000.0));
            let mut col = ui.new_child(egui::UiBuilder::new().max_rect(column));
            let mut cx = Cx { config, catalog, app_info, mics, link, events: &events, nav };
            page.ui(&mut col, &mut cx);
        });
    }

    pub(crate) fn click(&mut self, pos: Pos2) {
        self.h.press(pos);
        self.frame();
        self.h.release();
        self.frame();
    }

    pub(crate) fn config(&self) -> &flow_core::config::Config {
        self.config.config()
    }

    /// Clicks the middle of the text `label` painted last frame (a button's label).
    pub(crate) fn click_text(&mut self, label: &str) {
        let rect = self.h.text_rect(label).unwrap_or_else(|| panic!("“{label}” is not on screen"));
        self.click(rect.center());
    }

    /// The requests sent so far.
    pub(crate) fn requests(&self) -> Vec<Request> {
        self.backend.requests.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{ChipsInput, ErrorBoundary, NumberField, Select, SelectOption, Toggle};

    #[test]
    fn a_toggle_flips_on_click() {
        let mut h = Harness::new();
        let mut on = false;
        let mut draw = |ui: &mut Ui| {
            if let Some(v) = Toggle::new(on).show(ui).changed {
                on = v;
            }
        };
        h.frame(&mut draw);
        // "Off" (about 22 px) + 12 px gap, then the 42 px track: click its middle.
        h.click(pos2(60.0, 12.0), &mut draw);
        assert!(on);
    }

    #[test]
    fn a_select_opens_on_press_and_picks_with_the_pointer_and_keys() {
        let mut h = Harness::new();
        let options = [
            SelectOption::new(1, "One"),
            SelectOption::new(2, "Two"),
            SelectOption::new(3, "Three").disabled(true),
            SelectOption::new(4, "Four"),
        ];
        let value = std::cell::Cell::new(1);
        let mut draw = |ui: &mut Ui| {
            ui.add_space(100.0);
            let current = value.get();
            if let Some(v) = Select::new("test.select", Some(&current), &options).show(ui).changed {
                value.set(v);
            }
        };
        h.frame(&mut draw);
        // The trigger is 200×32 at y = 100; the flyout opens 4 px below with 4 px padding.
        h.click(pos2(100.0, 116.0), &mut draw);
        let second_item = pos2(100.0, 132.0 + 4.0 + 4.0 + 34.0 + 17.0);
        h.press(second_item);
        h.frame(&mut draw);
        h.release();
        h.frame(&mut draw);
        assert_eq!(value.get(), 2);

        // Keyboard: open with Enter on the focused trigger, Down skips the disabled item.
        h.key(Key::Enter);
        h.frame(&mut draw);
        h.key(Key::ArrowDown);
        h.frame(&mut draw);
        h.key(Key::ArrowDown);
        h.frame(&mut draw);
        h.key(Key::Enter);
        h.frame(&mut draw);
        assert_eq!(value.get(), 4);
    }

    #[test]
    fn a_number_field_clamps_and_commits_on_enter() {
        let mut h = Harness::new();
        let mut value = 300i64;
        let mut draw = |ui: &mut Ui| {
            if let Some(v) = NumberField::new("test.number", value, 5..=3600).unit("seconds").show(ui).committed {
                value = v;
            }
        };
        h.frame(&mut draw);
        h.click(pos2(44.0, 16.0), &mut draw);
        for _ in 0..3 {
            h.key(Key::Backspace);
        }
        h.type_text("9x9999");
        h.frame(&mut draw);
        h.key(Key::Enter);
        h.frame(&mut draw);
        h.frame(&mut draw);
        assert_eq!(value, 3600, "letters are dropped and the number is clamped");
    }

    #[test]
    fn a_text_field_commits_after_a_pause_in_typing() {
        let mut h = Harness::new();
        let value = std::cell::RefCell::new("Ctrl+V".to_string());
        let commits = std::cell::Cell::new(0);
        let mut draw = |ui: &mut Ui| {
            let current = value.borrow().clone();
            if let Some(v) = crate::components::TextField::new("test.text", &current).show(ui).committed {
                *value.borrow_mut() = v;
                commits.set(commits.get() + 1);
            }
        };
        h.frame(&mut draw);
        h.click(pos2(100.0, 16.0), &mut draw);
        h.type_text("!");
        h.frame(&mut draw);
        assert_eq!(commits.get(), 0, "nothing is saved while typing");
        h.wait(0.75);
        h.frame(&mut draw);
        assert_eq!(*value.borrow(), "Ctrl+V!");
        assert_eq!(commits.get(), 1);
    }

    #[test]
    fn escape_in_a_text_field_reverts_the_draft_and_keeps_focus() {
        let mut h = Harness::new();
        let value = std::cell::RefCell::new("Ctrl+V".to_string());
        let mut draw = |ui: &mut Ui| {
            let current = value.borrow().clone();
            if let Some(v) = crate::components::TextField::new("test.text", &current).show(ui).committed {
                *value.borrow_mut() = v;
            }
        };
        h.frame(&mut draw);
        h.click(pos2(100.0, 16.0), &mut draw);
        h.type_text("!");
        h.frame(&mut draw);
        h.key(Key::Escape);
        h.frame(&mut draw);
        h.frame(&mut draw);
        assert!(h.focused_rect().is_some(), "Chromium keeps focus in the box");
        h.wait(1.0);
        h.frame(&mut draw);
        assert_eq!(*value.borrow(), "Ctrl+V", "the typing is undone, not saved");
    }

    #[test]
    fn chips_add_on_enter_and_comma() {
        let mut h = Harness::new();
        let values = std::cell::RefCell::new(vec!["tory".to_string()]);
        let mut draw = |ui: &mut Ui| {
            let current = values.borrow().clone();
            if let Some(next) = ChipsInput::new("test.chips", &current).show(ui) {
                *values.borrow_mut() = next;
            }
        };
        h.frame(&mut draw);
        h.click(pos2(400.0, 16.0), &mut draw);
        h.type_text("towery");
        h.frame(&mut draw);
        h.key(Key::Enter);
        h.frame(&mut draw);
        assert_eq!(*values.borrow(), vec!["tory".to_string(), "towery".to_string()]);
        h.type_text("a,b");
        h.frame(&mut draw);
        h.frame(&mut draw);
        assert_eq!(values.borrow().len(), 4);
    }

    #[test]
    fn the_error_boundary_catches_a_broken_page() {
        let mut h = Harness::new();
        let mut boundary = ErrorBoundary::new();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        h.frame(|ui| boundary.show(ui, "general", |_| panic!("boom")));
        std::panic::set_hook(previous);
        // Drawn again without panicking: the notice stands in for the page.
        h.frame(|ui| boundary.show(ui, "general", |_| panic!("still broken")));
        // Another page clears the error.
        let mut drew = false;
        h.frame(|ui| boundary.show(ui, "hotkeys", |_| drew = true));
        assert!(drew);
    }

    #[test]
    fn the_general_page_writes_what_is_toggled() {
        let mut p = PageHarness::new(PageId::General, json!({ "general": { "onboarded": true } }));
        for _ in 0..3 {
            p.frame();
        }
        assert!(!p.config().general.launch_at_login);
        // Title 36 + 24, group heading 32 + 8, card padding 12, row 40: the first toggle
        // is centred 20 px into the row, its track ending 16 px from the card's edge.
        let toggle = pos2(696.0 - 16.0 - 21.0, 24.0 + 36.0 + 24.0 + 32.0 + 8.0 + 12.0 + 20.0 - 24.0);
        p.click(toggle);
        assert!(p.config().general.launch_at_login, "the optimistic state shows at once");
        assert_eq!(p.backend.raw()["general"]["launch_at_login"], json!(true));
    }
}
