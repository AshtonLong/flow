//! The recording pill, drawn as `src/renderer/overlay/index.html` and
//! `styles.css` lay it out: black (as on the marketing site), with the cloud,
//! the waveform or dots or a mark, then a label, then the lock and the queue
//! count. Sizes are CSS pixels times the size scale (0.84 / 1 / 1.22) times
//! the monitor's scale factor; everything is drawn in device pixels.
//!
//! The parts that never move between frames (shadows, body, icons, text) are
//! cached as one layer; each frame adds the waveform bars or the dots and, while
//! the pill enters or leaves, the CSS transform.

use std::time::Duration;

use flow_core::config::{OverlaySize, OverlayStyle};
use flow_core::types::{OverlayAppearance, OverlayPhase, OverlayState};
use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect,
    Stroke, Transform,
};

use crate::anim::CubicBezier;
use crate::blur;
use crate::color::{Rgba, SystemColors, brand};
use crate::svgpath;
use crate::text::{Font, PILL_WEIGHT, ShapedText, TextLook, tabular_nums};

/// The overlay window, in CSS pixels (overlay.ts).
pub const WINDOW_WIDTH: f32 = 460.0;
pub const WINDOW_HEIGHT: f32 = 84.0;
/// Bars in the waveform.
pub const BARS: usize = 13;
/// The entrance/exit: `opacity 180ms ease`, `transform 280ms cubic-bezier(0.16, 1, 0.3, 1)`.
pub const FADE: Duration = Duration::from_millis(180);
pub const MOVE: Duration = Duration::from_millis(280);
pub const MOVE_EASING: CubicBezier = CubicBezier::new(0.16, 1.0, 0.3, 1.0);
/// One cycle of the transcribing dots, and each dot's delay.
const DOT_CYCLE: f32 = 0.9;
const DOT_DELAYS: [f32; 3] = [0.0, 0.12, 0.24];

/// What the page shows: the result of `applyState`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PillLook {
    /// The stage's `data-phase`.
    pub phase: OverlayPhase,
    /// `#label`'s text. Kept while fading out so the pill does not collapse first.
    pub label: String,
    pub cloud: bool,
    pub lock: bool,
    /// `+N`, shown while listening with earlier dictations still transcribing.
    pub queued: Option<u32>,
}

impl Default for PillLook {
    fn default() -> Self {
        Self { phase: OverlayPhase::Hidden, label: String::new(), cloud: false, lock: false, queued: None }
    }
}

/// What the pill says in each state when main sends no message of its own.
pub fn default_label(phase: OverlayPhase) -> &'static str {
    match phase {
        OverlayPhase::Listening => "Listening",
        OverlayPhase::Transcribing => "Transcribing",
        OverlayPhase::Done => "Typed",
        OverlayPhase::Hidden | OverlayPhase::Error | OverlayPhase::Notice => "",
    }
}

impl PillLook {
    /// `applyState`.
    pub fn apply(&mut self, state: &OverlayState) {
        self.phase = state.phase;
        if state.phase != OverlayPhase::Hidden {
            self.label = state.message.clone().unwrap_or_else(|| default_label(state.phase).to_string());
        }
        self.lock = state.phase == OverlayPhase::Listening && state.locked;
        self.cloud = state.cloud;
        self.queued = (state.phase == OverlayPhase::Listening && state.queued > 0).then_some(state.queued);
    }

    pub fn from_state(state: &OverlayState) -> Self {
        let mut look = Self::default();
        look.apply(state);
        look
    }
}

/// The parts of the appearance the drawing depends on (`applyAppearance`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PillStyle {
    pub style: OverlayStyle,
    pub size: OverlaySize,
    pub show_waveform: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    /// Used when `high_contrast`.
    pub system: SystemColors,
}

impl Default for PillStyle {
    fn default() -> Self {
        Self {
            style: OverlayStyle::Pill,
            size: OverlaySize::Medium,
            show_waveform: true,
            high_contrast: false,
            reduced_motion: false,
            system: SystemColors::default(),
        }
    }
}

impl PillStyle {
    pub fn from_appearance(appearance: &OverlayAppearance, system: SystemColors) -> Self {
        Self {
            style: appearance.style,
            size: appearance.size,
            show_waveform: appearance.show_waveform,
            high_contrast: appearance.theme.high_contrast,
            reduced_motion: appearance.theme.reduced_motion,
            system,
        }
    }

    /// `--scale`
    pub fn size_scale(&self) -> f32 {
        match self.size {
            OverlaySize::Small => 0.84,
            OverlaySize::Medium => 1.0,
            OverlaySize::Large => 1.22,
        }
    }

    fn minimal(&self) -> bool {
        self.style == OverlayStyle::Minimal
    }
}

/// The custom properties of styles.css, normal and `data-contrast='high'`.
#[derive(Debug, Clone, Copy)]
struct Palette {
    bg: Rgba,
    fg: Rgba,
    dim: Rgba,
    edge: Rgba,
    /// CSS px of the inset edge.
    edge_width: f32,
    voice: Rgba,
    cloud: Rgba,
    ok: Rgba,
    err: Rgba,
    drop_shadows: bool,
}

impl Palette {
    fn for_style(style: &PillStyle) -> Self {
        if style.high_contrast {
            let s = style.system;
            Self {
                bg: s.canvas,
                fg: s.canvas_text,
                dim: s.canvas_text,
                edge: s.canvas_text,
                edge_width: 2.0,
                voice: s.highlight,
                cloud: s.canvas_text.mix_oklab(brand::WHITE, 0.55),
                ok: s.canvas_text,
                err: s.canvas_text,
                drop_shadows: false,
            }
        } else {
            Self {
                bg: brand::BLACK,
                fg: brand::WHITE,
                dim: brand::WHITE.with_alpha(0.66),
                edge: brand::WHITE.with_alpha(0.14),
                edge_width: 1.0,
                voice: brand::VOICE,
                cloud: brand::CLOUD.mix_oklab(brand::WHITE, 0.55),
                ok: brand::OK,
                err: brand::ERR,
                drop_shadows: true,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Icon {
    Cloud,
    Ok,
    Bang,
    Lock,
}

#[derive(Debug, Clone)]
enum ItemKind {
    Icon(Icon),
    Wave,
    Dots,
    Text { shaped: ShapedText, size: f32, color: Rgba },
}

/// A flex item, in device pixels.
#[derive(Debug, Clone)]
struct Item {
    kind: ItemKind,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Debug, Clone)]
struct Layout {
    /// The window, in device pixels.
    width: u32,
    height: u32,
    /// The pill's border box (unsnapped), for the transform origin.
    pill: (f32, f32, f32, f32),
    /// The pill as painted: snapped to device pixels.
    body: Rect,
    items: Vec<Item>,
    /// Device pixels per CSS pixel (window scale) and per pill CSS pixel (with `--scale`).
    dpr: f32,
    k: f32,
    /// The waveform canvas's CSS size (clientWidth/clientHeight).
    wave_css: (f32, f32),
}

/// Window size in device pixels for a scale factor.
pub fn window_size(dpr: f32) -> (u32, u32) {
    ((WINDOW_WIDTH * dpr).round().max(1.0) as u32, (WINDOW_HEIGHT * dpr).round().max(1.0) as u32)
}

fn layout(font: &Font, look: &PillLook, style: &PillStyle, palette: &Palette, dpr: f32) -> Layout {
    let (width, height) = window_size(dpr);
    let s = style.size_scale();
    let k = s * dpr;
    let minimal = style.minimal();
    let phase = look.phase;
    let pill_h = if minimal { 27.0 } else { 44.0 } * k;
    let pad = if minimal { 11.0 } else { 17.0 } * k;
    let gap = 9.0 * k;
    let icon = if minimal { 14.0 } else { 18.0 } * k;
    // canvas.clientWidth/clientHeight are whole CSS pixels.
    let wave_css = ((69.0 * s).round(), (if minimal { 13.0 } else { 21.0 } * s).round());

    let mut items: Vec<Item> = Vec::new();
    let mut push = |kind: ItemKind, w: f32, h: f32| items.push(Item { kind, x: 0.0, y: 0.0, w, h });
    if look.cloud {
        push(ItemKind::Icon(Icon::Cloud), icon, icon);
    }
    if phase == OverlayPhase::Listening && style.show_waveform {
        push(ItemKind::Wave, 69.0 * k, if minimal { 13.0 } else { 21.0 } * k);
    }
    if phase == OverlayPhase::Transcribing {
        push(ItemKind::Dots, 23.0 * k, 5.0 * k);
    }
    if phase == OverlayPhase::Done {
        push(ItemKind::Icon(Icon::Ok), icon, icon);
    }
    if phase == OverlayPhase::Error {
        push(ItemKind::Icon(Icon::Bang), icon, icon);
    }
    let label_shown = !look.label.is_empty()
        && (!minimal || matches!(phase, OverlayPhase::Error | OverlayPhase::Notice));
    let label_size = 14.0 * k;
    if label_shown {
        let shaped = font.shape(&look.label, &[]);
        let w = shaped.width(font, label_size);
        push(ItemKind::Text { shaped, size: label_size, color: palette.fg }, w, label_size);
    }
    if look.lock && !minimal {
        push(ItemKind::Icon(Icon::Lock), icon, icon);
    }
    if let Some(n) = look.queued.filter(|_| !minimal) {
        let size = 12.0 * k;
        let shaped = font.shape(&format!("+{n}"), &[tabular_nums()]);
        let w = shaped.width(font, size);
        push(ItemKind::Text { shaped, size, color: palette.dim }, w, size);
    }

    let content: f32 = items.iter().map(|i| i.w).sum::<f32>() + gap * items.len().saturating_sub(1) as f32;
    let mut pill_w = 2.0 * pad + content;
    // `max-width: calc(100vw - 16px)`: only the label (overflow hidden) shrinks.
    let max_w = (WINDOW_WIDTH - 16.0) * dpr;
    if pill_w > max_w
        && let Some(label) = items.iter_mut().find(|i| matches!(i.kind, ItemKind::Text { .. }) && label_shown)
    {
        let excess = pill_w - max_w;
        let room = (label.w - excess).max(0.0);
        if let ItemKind::Text { shaped, size, .. } = &mut label.kind {
            *shaped = font.ellipsize(shaped, *size, room);
        }
        label.w = room;
        pill_w = max_w;
    }

    let pill_x = (width as f32 - pill_w) / 2.0;
    let pill_y = (height as f32 - pill_h) / 2.0;
    let mut x = pill_x + pad;
    for item in &mut items {
        item.x = x;
        item.y = pill_y + (pill_h - item.h) / 2.0;
        x += item.w + gap;
    }
    let left = pill_x.round();
    let top = pill_y.round();
    let body = Rect::from_ltrb(left, top, (pill_x + pill_w).round(), (pill_y + pill_h).round())
        .unwrap_or_else(|| Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap_or(Rect::from_ltrb(0.0, 0.0, 1.0, 1.0).unwrap()));
    Layout { width, height, pill: (pill_x, pill_y, pill_w, pill_h), body, items, dpr, k, wave_css }
}

/// A rounded rectangle; `r` is clamped to half the shorter side.
fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let c = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    pb.close();
    pb.finish()
}

fn paint(color: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color.to_skia());
    p.anti_alias = true;
    p
}

/// `box-shadow: <offset-y> <blur> <spread> color` around the pill body.
fn draw_shadow(pixmap: &mut Pixmap, body: Rect, dpr: f32, dy: f32, blur_px: f32, spread: f32, color: Rgba) {
    let (w, h) = (pixmap.width(), pixmap.height());
    let grow = spread * dpr;
    let (x, y) = (body.left() - grow, body.top() - grow + dy * dpr);
    let (bw, bh) = (body.width() + 2.0 * grow, body.height() + 2.0 * grow);
    let radius = (body.height().min(body.width()) / 2.0 + grow).max(0.0);
    let Some(shape) = rounded_rect(x, y, bw, bh, radius) else { return };
    let Some(mut mask) = Mask::new(w, h) else { return };
    mask.fill_path(&shape, FillRule::Winding, true, Transform::identity());
    let mut values: Vec<f32> = mask.data().iter().map(|&v| v as f32 / 255.0).collect();
    // CSS: the shadow is a Gaussian with a standard deviation of half the blur radius.
    blur::gaussian(&mut values, w as usize, h as usize, blur_px * dpr / 2.0);
    for (dst, v) in mask.data_mut().iter_mut().zip(values) {
        *dst = (v * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    if let Some(rect) = Rect::from_xywh(0.0, 0.0, w as f32, h as f32) {
        let mut p = paint(color);
        p.anti_alias = false;
        pixmap.fill_rect(rect, &p, Transform::identity(), Some(&mask));
    }
}

fn icon_path(icon: Icon) -> &'static str {
    match icon {
        Icon::Cloud => svgpath::icons::CLOUD,
        Icon::Ok => svgpath::icons::OK,
        Icon::Bang => svgpath::icons::BANG,
        Icon::Lock => svgpath::icons::LOCK,
    }
}

fn draw_icon(pixmap: &mut Pixmap, icon: Icon, item: &Item, color: Rgba) {
    let Some(path) = svgpath::parse(icon_path(icon)) else { return };
    // The SVG box is painted at a whole device pixel.
    let ts = Transform::from_translate(item.x.round(), item.y.round()).pre_scale(item.w / 24.0, item.h / 24.0);
    let stroke = Stroke { width: 2.2, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
    pixmap.stroke_path(&path, &paint(color), &stroke, ts, None);
}

fn draw_text(pixmap: &mut Pixmap, font: &Font, item: &Item, layout: &Layout) {
    let ItemKind::Text { shaped, size, color } = &item.kind else { return };
    // The line box is `line-height: 1`; the baseline lands on a whole pixel.
    let baseline = (item.y + font.baseline_offset(*size, item.h)).round();
    let Some(line) = font.rasterize(shaped, *size, item.x, baseline, item.x + item.w + 0.5, TextLook::default())
    else {
        return;
    };
    // Paint the colour through the line's coverage.
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let Some(mut full) = Mask::new(w as u32, h as u32) else { return };
    let (mw, mh) = (line.mask.width() as i32, line.mask.height() as i32);
    let src = line.mask.data();
    let dst = full.data_mut();
    for my in 0..mh {
        let ty = line.y + my;
        if !(0..h).contains(&ty) {
            continue;
        }
        for mx in 0..mw {
            let tx = line.x + mx;
            if (0..w).contains(&tx) {
                dst[(ty * w + tx) as usize] = src[(my * mw + mx) as usize];
            }
        }
    }
    if let Some(rect) = Rect::from_xywh(line.x as f32, line.y as f32, mw as f32, mh as f32) {
        let mut p = paint(*color);
        p.anti_alias = false;
        pixmap.fill_rect(rect, &p, Transform::identity(), Some(&full));
    }
    let _ = layout;
}

/// Port of `drawWave`'s analysis: average the lower half of the spectrum into
/// 13 bands; fast attack, slow release keeps the bars lively without flicker.
pub fn update_levels(levels: &mut [f32; BARS], data: &[u8]) {
    // Speech energy sits in the low bins (0–4 kHz at a 16 kHz context).
    let usable = (data.len() as f64 * 0.5).floor() as usize;
    for (i, level) in levels.iter_mut().enumerate() {
        let from = ((i as f64 / BARS as f64) * usable as f64).floor() as usize;
        let to = (from + 1).max((((i + 1) as f64 / BARS as f64) * usable as f64).floor() as usize);
        let sum: u32 = data[from.min(data.len())..to.min(data.len())].iter().map(|&b| b as u32).sum();
        let target = (sum as f32 / (to - from) as f32 / 170.0).min(1.0);
        *level = if target > *level { target } else { *level * 0.82 + target * 0.18 };
    }
}

fn draw_wave(pixmap: &mut Pixmap, item: &Item, layout: &Layout, levels: &[f32; BARS], color: Rgba) {
    let (width, height) = layout.wave_css;
    let dpr = layout.dpr;
    // Thirteen bars, 3 px wide with a 2.5 px gap, in the voice colour: the site's pill.
    let gap = (width / 69.0) * 2.5;
    let bar = (width - gap * (BARS as f32 - 1.0)) / BARS as f32;
    let ox = item.x.round();
    let oy = item.y.round();
    let p = paint(color);
    let mut pb = PathBuilder::new();
    for i in 0..BARS {
        // Mirror around the centre so the shape reads as a voice, not a spectrum.
        let mirrored = levels[(i as f32 - (BARS as f32 - 1.0) / 2.0).abs() as usize];
        let h = (height * 0.14).max(mirrored * height);
        let x = i as f32 * (bar + gap);
        let r = bar.min(h) / 2.0;
        if let Some(path) = rounded_rect(ox + x * dpr, oy + (height - h) / 2.0 * dpr, bar * dpr, h * dpr, r * dpr) {
            pb.push_path(&path);
        }
    }
    if let Some(path) = pb.finish() {
        pixmap.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    }
}

/// The `dot` keyframes: opacity and lift of one dot `t` seconds into its animation.
fn dot_frame(t: f32) -> (f32, f32) {
    if t < 0.0 {
        // Waiting for its delay: the dot's own style (no fill mode).
        return (1.0, 0.0);
    }
    let p = (t % DOT_CYCLE) / DOT_CYCLE;
    let e = |u: f32| CubicBezier::EASE_IN_OUT.ease(u as f64) as f32;
    if p < 0.3 {
        let v = e(p / 0.3);
        (0.3 + 0.7 * v, v)
    } else if p < 0.6 {
        let v = e((p - 0.3) / 0.3);
        (1.0 - 0.7 * v, 1.0 - v)
    } else {
        (0.3, 0.0)
    }
}

fn draw_dots(pixmap: &mut Pixmap, item: &Item, layout: &Layout, time: f32, reduced: bool, color: Rgba) {
    let k = layout.k;
    let r = 2.5 * k;
    for (i, delay) in DOT_DELAYS.iter().enumerate() {
        let (opacity, lift) = if reduced { (0.8, 0.0) } else { dot_frame(time - delay) };
        let cx = item.x + i as f32 * 9.0 * k + r;
        let cy = item.y + r - lift * 3.0 * k;
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            pixmap.fill_path(&path, &paint(color.with_alpha(color.a * opacity)), FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// The moving parts of one frame.
#[derive(Debug, Clone, Copy)]
pub struct Motion<'a> {
    /// The waveform's bar levels (`levels` in main.ts), 0..1.
    pub levels: &'a [f32; BARS],
    /// Seconds since the dots appeared (the CSS animation's clock).
    pub dots_time: f32,
    /// The transform transition: 0 is `translateY(12px) scale(0.92)`, 1 is `none`.
    pub enter: f32,
}

struct Layer {
    look: PillLook,
    style: PillStyle,
    dpr: f32,
    layout: Layout,
    pixmap: Pixmap,
}

/// Draws the pill; keeps the static layer between frames.
pub struct PillRenderer {
    font: Option<Font>,
    layer: Option<Layer>,
    scratch: Option<Pixmap>,
}

impl Default for PillRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl PillRenderer {
    pub fn new() -> Self {
        let font = Font::load(PILL_WEIGHT);
        if font.is_none() {
            log::error!(target: "overlay", "the pill font failed to load; labels will be blank");
        }
        Self { font, layer: None, scratch: None }
    }

    /// Frees the cached layer and buffers (the window is hidden).
    pub fn release(&mut self) {
        self.layer = None;
        self.scratch = None;
    }

    fn ensure_layer(&mut self, look: &PillLook, style: &PillStyle, dpr: f32) -> Option<()> {
        let fresh = self.layer.as_ref().is_some_and(|l| l.look == *look && l.style == *style && l.dpr == dpr);
        if !fresh {
            let font = self.font.as_ref()?;
            let palette = Palette::for_style(style);
            let layout = layout(font, look, style, &palette, dpr);
            let mut pixmap = Pixmap::new(layout.width, layout.height)?;
            if style.style != OverlayStyle::None {
                paint_static(&mut pixmap, font, &layout, &palette);
            }
            self.layer = Some(Layer { look: look.clone(), style: *style, dpr, layout, pixmap });
        }
        self.layer.as_ref().map(|_| ())
    }

    /// Draws one frame into `out` (resized to the window if needed). `out` holds
    /// premultiplied RGBA; the caller applies the pill's opacity.
    pub fn render(&mut self, look: &PillLook, style: &PillStyle, dpr: f32, motion: &Motion, out: &mut Pixmap) {
        let (w, h) = window_size(dpr);
        if out.width() != w || out.height() != h {
            match Pixmap::new(w, h) {
                Some(p) => *out = p,
                None => return,
            }
        }
        let reduced = style.reduced_motion;
        if self.ensure_layer(look, style, dpr).is_none() {
            out.fill(tiny_skia::Color::TRANSPARENT);
            return;
        }
        let Self { layer, scratch, .. } = self;
        let Some(layer) = layer.as_ref() else { return };
        let layout = &layer.layout;
        let identity = motion.enter >= 1.0 || reduced;
        let palette = Palette::for_style(style);
        let target: &mut Pixmap = if identity {
            out.data_mut().copy_from_slice(layer.pixmap.data());
            &mut *out
        } else {
            let buffer = scratch.get_or_insert_with(|| layer.pixmap.clone());
            if buffer.width() != w || buffer.height() != h {
                *buffer = layer.pixmap.clone();
            } else {
                buffer.data_mut().copy_from_slice(layer.pixmap.data());
            }
            buffer
        };
        if style.style != OverlayStyle::None {
            for item in &layout.items {
                match item.kind {
                    ItemKind::Wave => draw_wave(target, item, layout, motion.levels, palette.voice),
                    ItemKind::Dots => draw_dots(target, item, layout, motion.dots_time, reduced, palette.fg),
                    _ => {}
                }
            }
        }
        if !identity {
            let p = motion.enter.clamp(0.0, 1.0);
            let (px, py, pw, ph) = layout.pill;
            let (cx, cy) = (px + pw / 2.0, py + ph / 2.0);
            let s = 0.92 + 0.08 * p;
            let ty = 12.0 * layout.dpr * (1.0 - p);
            let ts = Transform::from_translate(cx, cy + ty).pre_scale(s, s).pre_translate(-cx, -cy);
            out.fill(tiny_skia::Color::TRANSPARENT);
            if let Some(buffer) = scratch.as_ref() {
                let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
                out.draw_pixmap(0, 0, buffer.as_ref(), &paint, ts, None);
            }
        }
    }
}

/// Shadows, body, edge, icons and text: everything that does not move.
fn paint_static(pixmap: &mut Pixmap, font: &Font, layout: &Layout, palette: &Palette) {
    let dpr = layout.dpr;
    let body = layout.body;
    if palette.drop_shadows {
        // box-shadow: the first listed is on top, so paint the last first.
        draw_shadow(pixmap, body, dpr, 2.0, 6.0, 0.0, Rgba::new(0, 0, 0, 0.3));
        draw_shadow(pixmap, body, dpr, 10.0, 22.0, -8.0, Rgba::new(0, 0, 0, 0.5));
    }
    let radius = body.height().min(body.width()) / 2.0;
    if let Some(path) = rounded_rect(body.left(), body.top(), body.width(), body.height(), radius) {
        pixmap.fill_path(&path, &paint(palette.bg), FillRule::Winding, Transform::identity(), None);
    }
    // inset 0 0 0 <edge>px: a ring just inside the border.
    let e = palette.edge_width * dpr;
    let mut pb = PathBuilder::new();
    if let Some(outer) = rounded_rect(body.left(), body.top(), body.width(), body.height(), radius) {
        pb.push_path(&outer);
    }
    if let Some(inner) =
        rounded_rect(body.left() + e, body.top() + e, body.width() - 2.0 * e, body.height() - 2.0 * e, radius - e)
    {
        pb.push_path(&inner);
    }
    if let Some(ring) = pb.finish() {
        pixmap.fill_path(&ring, &paint(palette.edge), FillRule::EvenOdd, Transform::identity(), None);
    }
    for item in &layout.items {
        match &item.kind {
            ItemKind::Icon(icon) => {
                let color = match icon {
                    Icon::Cloud => palette.cloud,
                    Icon::Ok => palette.ok,
                    Icon::Bang => palette.err,
                    Icon::Lock => palette.dim,
                };
                draw_icon(pixmap, *icon, item, color);
            }
            ItemKind::Text { .. } => draw_text(pixmap, font, item, layout),
            ItemKind::Wave | ItemKind::Dots => {}
        }
    }
}

/// Multiplies a premultiplied pixmap by `opacity` (CSS `opacity` on the pill).
pub fn apply_opacity(pixmap: &mut Pixmap, opacity: f32) {
    if opacity >= 1.0 {
        return;
    }
    let o = opacity.clamp(0.0, 1.0);
    for v in pixmap.data_mut() {
        *v = (*v as f32 * o).round() as u8;
    }
}

/// Renders the 460 × 84 overlay (times `scale`) offscreen with the pill fully
/// shown: `state` as `applyState` would show it, `time` seconds into the phase
/// (the dots' clock) and the waveform at `levels` (13 values, 0..1; fewer are
/// padded with zeros). For tests and comparisons.
pub fn render_pill(
    state: &OverlayState,
    appearance: &OverlayAppearance,
    scale: f32,
    time: f32,
    levels: &[f32],
) -> Pixmap {
    let look = PillLook::from_state(state);
    let system = if appearance.theme.high_contrast { SystemColors::current() } else { SystemColors::default() };
    let style = PillStyle::from_appearance(appearance, system);
    let mut bars = [0.0f32; BARS];
    for (b, l) in bars.iter_mut().zip(levels) {
        *b = l.clamp(0.0, 1.0);
    }
    let (w, h) = window_size(scale);
    let mut out = Pixmap::new(w, h).unwrap_or_else(|| {
        // A 1 × 1 pixmap cannot fail; window_size never returns zero.
        Pixmap::new(1, 1).expect("1x1 pixmap")
    });
    let mut renderer = PillRenderer::new();
    renderer.render(&look, &style, scale, &Motion { levels: &bars, dots_time: time, enter: 1.0 }, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flow_core::types::ThemeInfo;

    fn appearance() -> OverlayAppearance {
        OverlayAppearance {
            style: OverlayStyle::Pill,
            size: OverlaySize::Medium,
            show_waveform: true,
            sounds: false,
            theme: ThemeInfo::default(),
        }
    }

    fn state(phase: OverlayPhase) -> OverlayState {
        OverlayState { phase, ..OverlayState::default() }
    }

    /// The opaque pill's horizontal extent on the centre row.
    fn pill_span(p: &Pixmap) -> (u32, u32) {
        let y = p.height() / 2;
        let row: Vec<u8> = (0..p.width()).map(|x| p.pixel(x, y).map_or(0, |c| c.alpha())).collect();
        let first = row.iter().position(|&a| a == 255).unwrap_or(0) as u32;
        let last = row.iter().rposition(|&a| a == 255).unwrap_or(0) as u32;
        (first, last)
    }

    #[test]
    fn apply_state_keeps_the_words_while_hiding() {
        let mut look = PillLook::default();
        look.apply(&OverlayState { phase: OverlayPhase::Listening, locked: true, queued: 2, ..Default::default() });
        assert_eq!(look.label, "Listening");
        assert!(look.lock);
        assert_eq!(look.queued, Some(2));
        look.apply(&state(OverlayPhase::Done));
        assert_eq!(look.label, "Typed");
        assert!(!look.lock);
        assert_eq!(look.queued, None);
        look.apply(&OverlayState::hidden());
        assert_eq!(look.phase, OverlayPhase::Hidden);
        assert_eq!(look.label, "Typed");
        look.apply(&OverlayState {
            phase: OverlayPhase::Error,
            message: Some("Groq rejected the API key".into()),
            ..Default::default()
        });
        assert_eq!(look.label, "Groq rejected the API key");
    }

    #[test]
    fn pill_widths_match_the_electron_screenshots() {
        // Pill border boxes measured in ../screenshots (100 % scale).
        let a = appearance();
        let cases = [
            (state(OverlayPhase::Listening), 178),
            (OverlayState { cloud: true, ..state(OverlayPhase::Transcribing) }, 180),
            (state(OverlayPhase::Done), 102),
            (
                OverlayState {
                    phase: OverlayPhase::Error,
                    message: Some("Groq rejected the API key".into()),
                    ..Default::default()
                },
                240,
            ),
            (
                OverlayState {
                    phase: OverlayPhase::Notice,
                    message: Some("No text field focused — copied to clipboard".into()),
                    ..Default::default()
                },
                332,
            ),
        ];
        for (s, expected) in cases {
            let p = render_pill(&s, &a, 1.0, 0.0, &[]);
            assert_eq!((p.width(), p.height()), (460, 84));
            let (first, last) = pill_span(&p);
            let width = last - first + 1;
            assert!((width as i32 - expected).abs() <= 2, "{:?}: {width} px, expected {expected}", s.phase);
        }
    }

    #[test]
    fn long_labels_are_cut_with_an_ellipsis() {
        let s = OverlayState {
            phase: OverlayPhase::Error,
            message: Some("The model could not be loaded because the file on disk is damaged; download it again".into()),
            ..Default::default()
        };
        let p = render_pill(&s, &appearance(), 1.0, 0.0, &[]);
        let (first, last) = pill_span(&p);
        assert!(last - first < 444, "{}", last - first + 1);
    }

    #[test]
    fn sizes_scale_the_pill() {
        let s = state(OverlayPhase::Done);
        let mut a = appearance();
        let medium = pill_span(&render_pill(&s, &a, 1.0, 0.0, &[]));
        a.size = OverlaySize::Large;
        let large = pill_span(&render_pill(&s, &a, 1.0, 0.0, &[]));
        assert!(large.1 - large.0 > medium.1 - medium.0);
        let hidpi = render_pill(&s, &appearance(), 1.5, 0.0, &[]);
        assert_eq!((hidpi.width(), hidpi.height()), (690, 126));
    }

    #[test]
    fn style_none_draws_nothing() {
        let mut a = appearance();
        a.style = OverlayStyle::None;
        let p = render_pill(&state(OverlayPhase::Listening), &a, 1.0, 0.0, &[0.5; 13]);
        assert!(p.data().iter().all(|&v| v == 0));
    }

    #[test]
    fn levels_attack_fast_and_release_slowly() {
        let mut levels = [0.0f32; BARS];
        let loud = [255u8; 256];
        update_levels(&mut levels, &loud);
        assert!(levels.iter().all(|&l| l == 1.0));
        let quiet = [0u8; 256];
        update_levels(&mut levels, &quiet);
        assert!(levels.iter().all(|&l| (l - 0.82).abs() < 1e-6));
    }

    #[test]
    fn dots_bounce_in_turn() {
        assert_eq!(dot_frame(-0.1), (1.0, 0.0));
        let (o, lift) = dot_frame(0.27);
        assert!((o - 1.0).abs() < 1e-3 && (lift - 1.0).abs() < 1e-3);
        assert_eq!(dot_frame(0.7), (0.3, 0.0));
        assert_eq!(dot_frame(0.0), (0.3, 0.0));
    }
}
