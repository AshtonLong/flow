//! The window: port of `App.tsx` and `main.tsx`. Owns the stores, pumps them
//! once per frame, applies the theme, and draws Boot (before the config
//! arrives), Onboarding (until `general.onboarded`) or the Shell (title strip,
//! nav rail, content panel with the error notice, the issues banner and the
//! current page).

mod chrome;
mod gallery;
mod nav;
mod png;
mod scroll;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{
    Align, Color32, CornerRadius, Id, Layout, OutputCommand, Rect, Stroke, Ui, UiBuilder, ViewportCommand, pos2, vec2,
};
use flow_core::config::ThemeSetting;
use flow_core::protocol::settings::{Event, Request};

use crate::backend::{Backend, EventQueue, Link, Pending};
use crate::components::hotkey_recorder;
use crate::components::{ErrorBoundary, IconButton, Notice, Tone, issues_banner};
use crate::design::{self, Icon, Tokens, focus, tokens};
use crate::pages::{self, Cx, Page, PageId};
use crate::platform::{self, SystemColors};
use crate::stores::{self, AppInfoStore, CatalogStore, ConfigStore, Microphones, SystemTheme};
use crate::util::models::model_label;

use chrome::TITLE_H;
use nav::RAIL_W;
use scroll::PageScroll;

/// How the window is started. See [`crate::run`].
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Open on this page (`--page models`): `general`, `hotkeys`, `models`, …
    pub page: Option<String>,
    /// Render until everything has loaded, save a PNG of the window here, then exit.
    pub screenshot: Option<PathBuf>,
    /// Force light or dark, over `general.theme` and Windows (previews, screenshots).
    pub theme: Option<ThemeSetting>,
    /// Force the high-contrast palette on or off.
    pub high_contrast: Option<bool>,
    /// Force reduced motion on or off.
    pub reduced_motion: Option<bool>,
    /// Scroll the page to this offset first (screenshots of lower parts).
    pub scroll: Option<f32>,
    /// Show the component gallery in the content panel (preview only).
    #[doc(hidden)]
    pub gallery: bool,
    /// Opens an overlay in the gallery for a screenshot: `select`, `confirm`, `modal`.
    #[doc(hidden)]
    pub demo: Option<String>,
    /// Scrolls the page up and down for this many frames, repainting as fast
    /// as possible, prints the frame times and exits (preview only).
    #[doc(hidden)]
    pub bench: Option<u32>,
}

struct Bench {
    frames: u32,
    times: Vec<f64>,
    last: Option<std::time::Instant>,
}

struct Shot {
    path: PathBuf,
    ready_frames: u32,
    requested: bool,
    done: Arc<AtomicBool>,
}

pub(crate) struct SettingsApp {
    link: Link,
    events: EventQueue,
    config: ConfigStore,
    catalog: CatalogStore,
    app_info: AppInfoStore,
    mics: Microphones,
    theme: SystemTheme,
    system_colors: SystemColors,
    options: RunOptions,
    page_id: PageId,
    page: Box<dyn Page>,
    onboarding: Option<Box<dyn Page>>,
    gallery: Option<gallery::Gallery>,
    boundary: ErrorBoundary,
    root_boundary: ErrorBoundary,
    scroll: PageScroll,
    window: Option<platform::window::Handle>,
    styled: Option<(bool, bool)>,
    was_focused: bool,
    pause: Option<Pending<()>>,
    shot: Option<Shot>,
    bench: Option<Bench>,
    /// The window background: `--mica` (paper).
    mica: Color32,
    high_contrast: bool,
    /// Whether the Mica backdrop is behind the window, so the paper is drawn at 84 %.
    backdrop: bool,
}

impl SettingsApp {
    /// `hwnd` is the window's raw handle (0 when there is none).
    pub(crate) fn new(ctx: &egui::Context, backend: Arc<dyn Backend>, options: RunOptions, hwnd: isize) -> Self {
        let ctx = ctx.clone();
        design::fonts::install(&ctx);
        let window = platform::window::Handle::from_raw(hwnd);
        if let Some(handle) = window
            && !platform::caption::install(handle, &ctx)
        {
            log::warn!(target: "settings", "snap layouts unavailable: the window could not be subclassed");
        }
        let link = Link::new(Arc::clone(&backend), ctx.clone());
        let events = EventQueue::subscribe(&backend, &ctx);
        let page_id = options.page.as_deref().and_then(PageId::parse).unwrap_or(PageId::General);
        let mut scroll = PageScroll::default();
        if let Some(offset) = options.scroll {
            scroll.scroll_to(offset);
        }
        let bench = options.bench.map(|frames| Bench { frames, times: Vec::new(), last: None });
        let shot = options.screenshot.clone().map(|path| Shot {
            path,
            ready_frames: 0,
            requested: false,
            done: Arc::new(AtomicBool::new(false)),
        });
        Self {
            config: ConfigStore::new(link.clone()),
            catalog: CatalogStore::new(link.clone()),
            app_info: AppInfoStore::new(link.clone()),
            mics: Microphones::new(),
            theme: SystemTheme::new(&link),
            system_colors: platform::system_colors(),
            gallery: options.gallery.then(|| gallery::Gallery::new(options.demo.clone())),
            options,
            page_id,
            page: pages::create(page_id),
            onboarding: None,
            boundary: ErrorBoundary::new(),
            root_boundary: ErrorBoundary::new(),
            scroll,
            window,
            styled: None,
            was_focused: true,
            pause: None,
            shot,
            bench,
            mica: Tokens::light().mica,
            high_contrast: false,
            backdrop: false,
            link,
            events,
        }
    }

    fn handle_events(&mut self, ctx: &egui::Context, events: &[Event], nav: &mut Option<PageId>) {
        for event in events {
            match event {
                Event::Focus { page } => {
                    // The tray asked for Settings again: come forward, maybe on a page.
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                    if let Some(handle) = self.window {
                        platform::window::bring_to_front(handle);
                    }
                    if let Some(page) = page.as_deref().and_then(PageId::parse) {
                        *nav = Some(page);
                    }
                }
                Event::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
                _ => {}
            }
        }
    }

    /// Applies the appearance; returns whether it is dark.
    fn apply_theme(&mut self, ctx: &egui::Context) -> bool {
        let setting = if self.config.loaded() { self.config.config().general.theme } else { ThemeSetting::System };
        let mut appearance = stores::appearance(self.theme.info(), self.options.theme.unwrap_or(setting));
        if let Some(high_contrast) = self.options.high_contrast {
            appearance.high_contrast = high_contrast;
        }
        if let Some(reduced) = self.options.reduced_motion {
            appearance.reduced_motion = reduced;
        }
        let tokens = Tokens::new(appearance, &self.system_colors);
        self.mica = tokens.mica;
        self.high_contrast = appearance.high_contrast;
        design::apply(ctx, tokens);
        appearance.dark
    }

    fn shell(&mut self, ui: &mut Ui, window: Rect, events: &[Event], nav: &mut Option<PageId>) {
        let t = tokens(ui.ctx());
        let rail = Rect::from_min_max(window.min, pos2(window.left() + RAIL_W, window.bottom()));
        let main = Rect::from_min_max(pos2(rail.right(), window.top()), window.max);
        chrome::drag_region(
            ui,
            Rect::from_min_max(rail.min, pos2(rail.right(), rail.top() + TITLE_H)),
            Id::unique("flow-drag-rail"),
        );
        chrome::drag_region(
            ui,
            Rect::from_min_max(main.min, pos2(main.right(), main.top() + TITLE_H)),
            Id::unique("flow-drag-main"),
        );
        nav::logo_strip(ui, rail);
        let list = Rect::from_min_max(
            pos2(rail.left(), rail.top() + TITLE_H),
            pos2(rail.right(), rail.bottom() - nav::footer_height()),
        );
        if let Some(page) = nav::nav_items(ui, list, self.page_id) {
            *nav = Some(page);
        }

        let config = self.config.config();
        let status = nav::Status {
            paused: self.app_info.paused(),
            active_model: &config.model.active,
            model_label: model_label(self.catalog.snapshot(), &config.model.active),
        };
        if nav::status_footer(ui, rail, &status) && self.pause.is_none() {
            self.pause = Some(self.link.call(Request::AppSetPaused(!status.paused)));
        }

        // The content panel: `rounded-tl-lg border-t border-l border-line bg-layer`.
        let panel = Rect::from_min_max(pos2(main.left(), main.top() + TITLE_H), main.max);
        let r = CornerRadius { nw: 8, ne: 0, sw: 0, se: 0 };
        ui.painter().rect_filled(panel, r, t.layer);
        let border =
            ui.painter().with_clip_rect(Rect::from_min_max(panel.min, pos2(panel.right() + 1.0, panel.bottom() + 1.0)));
        border.rect_stroke(
            Rect::from_min_max(panel.min, pos2(panel.right() + 2.0, panel.bottom() + 2.0)),
            r,
            Stroke::new(1.0, t.line),
            egui::StrokeKind::Inside,
        );

        let inner = Rect::from_min_max(pos2(panel.left() + 1.0, panel.top() + 1.0), panel.max);
        let Self { scroll, page, page_id, boundary, config, catalog, app_info, mics, link, gallery, .. } = self;
        let scroll_id = Id::unique(("flow-page-scroll", page_id.as_str(), gallery.is_some()));
        scroll.show(ui, inner, scroll_id, |ui| {
            // `mx-auto max-w-[760px] px-8 pt-6 pb-12`
            let avail = ui.available_width();
            let column_w = avail.min(760.0);
            let left = ui.max_rect().left() + (avail - column_w) / 2.0 + 32.0;
            ui.add_space(24.0);
            let top = ui.cursor().min.y;
            let column = Rect::from_min_size(pos2(left, top), vec2(column_w - 64.0, f32::INFINITY));
            let mut col = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));
            if let Some(error) = config.error().map(str::to_owned) {
                let dismiss = Notice::new(Tone::Danger, &error).show_with_action(&mut col, |ui| {
                    IconButton::new("Dismiss", Icon::X, 14.0).small().show(ui).clicked()
                });
                if dismiss {
                    config.dismiss_error();
                }
                col.add_space(12.0);
            }
            let issues = config.snapshot().issues.clone();
            issues_banner(&mut col, &issues, link);
            let mut cx = Cx { config, catalog, app_info, mics, link, events, nav };
            match gallery {
                Some(gallery) => gallery.ui(&mut col, &mut cx),
                None => boundary.show(&mut col, page_id.as_str(), |ui| page.ui(ui, &mut cx)),
            }
            let bottom = col.min_rect().bottom();
            ui.advance_cursor_after_rect(Rect::from_min_max(
                pos2(ui.max_rect().left(), top),
                pos2(ui.max_rect().left() + 1.0, bottom),
            ));
            ui.add_space(48.0);
        });
    }

    fn boot(&self, ui: &mut Ui, window: Rect) {
        // Before the config arrives: follow the system theme and say what is wrong, if anything.
        chrome::drag_region(
            ui,
            Rect::from_min_max(window.min, pos2(window.right(), window.top() + TITLE_H)),
            Id::unique("flow-drag-boot"),
        );
        if let Some(error) = self.config.error() {
            let width = window.width().min(480.0) - 48.0;
            let rect = Rect::from_min_size(
                pos2(window.center().x - width / 2.0, window.top() + TITLE_H + 64.0),
                vec2(width, f32::INFINITY),
            );
            let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
            Notice::new(Tone::Danger, &format!("Flow could not read its settings: {error}")).show(&mut child);
        }
    }

    fn onboarding(&mut self, ui: &mut Ui, window: Rect, events: &[Event], nav: &mut Option<PageId>) {
        chrome::drag_region(
            ui,
            Rect::from_min_max(window.min, pos2(window.right(), window.top() + TITLE_H)),
            Id::unique("flow-drag-onboarding"),
        );
        let page = self.onboarding.get_or_insert_with(|| Box::new(crate::onboarding::Onboarding::default()));
        let rect = Rect::from_min_max(pos2(window.left(), window.top() + TITLE_H), window.max);
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
        let mut cx = Cx {
            config: &mut self.config,
            catalog: &mut self.catalog,
            app_info: &mut self.app_info,
            mics: &mut self.mics,
            link: &self.link,
            events,
            nav,
        };
        self.root_boundary.show(&mut child, "onboarding", |ui| page.ui(ui, &mut cx));
    }

    /// The page background: `color-mix(in srgb, var(--mica) 84%, transparent)`
    /// over Mica, solid paper without it.
    fn window_fill(&self) -> Color32 {
        if self.backdrop { self.mica.gamma_multiply_u8(214) } else { self.mica }
    }

    fn bench(&mut self, ctx: &egui::Context) {
        let ready = self.config.loaded() && self.catalog.snapshot().is_some();
        let Some(bench) = self.bench.as_mut() else { return };
        ctx.request_repaint();
        if !ready {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(last) = bench.last {
            bench.times.push((now - last).as_secs_f64() * 1000.0);
        }
        bench.last = Some(now);
        let n = bench.times.len() as u32;
        // Down the page and back, 4 pt a frame.
        let phase = n % 200;
        let offset = if phase < 100 { phase as f32 * 4.0 } else { (200 - phase) as f32 * 4.0 };
        self.scroll.scroll_to(offset);
        if n >= bench.frames {
            let mut t = bench.times.clone();
            t.sort_by(f64::total_cmp);
            let avg = t.iter().sum::<f64>() / t.len() as f64;
            let p95 = t[(t.len() * 95 / 100).min(t.len() - 1)];
            eprintln!(
                "bench: {} frames, frame interval avg {avg:.2} ms, p95 {p95:.2} ms, max {:.2} ms",
                t.len(),
                t[t.len() - 1]
            );
            ctx.send_viewport_cmd(ViewportCommand::Close);
            self.bench = None;
        }
    }

    fn screenshot(&mut self, ctx: &egui::Context) {
        let ready = self.config.loaded() && self.catalog.snapshot().is_some() && self.app_info.info().is_some();
        let Some(shot) = self.shot.as_mut() else {
            return;
        };
        if shot.done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        ctx.request_repaint();
        if shot.requested {
            return;
        }
        if ready {
            shot.ready_frames += 1;
        }
        // A few frames for fonts, measured rows and the first animations to settle.
        if shot.ready_frames >= 20 {
            shot.requested = true;
            let path = shot.path.clone();
            let done = Arc::clone(&shot.done);
            let wake = ctx.clone();
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| {
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
                match png::write(&path, image.size[0] as u32, image.size[1] as u32, &rgba) {
                    Ok(()) => {
                        log::info!(target: "settings", "screenshot saved to {}", path.display())
                    }
                    Err(err) => log::error!(target: "settings", "screenshot not saved: {err}"),
                }
                done.store(true, Ordering::SeqCst);
                wake.request_repaint();
            })));
        }
    }
}

impl crate::runner::App for SettingsApp {
    fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let events = self.events.drain();
        let mut nav: Option<PageId> = None;
        self.handle_events(&ctx, &events, &mut nav);
        self.config.pump(&events);
        self.catalog.pump(&events);
        self.theme.pump(&events);
        let focused = ctx.input(|i| i.focused);
        let gained = focused && !self.was_focused;
        self.was_focused = focused;
        self.app_info.pump(gained);
        if gained {
            self.mics.refresh();
        }
        if let Some(result) = self.pause.as_mut().and_then(Pending::take) {
            self.pause = None;
            if let Err(err) = result {
                log::warn!(target: "settings", "pause failed: {err}");
            }
            self.app_info.refresh();
        }

        let dark = self.apply_theme(&ctx);
        focus::update(&ctx);
        // Mica as Electron's `backgroundMaterial`: none under high contrast, and
        // none in screenshots (they compare against the opaque preview).
        let want_mica = !self.high_contrast && self.shot.is_none();
        if let Some(handle) = self.window
            && self.styled != Some((dark, want_mica))
        {
            platform::window::style(handle, dark);
            self.backdrop = platform::window::backdrop(handle, want_mica);
            self.styled = Some((dark, want_mica));
        }

        let window = ui.max_rect();
        ui.painter().rect_filled(window, 0.0, self.window_fill());
        if !self.config.loaded() {
            self.boot(ui, window);
        } else if !self.config.config().general.onboarded && self.gallery.is_none() {
            self.onboarding(ui, window, &events, &mut nav);
        } else {
            self.onboarding = None;
            self.shell(ui, window, &events, &mut nav);
        }
        chrome::caption_buttons(ui, window, dark);
        chrome::resize_edges(ui, window);

        if let Some(next) = nav
            && next != self.page_id
        {
            hotkey_recorder::cancel_capture(&ctx, &self.link);
            self.page_id = next;
            self.page = pages::create(next);
            self.gallery = None;
            self.scroll.reset();
            ctx.request_repaint();
        }

        // egui-winit is built without its clipboard feature; copies go out through Win32.
        let copied: Vec<String> = ctx.output(|o| {
            o.commands
                .iter()
                .filter_map(|c| match c {
                    OutputCommand::CopyText(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        });
        for text in copied {
            platform::clipboard::set_text(&text);
        }
        self.screenshot(&ctx);
        self.bench(&ctx);
    }

    fn clear_color(&self) -> Color32 {
        if self.backdrop { Color32::TRANSPARENT } else { self.mica }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let capturing = hotkey_recorder::capturing(ctx);
        let mut events = Vec::with_capacity(raw_input.events.len());
        for event in raw_input.events.drain(..) {
            match &event {
                // While a hotkey is being recorded, keys belong to the recorder.
                egui::Event::Key { .. } | egui::Event::Text(_) if capturing => {}
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    use egui::Key;
                    let paste = (*key == Key::V && modifiers.command && !modifiers.alt)
                        || (*key == Key::Insert && modifiers.shift);
                    let copy = (*key == Key::C && modifiers.command && !modifiers.alt)
                        || (*key == Key::Insert && modifiers.command);
                    let cut = (*key == Key::X && modifiers.command && !modifiers.alt)
                        || (*key == Key::Delete && modifiers.shift);
                    if paste {
                        if let Some(text) = platform::clipboard::get_text() {
                            let text = text.replace("\r\n", "\n");
                            if !text.is_empty() {
                                events.push(egui::Event::Paste(text));
                            }
                        }
                    } else if copy {
                        events.push(egui::Event::Copy);
                    } else if cut {
                        events.push(egui::Event::Cut);
                    } else {
                        events.push(event);
                    }
                }
                _ => events.push(event),
            }
        }
        raw_input.events = events;
    }
}

/// The window icon (`resources/icons/icon.png`).
pub(crate) fn icon() -> Option<egui::IconData> {
    let mut decoder = ::png::Decoder::new(std::io::Cursor::new(&include_bytes!("../../../../../resources/icons/icon.png")[..]));
    decoder.set_transformations(::png::Transformations::normalize_to_color8() | ::png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let rgba = match info.color_type {
        ::png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        ::png::ColorType::GrayscaleAlpha => {
            buf[..info.buffer_size()].chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        _ => return None,
    };
    Some(egui::IconData { rgba, width: info.width, height: info.height })
}
