//! Inline messages and small status pieces: `Notice` (info / ok / warn /
//! danger, one optional action), `Badge`, `ProgressBar` and the `.spin`
//! loader. Port of `components/Notice.tsx`.

use egui::epaint::StrokeKind;
use egui::{
    Align, Color32, CornerRadius, Layout, Rect, Response, Sense, Shape, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};

use super::measure;
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, icons, motion, tokens};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Info,
    Ok,
    Warn,
    Danger,
}

impl Tone {
    fn icon(self) -> Icon {
        match self {
            Self::Info => Icon::Info,
            Self::Ok => Icon::CircleCheck,
            Self::Warn => Icon::TriangleAlert,
            Self::Danger => Icon::CircleAlert,
        }
    }
}

/// An inline message with one optional action. The text is selectable.
///
/// ```ignore
/// Notice::new(Tone::Danger, &error).show_with_action(ui, |ui| { IconButton::new("Dismiss", Icon::X, 14.0).small().show(ui) });
/// ```
pub struct Notice<'a> {
    tone: Tone,
    text: &'a str,
}

impl<'a> Notice<'a> {
    pub fn new(tone: Tone, text: &'a str) -> Self {
        Self { tone, text }
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        self.layout(ui, None::<fn(&mut Ui)>)
    }

    pub fn show_with_action<R>(self, ui: &mut Ui, action: impl FnOnce(&mut Ui) -> R) -> R {
        let mut out = None;
        self.layout(
            ui,
            Some(|ui: &mut Ui| {
                out = Some(action(ui));
            }),
        );
        out.expect("action ran")
    }

    fn layout(self, ui: &mut Ui, action: Option<impl FnOnce(&mut Ui)>) -> Response {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let (bg, fg) = match self.tone {
            Tone::Info => (t.inset, t.fg_2),
            Tone::Ok => (t.ok_bg, t.fg),
            Tone::Warn => (t.warn_bg, t.fg),
            Tone::Danger => (t.danger_bg, t.fg),
        };
        let icon_color = match self.tone {
            Tone::Info => t.fg_2,
            Tone::Ok => t.ok,
            Tone::Warn => t.warn,
            Tone::Danger => t.danger,
        };
        let id = ui.auto_id_with(("flow-notice", self.text));
        let slot = ui.painter().add(Shape::Noop);
        let width = ui.available_width();
        let top = ui.cursor().min;
        let action_size =
            if action.is_some() { measure::remembered(&ctx, id).unwrap_or(Vec2::ZERO) } else { Vec2::ZERO };
        let left = top.x + 1.0 + 12.0;
        let right = top.x + width - 1.0 - 12.0;
        let text_left = left + 16.0 + 10.0;
        let text_w = (right - text_left - if action.is_some() { 10.0 + action_size.x } else { 0.0 }).max(40.0);
        let content_top = top.y + 1.0 + 8.0;
        let galley = text::galley(ui, self.text, TextStyle::CAPTION, fg, text_w);
        let text_h = galley.size().y;
        icons::paint(
            ui,
            self.tone.icon(),
            Rect::from_min_size(pos2(left, content_top), Vec2::splat(16.0)),
            16.0,
            2.0,
            icon_color,
        );
        // The message can be selected and copied.
        let mut text_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(text_left, content_top), vec2(text_w, text_h)))
                .layout(Layout::top_down(Align::Min)),
        );
        text_ui.style_mut().interaction.selectable_labels = true;
        text_ui.add(egui::Label::new(galley).selectable(true));
        let mut inner_h = text_h.max(16.0);
        if let Some(action) = action {
            // `-my-0.5`: the action may stick 2 px out of the padding.
            let area = Rect::from_min_max(
                pos2(right - action_size.x, content_top - 2.0),
                pos2(right, content_top - 2.0 + action_size.y),
            );
            let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::left_to_right(Align::Min)));
            child.spacing_mut().item_spacing = vec2(8.0, 0.0);
            action(&mut child);
            measure::record(&ctx, id, child.min_rect().size());
            inner_h = inner_h.max(action_size.y - 4.0);
        }
        let rect = Rect::from_min_size(top, vec2(width, inner_h + 18.0));
        ui.painter().set(slot, notice_box(rect, bg, t.line));
        ui.allocate_rect(rect, Sense::hover())
    }
}

fn notice_box(rect: Rect, bg: Color32, line: Color32) -> Shape {
    Shape::Vec(vec![
        Shape::rect_filled(rect, CornerRadius::from(4.0), bg),
        Shape::rect_stroke(rect, CornerRadius::from(4.0), Stroke::new(1.0, line), StrokeKind::Inside),
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeTone {
    #[default]
    Neutral,
    Accent,
    Ok,
    Warn,
}

/// `.badge`: a 20 px pill, caption weight 600.
pub fn badge(ui: &mut Ui, label: &str, tone: BadgeTone) -> Response {
    let t = tokens(ui.ctx());
    let (bg, fg) = match tone {
        BadgeTone::Neutral => (t.subtle_hover, t.fg_2),
        BadgeTone::Accent => (t.accent_fill, t.on_accent),
        BadgeTone::Ok => (t.ok_bg, t.ok),
        BadgeTone::Warn => (t.warn_bg, t.warn),
    };
    let galley = text::galley(ui, label, TextStyle::CAPTION.weight(Weight::Semibold), fg, f32::INFINITY);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 20.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::from(10.0), bg);
    if let Some(border) = t.hc_border {
        painter.rect_stroke(rect, CornerRadius::from(10.0), Stroke::new(1.0, border), StrokeKind::Inside);
    }
    painter.galley(pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, fg);
    response
}

/// Determinate when `value` (0..1) is given, otherwise a looping bar. 4 px tall.
pub fn progress_bar(ui: &mut Ui, value: Option<f32>) -> Response {
    let t = tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, CornerRadius::from(2.0), t.subtle_hover);
    match value {
        Some(value) => {
            // `transition-[width] duration-200`
            let shown = motion::value(ui, response.id.with("flow-progress"), value.clamp(0.0, 1.0), 200.0);
            let fill = Rect::from_min_size(rect.min, vec2(rect.width() * shown, rect.height()));
            painter.rect_filled(fill, CornerRadius::from(2.0), t.accent_fill);
        }
        None => {
            // `flow-indeterminate 1.4s ease-in-out infinite`: a 40 % bar from -100 % to 250 %.
            let time = ui.input(|i| i.time);
            let phase = if motion::enabled(ui) { motion::ease_in_out(((time % 1.4) / 1.4) as f32) } else { 0.0 };
            let w = rect.width() * 0.4;
            let x = rect.left() + w * (-1.0 + 3.5 * phase);
            painter.rect_filled(
                Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height())),
                CornerRadius::from(2.0),
                t.accent_fill,
            );
            if motion::enabled(ui) {
                ui.ctx().request_repaint();
            }
        }
    }
    if let Some(border) = t.hc_border {
        painter.rect_stroke(rect, CornerRadius::from(2.0), Stroke::new(1.0, border), StrokeKind::Inside);
    }
    response
}

/// The `.spin` loader (`<LoaderCircle className="spin" />`): one turn per 900 ms.
pub fn spinner(ui: &mut Ui, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let angle = if motion::enabled(ui) {
        ui.ctx().request_repaint();
        (ui.input(|i| i.time) % 0.9 / 0.9) as f32 * std::f32::consts::TAU
    } else {
        0.0
    };
    icons::paint_rotated(ui, Icon::LoaderCircle, rect, size, 2.0, color, angle);
    response
}
