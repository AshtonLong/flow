//! Models: pick the speech model that turns your voice into text, download
//! and remove local models, connect cloud providers, register custom
//! endpoints, tune the local engine, and compare models on your own voice.
//! Port of `pages/Models.tsx`. The sections live in their own modules, like
//! the TSX: [`cloud_providers`], [`custom_endpoints`], [`test_bench`].

pub mod cloud_providers;
pub mod custom_endpoints;
pub mod test_bench;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use egui::epaint::StrokeKind;
use egui::{
    Align, Color32, CornerRadius, Galley, Id, Layout, Margin, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2, pos2,
    vec2,
};
use flow_core::catalog::{CatalogSnapshot, DownloadProgress, DownloadState, LocalModelEntry, ModelEntry, RunsOn, Tier};
use flow_core::config::{ConfigPatch, DEFAULT_LOCAL_MODEL, DeviceSetting};
use flow_core::path;
use flow_core::protocol::settings::Request;
use flow_core::types::AppInfo;

use super::{Cx, Page};
use crate::backend::Pending;
use crate::components::button::link_colored;
use crate::components::{
    BadgeTone, Button, ConfirmDialog, Group, IconButton, Notice, Select, SelectOption, SettingRow, Tone, badge, card,
    empty_state, measure, page_header, progress_bar,
};
use crate::design::text::{self, Rich, TextStyle};
use crate::design::{Icon, Weight, icons, tokens};
use crate::util::format::{error_message, format_bytes, format_memory, format_speed, sentence};
use crate::util::models::{installed_local, is_gpu_device, model_label};

use cloud_providers::CloudProviders;
use custom_endpoints::CustomEndpoints;
use test_bench::TestBench;

// ── act: backend calls that report failures on the page ─────────────────────

/// One step of an [`Act`].
pub enum Step {
    /// A call to the main process that answers `()`.
    Call(Request),
    /// A config write. Like `patch` in `lib/config.tsx` it never fails the
    /// act: the shell already shows "The change was not saved: …".
    Patch(Vec<ConfigPatch>),
}

struct Job {
    id: u64,
    steps: VecDeque<Step>,
    /// The step in flight, and whether it is a config write.
    current: Option<(bool, Pending<()>)>,
}

/// `act` in Models.tsx ("Runs a backend call and reports a failure on the
/// page"): runs the steps one after another, then re-reads the catalog. A
/// failure stops the run and shows above the model list.
#[derive(Default)]
pub struct Act {
    error: Option<String>,
    jobs: Vec<Job>,
    next_id: u64,
}

impl Act {
    /// Starts `steps`. Returns an id for [`Act::running`].
    pub fn run(&mut self, cx: &mut Cx<'_>, steps: Vec<Step>) -> u64 {
        self.error = None;
        self.next_id += 1;
        let id = self.next_id;
        self.jobs.push(Job { id, steps: steps.into(), current: None });
        self.pump(cx);
        id
    }

    /// A single call.
    pub fn call(&mut self, cx: &mut Cx<'_>, request: Request) -> u64 {
        self.run(cx, vec![Step::Call(request)])
    }

    /// Whether the run `id` is still going.
    pub fn running(&self, id: u64) -> bool {
        self.jobs.iter().any(|job| job.id == id)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Advances the runs whose current step has settled.
    pub fn pump(&mut self, cx: &mut Cx<'_>) {
        let mut failed = None;
        self.jobs.retain_mut(|job| {
            loop {
                if let Some((patch, pending)) = job.current.as_mut() {
                    match pending.take() {
                        None => return true,
                        Some(Err(err)) if !*patch => {
                            failed = Some(error_message(&err));
                            return false;
                        }
                        Some(_) => job.current = None,
                    }
                }
                match job.steps.pop_front() {
                    Some(Step::Call(request)) => job.current = Some((false, cx.link.call(request))),
                    Some(Step::Patch(patches)) => job.current = Some((true, cx.config.patch(patches))),
                    None => {
                        cx.catalog.reload();
                        return false;
                    }
                }
            }
        });
        if failed.is_some() {
            self.error = failed;
        }
    }
}

// ── Layout helpers shared by the sections ───────────────────────────────────

/// How a [`flex_row`] lines its two sides up.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Cross {
    /// `items-start`; the side starts this far down (`pt-0.5`).
    Start(f32),
    /// `items-center`, at least this tall (`min-h-8`).
    Center(f32),
}

/// `flex gap-N` with a `min-w-0 flex-1` column and a `shrink-0` side: `main`
/// takes the width `side` leaves. `side` lays out left to right, `side_gap`
/// apart, its items centred on each other. Sizes come from the previous pass
/// (see `components::measure`).
pub(crate) fn flex_row<L, R>(
    ui: &mut Ui,
    id: Id,
    gap: f32,
    cross: Cross,
    side_gap: f32,
    main: impl FnOnce(&mut Ui) -> L,
    side: impl FnOnce(&mut Ui) -> R,
) -> (L, R) {
    let ctx = ui.ctx().clone();
    let top = ui.cursor().min;
    let width = ui.available_width();
    let side_size = measure::remembered(&ctx, id.with("side")).unwrap_or(Vec2::ZERO);
    let main_h = measure::remembered(&ctx, id.with("main")).map_or(0.0, |s| s.y);
    let main_w = (width - gap - side_size.x).max(0.0);
    let (main_top, side_top) = match cross {
        Cross::Start(pad) => (top.y, top.y + pad),
        Cross::Center(min_h) => {
            let row_h = main_h.max(side_size.y).max(min_h);
            (top.y + (row_h - main_h) / 2.0, top.y + (row_h - side_size.y) / 2.0)
        }
    };
    let main_rect = Rect::from_min_size(pos2(top.x, main_top), vec2(main_w, f32::INFINITY));
    let mut main_ui = ui.new_child(UiBuilder::new().max_rect(main_rect).layout(Layout::top_down(Align::Min)));
    let left = main(&mut main_ui);
    let main_now = main_ui.min_rect().height().max(0.0);
    measure::record(&ctx, id.with("main"), vec2(main_w, main_now));

    let side_rect = Rect::from_min_size(pos2(top.x + width - side_size.x, side_top), side_size);
    let mut side_ui = ui.new_child(UiBuilder::new().max_rect(side_rect).layout(Layout::left_to_right(Align::Center)));
    side_ui.spacing_mut().item_spacing = vec2(side_gap, 0.0);
    let right = side(&mut side_ui);
    let side_now = side_ui.min_rect().size();
    measure::record(&ctx, id.with("side"), side_now);

    let row_h = match cross {
        Cross::Start(pad) => main_now.max(pad + side_now.y),
        Cross::Center(min_h) => main_now.max(side_now.y).max(min_h),
    };
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, row_h)));
    (left, right)
}

/// One item of a [`flow`] line.
pub(crate) enum Inline {
    /// Text that does not wrap.
    Text(Arc<Galley>, Color32),
    /// `<Badge>`.
    Badge(String, BadgeTone),
    /// A `dt`/`dd` pair 6 px apart (`flex gap-1.5`); a `None` term is `sr-only`.
    /// The value may be an `ExternalLink`.
    Pair { term: Option<Arc<Galley>>, value: Arc<Galley>, color: Color32, link: Option<String> },
}

const BADGE_STYLE: TextStyle = TextStyle::CAPTION.weight(Weight::Semibold);

impl Inline {
    pub(crate) fn text(ui: &Ui, text: &str, style: TextStyle, color: Color32) -> Self {
        Self::Text(text::galley(ui, text, style, color, f32::INFINITY), color)
    }

    fn size(&self, ui: &Ui) -> Vec2 {
        match self {
            Self::Text(galley, _) => galley.size(),
            // `.badge`: 8 px padding and a 1 px transparent border on each side.
            Self::Badge(label, _) => vec2(text::width(ui, label, BADGE_STYLE) + 18.0, 20.0),
            Self::Pair { term, value, .. } => {
                let term_w = term.as_ref().map_or(0.0, |t| t.size().x + 6.0);
                vec2(term_w + value.size().x, value.size().y)
            }
        }
    }
}

/// `flex flex-wrap items-center` with `gap` between items and lines. Returns
/// the URL of a link that was clicked.
pub(crate) fn flow(ui: &mut Ui, gap: Vec2, items: Vec<Inline>) -> Option<String> {
    let t = tokens(ui.ctx());
    let top = ui.cursor().min;
    let width = ui.available_width();
    // Break into lines, then centre each item on its line.
    let sizes: Vec<Vec2> = items.iter().map(|i| i.size(ui)).collect();
    let mut lines: Vec<(usize, usize, f32)> = Vec::new();
    let mut start = 0;
    let mut x = 0.0;
    let mut line_h: f32 = 0.0;
    for (i, size) in sizes.iter().enumerate() {
        if i > start && x + gap.x + size.x > width {
            lines.push((start, i, line_h));
            start = i;
            x = 0.0;
            line_h = 0.0;
        }
        x += if i > start { gap.x } else { 0.0 } + size.x;
        line_h = line_h.max(size.y);
    }
    if start < items.len() {
        lines.push((start, items.len(), line_h));
    }
    let mut clicked = None;
    let mut y = top.y;
    let mut items: Vec<Option<Inline>> = items.into_iter().map(Some).collect();
    for (n, &(from, to, h)) in lines.iter().enumerate() {
        if n > 0 {
            y += gap.y;
        }
        let mut x = top.x;
        for i in from..to {
            let size = sizes[i];
            let rect = Rect::from_min_size(pos2(x, y + (h - size.y) / 2.0), size);
            x += size.x + gap.x;
            match items[i].take() {
                Some(Inline::Text(galley, color)) => {
                    ui.painter().galley(rect.min, galley, color);
                }
                Some(Inline::Badge(label, tone)) => {
                    let mut child = ui.new_child(UiBuilder::new().max_rect(rect.translate(vec2(1.0, 0.0))));
                    badge(&mut child, &label, tone);
                }
                Some(Inline::Pair { term, value, color, link: url }) => {
                    let mut value_x = rect.left();
                    if let Some(term) = term {
                        value_x += term.size().x + 6.0;
                        ui.painter().galley(rect.min, term, t.fg_2);
                    }
                    let value_rect = Rect::from_min_size(pos2(value_x, rect.top()), value.size());
                    match url {
                        Some(url) => {
                            let mut child = ui.new_child(UiBuilder::new().max_rect(value_rect));
                            if link_colored(&mut child, value.text(), TextStyle::CAPTION, color).clicked() {
                                clicked = Some(url);
                            }
                        }
                        None => ui.painter().galley(value_rect.min, value, color),
                    }
                }
                None => {}
            }
        }
        y += h;
    }
    ui.advance_cursor_after_rect(Rect::from_min_max(top, pos2(top.x + width, y)));
    clicked
}

/// `border-t border-line` across the full width of the current Ui.
pub(crate) fn rule(ui: &mut Ui) {
    let t = tokens(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, t.line);
}

/// Runs `content` with `x` points of padding on the left and right (`px-4`).
pub(crate) fn padded<R>(ui: &mut Ui, x: f32, content: impl FnOnce(&mut Ui) -> R) -> R {
    let top = ui.cursor().min;
    let width = ui.available_width();
    let rect = Rect::from_min_size(pos2(top.x + x, top.y), vec2((width - 2.0 * x).max(0.0), f32::INFINITY));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    let out = content(&mut child);
    let bottom = child.min_rect().bottom().max(top.y);
    ui.advance_cursor_after_rect(Rect::from_min_max(top, pos2(top.x + width, bottom)));
    out
}

/// `<p className="sr-only" aria-live="polite">`: a status line only screen readers see.
pub(crate) fn live_region(ui: &mut Ui, id: Id, message: &str) {
    let rect = Rect::from_min_size(ui.cursor().min, Vec2::ZERO);
    ui.interact(rect, id, Sense::hover());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label(message.to_string());
    });
}

/// A spinning `LoaderCircle` for a button (`<LoaderCircle className="spin" />`).
pub(crate) fn spinner_icon(ui: &Ui) -> crate::components::IconSpec {
    let mut icon = crate::components::IconSpec::new(Icon::LoaderCircle, 15.0);
    if crate::design::motion::enabled(ui) {
        icon.rotation = (ui.input(|i| i.time) % 0.9 / 0.9) as f32 * std::f32::consts::TAU;
        ui.ctx().request_repaint();
    }
    icon
}

/// A `<Notice>` whose message holds a link (the TSX nests `<ExternalLink>`).
/// Returns true when the link was clicked.
fn notice_with_link(ui: &mut Ui, tone: Tone, rich: Rich) -> bool {
    let t = tokens(ui.ctx());
    let (bg, icon, icon_color) = match tone {
        Tone::Info => (t.inset, Icon::Info, t.fg_2),
        Tone::Ok => (t.ok_bg, Icon::CircleCheck, t.ok),
        Tone::Warn => (t.warn_bg, Icon::TriangleAlert, t.warn),
        Tone::Danger => (t.danger_bg, Icon::CircleAlert, t.danger),
    };
    let slot = ui.painter().add(Shape::Noop);
    let top = ui.cursor().min;
    let width = ui.available_width();
    // `border px-3 py-2`, the icon 16 px with a 10 px gap.
    let left = top.x + 13.0;
    let content_top = top.y + 9.0;
    icons::paint(ui, icon, Rect::from_min_size(pos2(left, content_top), Vec2::splat(16.0)), 16.0, 2.0, icon_color);
    let text_rect = Rect::from_min_size(pos2(left + 26.0, content_top), vec2(width - 26.0 - 26.0, f32::INFINITY));
    let mut child = ui.new_child(UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
    let clicked = rich.show(&mut child).clicked.is_some();
    let height = child.min_rect().height().max(16.0) + 18.0;
    let rect = Rect::from_min_size(top, vec2(width, height));
    ui.painter().set(
        slot,
        Shape::Vec(vec![
            Shape::rect_filled(rect, CornerRadius::from(4.0), bg),
            Shape::rect_stroke(rect, CornerRadius::from(4.0), Stroke::new(1.0, t.line), StrokeKind::Inside),
        ]),
    );
    ui.advance_cursor_after_rect(rect);
    clicked
}

// ── Download announcements ──────────────────────────────────────────────────

/// Reads out download milestones without flooding a screen reader
/// (`useDownloadAnnouncements`).
#[derive(Default)]
struct Announcer {
    /// The last milestone per model: -1 failed, 0/25/50/75 percent, 101 verifying.
    steps: HashMap<String, i32>,
    message: String,
}

impl Announcer {
    fn update(&mut self, snapshot: Option<&CatalogSnapshot>, progress: &HashMap<String, DownloadProgress>) {
        let mut ids: Vec<&String> = progress.keys().collect();
        ids.sort();
        for id in ids {
            let p = &progress[id];
            let name = model_label(snapshot, id);
            let step = match p.state {
                DownloadState::Error => Some((-1, format!("{name}: download failed."))),
                DownloadState::Verifying => Some((101, format!("{name}: verifying the download."))),
                DownloadState::Downloading if p.total_bytes > 0 => {
                    let step = ((p.received_bytes as f64 / p.total_bytes as f64) * 4.0).floor() as i32 * 25;
                    Some((step, format!("{name}: {step}% downloaded.")))
                }
                _ => None,
            };
            if let Some((step, message)) = step {
                if self.steps.get(id) != Some(&step) {
                    self.message = message;
                }
                self.steps.insert(id.clone(), step);
            }
        }
        let mut gone: Vec<String> = self.steps.keys().filter(|id| !progress.contains_key(*id)).cloned().collect();
        gone.sort();
        for id in gone {
            if snapshot.and_then(|s| s.status.get(&id)).is_some_and(|s| s.ready) {
                self.message = format!("{} is ready.", model_label(snapshot, &id));
            }
            self.steps.remove(&id);
        }
    }
}

// ── Local models ────────────────────────────────────────────────────────────

/// What a local row asked for this frame.
enum LocalAction {
    Download,
    Cancel,
    Activate,
    /// Opens the "Remove?" dialog.
    ConfirmRemove,
    Open(String),
}

fn https(url: Option<&str>) -> Option<&str> {
    url.filter(|u| u.starts_with("https://"))
}

/// The progress line under a download: left and right halves (`justify-between`).
fn progress_text(download: &DownloadProgress, size_bytes: u64) -> (String, Option<String>) {
    if download.state == DownloadState::Verifying {
        return ("Verifying the download…".into(), None);
    }
    let mut left = if download.total_bytes > 0 {
        format!("{}%", (download.received_bytes as f64 / download.total_bytes as f64 * 100.0).floor())
    } else {
        "Starting…".into()
    };
    if download.speed > 0.0 {
        left.push_str(&format!(", {}", format_speed(download.speed)));
    }
    let total = if download.total_bytes > 0 { download.total_bytes } else { size_bytes };
    let right = format!("{} of {}", format_bytes(download.received_bytes as f64), format_bytes(total as f64));
    (left, Some(right))
}

/// `describeDevices`: what the engine found, for the Device row.
fn describe_devices(info: Option<&AppInfo>) -> String {
    let Some(info) = info else {
        return String::new();
    };
    if info.devices.is_empty() {
        return "Devices are detected the first time a local model runs.".into();
    }
    let names: Vec<String> = info
        .devices
        .iter()
        .map(|d| {
            let memory =
                if d.memory_total > 0 { format!(", {}", format_memory(d.memory_total as f64)) } else { String::new() };
            let name = if d.description.is_empty() { &d.name } else { &d.description };
            format!("{name} ({}{memory})", if is_gpu_device(d) { "GPU" } else { "CPU" })
        })
        .collect();
    format!("Detected: {}.", names.join("; "))
}

const MEMORY: [(f64, &str); 5] = [
    (0.0, "Unload immediately"),
    (10.0, "10 minutes (default)"),
    (30.0, "30 minutes"),
    (60.0, "60 minutes"),
    (-1.0, "Always loaded"),
];

fn devices() -> [SelectOption<DeviceSetting>; 3] {
    [
        SelectOption::new(DeviceSetting::Auto, "Automatic"),
        SelectOption::new(DeviceSetting::Cpu, "CPU"),
        SelectOption::new(DeviceSetting::Gpu, "GPU"),
    ]
}

/// `Number(v)` as the config stores it: whole minutes are written as integers.
fn minutes_value(minutes: f64) -> serde_json::Value {
    if minutes.fract() == 0.0 && minutes.abs() < 1e15 {
        serde_json::Value::from(minutes as i64)
    } else {
        serde_json::Value::from(minutes)
    }
}

#[derive(Default)]
pub struct Models {
    act: Act,
    /// The local model whose "Remove?" dialog is open.
    confirm_remove: Option<String>,
    announcer: Announcer,
    cloud: CloudProviders,
    custom: CustomEndpoints,
    bench: TestBench,
}

impl Models {
    fn local_row(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, entry: &LocalModelEntry, has_gpu: Option<bool>) {
        let t = tokens(ui.ctx());
        let snapshot = cx.catalog.snapshot();
        let status = snapshot.and_then(|s| s.status.get(&entry.id)).cloned();
        let download = cx.catalog.download_of(&entry.id).cloned();
        let ready = status.as_ref().is_some_and(|s| s.ready);
        let active = status.as_ref().is_some_and(|s| s.active);
        let loaded = status.as_ref().is_some_and(|s| s.loaded);
        let busy =
            download.as_ref().is_some_and(|d| matches!(d.state, DownloadState::Downloading | DownloadState::Verifying));
        let failed = download.as_ref().filter(|d| d.state == DownloadState::Error);
        let is_fallback = cx.config.config().model.fallback == entry.id;
        let licence_url = https(entry.licence_url.as_deref()).map(str::to_owned);
        let id = Id::unique(("flow-models-local", &entry.id));
        let mut action = None;

        card(ui, Margin::symmetric(16, 12), |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            let main = |ui: &mut Ui| {
                let mut name = vec![Inline::text(ui, &entry.name, TextStyle::BODY.weight(Weight::Semibold), t.fg)];
                if entry.id == DEFAULT_LOCAL_MODEL {
                    name.push(Inline::Badge("Default".into(), BadgeTone::Neutral));
                }
                if loaded {
                    name.push(Inline::Badge("In memory".into(), BadgeTone::Neutral));
                }
                flow(ui, vec2(8.0, 8.0), name);
                ui.add_space(2.0);
                text::paragraph(ui, sentence(&entry.description), TextStyle::CAPTION, t.fg_2);
                ui.add_space(8.0);
                let caption = |s: &str, c| text::galley(ui, s, TextStyle::CAPTION, c, f32::INFINITY);
                let link_style = TextStyle::CAPTION.weight(Weight::Strong);
                let mut facts = vec![
                    Inline::Pair {
                        term: None,
                        value: caption(&format_bytes(entry.size_bytes as f64), t.fg_2),
                        color: t.fg_2,
                        link: None,
                    },
                    Inline::Pair {
                        term: Some(caption("Runs on", t.fg_2)),
                        value: caption(if entry.runs_on == RunsOn::Gpu { "GPU" } else { "CPU" }, t.fg),
                        color: t.fg,
                        link: None,
                    },
                    Inline::Pair {
                        term: Some(caption("Licence", t.fg_2)),
                        value: match &licence_url {
                            Some(_) => text::galley(ui, &entry.licence, link_style, t.fg, f32::INFINITY),
                            None => caption(&entry.licence, t.fg),
                        },
                        color: t.fg,
                        link: licence_url.clone(),
                    },
                ];
                if let Some(wer) = entry.wer {
                    facts.push(Inline::Pair {
                        term: Some(caption("Word error rate", t.fg_2)),
                        value: caption(&format!("{}%", crate::util::format::to_fixed(wer, 2)), t.fg),
                        color: t.fg,
                        link: None,
                    });
                }
                flow(ui, vec2(20.0, 4.0), facts)
            };
            let side = |ui: &mut Ui| {
                let mut action = None;
                if active {
                    badge(ui, "Active", BadgeTone::Accent);
                }
                if ready && !active {
                    if Button::new("Set active").show(ui).clicked() {
                        action = Some(LocalAction::Activate);
                    }
                    let label = format!("Remove {}", entry.name);
                    if IconButton::new(label, Icon::Trash2, 15.0).show(ui).clicked() {
                        action = Some(LocalAction::ConfirmRemove);
                    }
                }
                if !ready && !busy {
                    let label = if failed.is_some() { "Retry" } else { "Download" };
                    if Button::new(label).icon(Icon::Download, 15.0).show(ui).clicked() {
                        action = Some(LocalAction::Download);
                    }
                }
                if busy && Button::new("Cancel").show(ui).clicked() {
                    action = Some(LocalAction::Cancel);
                }
                action
            };
            let (opened, clicked) = flex_row(ui, id, 24.0, Cross::Start(2.0), 4.0, main, side);
            action = clicked.or(opened.map(LocalAction::Open));

            if let (true, Some(download)) = (busy, download.as_ref()) {
                ui.add_space(12.0);
                let value = (download.state != DownloadState::Verifying && download.total_bytes > 0)
                    .then(|| (download.received_bytes as f64 / download.total_bytes as f64) as f32);
                progress_bar(ui, value);
                ui.add_space(6.0);
                let (left, right) = progress_text(download, entry.size_bytes);
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
                let left = text::galley(ui, left, TextStyle::CAPTION, t.fg_2, f32::INFINITY);
                ui.painter().galley(row.min, left, t.fg_2);
                if let Some(right) = right {
                    let right = text::galley(ui, right, TextStyle::CAPTION, t.fg_2, f32::INFINITY);
                    ui.painter().galley(pos2(row.right() - right.size().x, row.top()), right, t.fg_2);
                }
            }
            if let Some(download) = failed {
                ui.add_space(12.0);
                let reason = match download.error.as_deref() {
                    Some(error) if !error.is_empty() => format!(": {error}"),
                    _ => ".".into(),
                };
                let message = format!("The download failed{reason} Retry picks up where it stopped.");
                Notice::new(Tone::Danger, &message).show(ui);
            }
            if !ready && !busy && entry.gated {
                ui.add_space(12.0);
                let mut rich = Rich::new(TextStyle::CAPTION, t.fg_2)
                    .text("Gated download: accept the terms on the model page first");
                if licence_url.is_some() {
                    rich = rich.text(" (").link("open the page").text(")");
                }
                if notice_with_link(ui, Tone::Info, rich.text("."))
                    && let Some(url) = &licence_url
                {
                    action = Some(LocalAction::Open(url.clone()));
                }
            }
            if !ready && entry.runs_on == RunsOn::Gpu && has_gpu == Some(false) {
                ui.add_space(12.0);
                Notice::new(Tone::Warn, "No GPU was detected on this PC. This model will be slow on the CPU.").show(ui);
            }
            if let Some(note) = &entry.note {
                ui.add_space(8.0);
                text::paragraph(ui, note, TextStyle::CAPTION, t.fg_2);
            }
        });

        let mut open = self.confirm_remove.as_deref() == Some(entry.id.as_str());
        if open {
            let title = format!("Remove {}?", entry.name);
            let size = format_bytes(entry.size_bytes as f64);
            let description = if is_fallback {
                format!(
                    "This deletes the {size} model file. It is your fallback model, so cloud failures will have nothing to fall back to until you pick another."
                )
            } else {
                format!("This deletes the {size} model file from this PC. You can download it again at any time.")
            };
            let dialog = ConfirmDialog::new(("models-remove", &entry.id), &title, &description, "Remove model");
            if dialog.show(ui.ctx(), &mut open) {
                self.act.call(cx, Request::ModelsRemove(entry.id.clone()));
            }
            if !open {
                self.confirm_remove = None;
            }
        }

        match action {
            Some(LocalAction::Download) => {
                cx.catalog.clear_progress(&entry.id);
                self.act.call(cx, Request::ModelsDownload(entry.id.clone()));
            }
            Some(LocalAction::Cancel) => {
                self.act.call(cx, Request::ModelsCancel(entry.id.clone()));
            }
            Some(LocalAction::Activate) => {
                self.act.call(cx, Request::ModelsActivate(entry.id.clone()));
            }
            Some(LocalAction::ConfirmRemove) => self.confirm_remove = Some(entry.id.clone()),
            Some(LocalAction::Open(url)) => cx.link.send(Request::AppOpenExternal(url)),
            None => {}
        }
    }

    fn model_settings(ui: &mut Ui, cx: &mut Cx<'_>) {
        let model = cx.config.config().model.clone();
        let snapshot = cx.catalog.snapshot();
        let minutes = model.keep_loaded_minutes;
        let mut memory: Vec<SelectOption<f64>> = MEMORY.iter().map(|&(m, label)| SelectOption::new(m, label)).collect();
        if !MEMORY.iter().any(|&(m, _)| m == minutes) {
            memory.push(SelectOption::new(minutes, format!("{minutes} minutes")).hint("From the file"));
        }
        let mut fallbacks: Vec<SelectOption<String>> = snapshot
            .map(|s| installed_local(s).into_iter().map(|m| SelectOption::new(m.id, m.name)).collect())
            .unwrap_or_default();
        if !fallbacks.iter().any(|o| o.value == model.fallback) {
            fallbacks.push(
                SelectOption::new(model.fallback.clone(), model_label(snapshot, &model.fallback)).hint("Not installed"),
            );
        }
        let description = format!(
            "Automatic uses the GPU when one is found, otherwise the CPU. {}",
            describe_devices(cx.app_info.info())
        );

        Group::new("Model settings").show(ui, |ui| {
            SettingRow::new("Device", description).show(ui, |ui| {
                if let Some(v) = Select::new("model.device", Some(&model.device), &devices()).show(ui).changed {
                    cx.config.set(path!["model", "device"], v);
                }
            });
            SettingRow::new(
                "Keep model in memory",
                "How long a local model stays loaded after you last dictated. Longer is faster to start; shorter frees memory.",
            )
            .show(ui, |ui| {
                if let Some(v) = Select::new("model.keep_loaded_minutes", Some(&minutes), &memory).show(ui).changed {
                    cx.config.set(path!["model", "keep_loaded_minutes"], minutes_value(v));
                }
            });
            SettingRow::new(
                "Fallback model",
                "The installed model Flow switches to when a cloud request fails or the PC is offline.",
            )
            .show(ui, |ui| {
                let select = Select::new("model.fallback", Some(&model.fallback), &fallbacks).width(240.0);
                if let Some(v) = select.show(ui).changed {
                    cx.config.set(path!["model", "fallback"], v);
                }
            });
        });
    }
}

impl Page for Models {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        self.act.pump(cx);
        self.announcer.update(cx.catalog.snapshot(), cx.catalog.progress());

        let Some(snapshot) = cx.catalog.snapshot().cloned() else {
            page_header(ui, "Models", None);
            match cx.catalog.error().map(str::to_owned) {
                Some(error) => {
                    let message = format!("The model catalog could not be loaded: {error}");
                    let retry = Notice::new(Tone::Danger, &message)
                        .show_with_action(ui, |ui| Button::new("Try again").small().show(ui).clicked());
                    if retry {
                        cx.catalog.reload();
                    }
                }
                None => {
                    text::paragraph(ui, "Loading the catalog…", TextStyle::BODY, t.fg_2);
                }
            }
            return;
        };

        page_header(
            ui,
            "Models",
            Some(
                "Pick the speech model that turns your voice into text. Local models are free and work offline; cloud models use your own API key.",
            ),
        );
        live_region(ui, Id::unique("flow-models-announcement"), &self.announcer.message);
        if let Some(error) = self.act.error().map(str::to_owned) {
            Notice::new(Tone::Danger, &error).show(ui);
            ui.add_space(20.0);
        }

        let has_gpu = cx.app_info.info().filter(|i| !i.devices.is_empty()).map(|i| i.devices.iter().any(is_gpu_device));
        let custom_local = snapshot.custom.iter().filter_map(|m| match m {
            ModelEntry::Local(local) => Some(local),
            ModelEntry::Cloud(_) => None,
        });
        let core: Vec<&LocalModelEntry> =
            snapshot.catalog.local.iter().filter(|m| m.tier == Tier::Core).chain(custom_local).collect();
        let accuracy: Vec<&LocalModelEntry> =
            snapshot.catalog.local.iter().filter(|m| m.tier == Tier::Accuracy).collect();

        Group::new("Local").description("Free, offline. Audio never leaves this PC.").show(ui, |ui| {
            for entry in &core {
                self.local_row(ui, cx, entry, has_gpu);
            }
        });

        Group::new("GPU accuracy tier")
            .description("The most accurate open models. They need a recent graphics card with CUDA.")
            .show(ui, |ui| {
                if accuracy.is_empty() {
                    empty_state(ui, "No GPU accuracy models are in the catalog yet.", |_| {});
                }
                for entry in &accuracy {
                    self.local_row(ui, cx, entry, has_gpu);
                }
            });

        self.cloud.show(ui, cx, &mut self.act);
        self.custom.show(ui, cx, &mut self.act);
        Self::model_settings(ui, cx);
        self.bench.show(ui, cx);
    }
}

#[cfg(test)]
mod tests;
