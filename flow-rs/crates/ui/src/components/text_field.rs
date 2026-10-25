//! Text inputs that hold a local draft and commit on blur or Enter (and, for
//! free text, after a 700 ms pause in typing). While focused, a draft is never
//! overwritten by a config reload. Port of `components/TextField.tsx` and the
//! `.input` rule.
//!
//! The draft lives in egui memory under the field's id, like React state:
//! give each field a stable `id_salt` (the config path is a good one).

use std::hash::Hash;
use std::sync::Arc;

use egui::epaint::StrokeKind;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, CornerRadius, EventFilter, Frame, Galley, Id, Key, Layout, Margin, Painter, Rect, Response, Sense,
    Stroke, TextBuffer, TextEdit, Ui, UiBuilder, pos2, vec2,
};

use crate::design::text::{self, TextStyle};
use crate::design::{Icon, icons, tokens};

const DEBOUNCE: f64 = 0.7;

/// How an `.input` frame is drawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct InputLook {
    pub hovered: bool,
    pub focused: bool,
    pub disabled: bool,
    pub invalid: bool,
}

/// Paints the `.input` box: 8 px radius, a 1 px border with a stronger bottom
/// edge; focus turns the bottom into a 2 px accent line.
pub fn paint_input_frame(painter: &Painter, rect: Rect, look: InputLook) {
    let t = tokens(painter.ctx());
    let r = CornerRadius::from(8.0);
    let fill = if look.disabled {
        t.ctl_disabled
    } else if look.focused {
        t.input_focus
    } else if look.hovered {
        t.ctl_hover
    } else {
        t.ctl
    };
    painter.rect_filled(rect, r, fill);
    painter.rect_stroke(rect, r, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
    let bottom = if look.disabled {
        t.ctl_line
    } else if look.invalid {
        t.danger
    } else if look.focused {
        t.accent_fill
    } else {
        t.input_line
    };
    // The bottom border owns the lower part of the rounded corners, as in CSS.
    let band =
        Rect::from_min_max(pos2(rect.left() - 1.0, rect.bottom() - 3.0), pos2(rect.right() + 1.0, rect.bottom() + 1.0));
    painter.with_clip_rect(band.intersect(painter.clip_rect())).rect_stroke(
        rect,
        r,
        Stroke::new(1.0, bottom),
        StrokeKind::Inside,
    );
    if !look.disabled && (look.focused || look.invalid) {
        // `box-shadow: inset 0 -1px 0 <colour>`.
        let band = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), pos2(rect.right(), rect.bottom() - 1.0));
        painter.with_clip_rect(band.intersect(painter.clip_rect())).rect_stroke(
            rect.shrink(1.0),
            CornerRadius::from(7.0),
            Stroke::new(1.0, bottom),
            StrokeKind::Inside,
        );
    }
}

fn layouter(style: TextStyle, color: Color32) -> impl FnMut(&Ui, &dyn TextBuffer, f32) -> Arc<Galley> {
    move |ui: &Ui, buffer: &dyn TextBuffer, wrap: f32| {
        let mut job = LayoutJob::single_section(buffer.as_str().to_owned(), style.format(color));
        job.wrap = TextWrapping { max_width: wrap, ..TextWrapping::default() };
        crate::design::text::snap_baselines(ui.painter().layout_job(job), style)
    }
}

struct EditOutput {
    response: Response,
    changed: bool,
    escape: bool,
    enter: bool,
}

/// Everything about one TextEdit inside an `.input` frame.
struct EditSpec<'a> {
    id: Id,
    rect: Rect,
    style: TextStyle,
    align: Align,
    multiline: bool,
    password: bool,
    placeholder: Option<&'a str>,
    enabled: bool,
    invalid: bool,
}

fn edit(ui: &mut Ui, spec: &EditSpec<'_>, text: &mut String) -> EditOutput {
    edit_padded(ui, spec, text, 10.0, 10.0)
}

/// [`edit`] with its own side padding (room for an icon or a clear button inside the box).
fn edit_padded(ui: &mut Ui, spec: &EditSpec<'_>, text: &mut String, left: f32, right: f32) -> EditOutput {
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let had_focus = ctx.memory(|m| m.has_focus(spec.id));
    let hovered = spec.enabled && ui.rect_contains_pointer(spec.rect);
    paint_input_frame(
        ui.painter(),
        spec.rect,
        InputLook { hovered, focused: had_focus, disabled: !spec.enabled, invalid: spec.invalid },
    );
    // Escape and Enter belong to the field (revert / commit), not to focus handling.
    let (escape, enter) = if had_focus {
        ui.memory_mut(|m| m.set_focus_lock_filter(spec.id, EventFilter { escape: true, ..EventFilter::default() }));
        ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter)))
    } else {
        (false, false)
    };
    let color = if spec.enabled { t.fg } else { t.fg_disabled };
    let inner = spec.rect.shrink(1.0);
    let pad_y = if spec.multiline { 5.0 } else { ((inner.height() - spec.style.line_height) / 2.0).max(0.0) };
    if text.is_empty()
        && let Some(placeholder) = spec.placeholder
    {
        let galley = text::galley(ui, placeholder, spec.style, t.fg_3, inner.width() - left - right);
        let x = match spec.align {
            Align::Max => inner.right() - right - galley.size().x,
            _ => inner.left() + left,
        };
        ui.painter().with_clip_rect(inner).galley(pos2(x, inner.top() + pad_y), galley, t.fg_3);
    }
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    let mut layout = layouter(spec.style, color);
    let editor = if spec.multiline { TextEdit::multiline(text) } else { TextEdit::singleline(text) };
    let output = editor
        .id(spec.id)
        // With an explicit frame egui takes the padding from it, not from `.margin()`.
        .frame(Frame::NONE.inner_margin(Margin {
            left: left as i8,
            right: right as i8,
            top: pad_y.floor() as i8,
            bottom: pad_y.ceil() as i8,
        }))
        .desired_width(inner.width() - left - right)
        .min_size(inner.size())
        .align(egui::Align2([spec.align, Align::Min]))
        .password(spec.password)
        .interactive(spec.enabled)
        .text_color(color)
        .layouter(&mut layout)
        .show(&mut child);
    let changed = output.response.changed();
    EditOutput { response: output.response.response, changed, escape, enter }
}

#[derive(Debug, Clone, Default)]
struct Draft {
    init: bool,
    draft: String,
    committed: String,
    focused: bool,
    deadline: Option<f64>,
    error: Option<String>,
}

/// What a text field reported.
pub struct FieldResponse<T> {
    pub response: Response,
    /// The value to save, when the draft was committed this frame.
    pub committed: Option<T>,
}

/// Returns an error message for a draft that must not be saved.
pub type Validator<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A single-line text box (`TextField`). Commits on blur, Enter or a pause.
///
/// ```ignore
/// if let Some(v) = TextField::new("paste_shortcut", &config.insert.paste_shortcut).show(ui).committed {
///     cx.config.set(path!["insert", "paste_shortcut"], v);
/// }
/// ```
pub struct TextField<'a> {
    id: Id,
    value: &'a str,
    width: f32,
    mono: bool,
    password: bool,
    placeholder: Option<&'a str>,
    disabled: bool,
    validate: Option<Validator<'a>>,
}

impl<'a> TextField<'a> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, value: &'a str) -> Self {
        Self {
            id: Id::unique(("flow-text-field", id_salt)),
            value,
            width: 200.0,
            mono: false,
            password: false,
            placeholder: None,
            disabled: false,
            validate: None,
        }
    }
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    /// `font-mono text-[13px]`
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }
    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Returns an error message for a draft that must not be saved.
    pub fn validate(mut self, validate: Validator<'a>) -> Self {
        self.validate = Some(validate);
        self
    }

    pub fn show(self, ui: &mut Ui) -> FieldResponse<String> {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let now = ctx.input(|i| i.time);
        let mut d: Draft = ctx.data(|m| m.get_temp(self.id)).unwrap_or_default();
        if !d.init || d.committed != self.value && !d.focused {
            d.draft = self.value.to_string();
        }
        if !d.init || d.committed != self.value {
            d.committed = self.value.to_string();
        }
        d.init = true;

        let style = if self.mono { TextStyle::BODY.mono().size(13.0, 20.0) } else { TextStyle::BODY };
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, 32.0), Sense::hover());
        let spec = EditSpec {
            id: self.id,
            rect,
            style,
            align: Align::Min,
            multiline: false,
            password: self.password,
            placeholder: self.placeholder,
            enabled: !self.disabled,
            invalid: d.error.is_some(),
        };
        let mut text = d.draft.clone();
        let out = edit(ui, &spec, &mut text);
        let mut committed = None;
        let validate = self.validate;
        let mut flush = |d: &mut Draft| {
            d.deadline = None;
            if d.draft != d.committed {
                let problem = validate.and_then(|v| v(&d.draft));
                d.error = problem.clone();
                if problem.is_none() {
                    d.committed = d.draft.clone();
                    committed = Some(d.draft.clone());
                }
            }
        };
        if out.changed {
            d.error = None;
            d.draft = text;
            d.deadline = Some(now + DEBOUNCE);
        }
        if out.escape && d.draft != self.value {
            d.error = None;
            d.deadline = None;
            d.draft = d.committed.clone();
        }
        if out.enter {
            flush(&mut d);
        }
        if out.response.gained_focus() {
            d.focused = true;
        }
        if out.response.lost_focus() || (d.focused && !out.response.has_focus()) {
            d.focused = false;
            flush(&mut d);
        }
        if let Some(deadline) = d.deadline {
            if now >= deadline {
                flush(&mut d);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(deadline - now));
            }
        }
        if let Some(error) = &d.error {
            ui.add_space(4.0);
            text::paragraph_max(ui, error, TextStyle::CAPTION, t.danger, self.width);
        }
        ctx.data_mut(|m| m.insert_temp(self.id, d));
        FieldResponse { response: out.response, committed }
    }
}

/// Whole numbers, clamped to the range the config accepts, with an optional
/// unit after the box ("seconds", "ms", "days"). Commits on blur or Enter.
pub struct NumberField<'a> {
    id: Id,
    value: i64,
    min: i64,
    max: i64,
    unit: Option<&'a str>,
    disabled: bool,
}

impl<'a> NumberField<'a> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, value: i64, range: std::ops::RangeInclusive<i64>) -> Self {
        Self {
            id: Id::unique(("flow-number-field", id_salt)),
            value,
            min: *range.start(),
            max: *range.end(),
            unit: None,
            disabled: false,
        }
    }
    pub fn unit(mut self, unit: &'a str) -> Self {
        self.unit = Some(unit);
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn show(self, ui: &mut Ui) -> FieldResponse<i64> {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let value_text = self.value.to_string();
        let mut d: Draft = ctx.data(|m| m.get_temp(self.id)).unwrap_or_default();
        if !d.init || d.committed != value_text && !d.focused {
            d.draft = value_text.clone();
        }
        if !d.init || d.committed != value_text {
            d.committed = value_text.clone();
        }
        d.init = true;

        let unit = self.unit.map(|u| text::galley(ui, u, TextStyle::BODY, t.fg_2, f32::INFINITY));
        let width = 88.0 + unit.as_ref().map(|u| 8.0 + u.size().x).unwrap_or(0.0);
        let (outer, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
        let rect = Rect::from_min_size(outer.min, vec2(88.0, 32.0));
        if let Some(unit) = unit {
            ui.painter().galley(pos2(rect.right() + 8.0, outer.center().y - unit.size().y / 2.0), unit, t.fg_2);
        }
        let spec = EditSpec {
            id: self.id,
            rect,
            style: TextStyle::BODY,
            align: Align::Max,
            multiline: false,
            password: false,
            placeholder: None,
            enabled: !self.disabled,
            invalid: false,
        };
        let mut text = d.draft.clone();
        let out = edit(ui, &spec, &mut text);
        let (min, max, value) = (self.min, self.max, self.value);
        let mut committed = None;
        // Shows what was actually saved: the clamped number, or the old value for junk.
        let mut settle = |d: &mut Draft| {
            let parsed = d.draft.parse::<i64>().ok();
            if let Some(parsed) = parsed {
                let clamped = parsed.clamp(min, max);
                if clamped != value {
                    committed = Some(clamped);
                }
                d.committed = clamped.to_string();
                d.draft = clamped.to_string();
            } else {
                d.draft = value.to_string();
            }
        };
        if out.changed {
            d.draft = text.chars().filter(|c| c.is_ascii_digit() || *c == '-').collect();
        }
        if out.escape {
            d.draft = d.committed.clone();
        }
        if out.enter {
            settle(&mut d);
        }
        if out.response.gained_focus() {
            d.focused = true;
        }
        if out.response.lost_focus() || (d.focused && !out.response.has_focus()) {
            d.focused = false;
            settle(&mut d);
        }
        ctx.data_mut(|m| m.insert_temp(self.id, d));
        FieldResponse { response: out.response, committed }
    }
}

/// A multi-line text box (`TextArea`): `rows` lines tall, commits like [`TextField`].
pub struct TextArea<'a> {
    id: Id,
    value: &'a str,
    rows: usize,
    placeholder: Option<&'a str>,
}

impl<'a> TextArea<'a> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, value: &'a str) -> Self {
        Self { id: Id::unique(("flow-text-area", id_salt)), value, rows: 3, placeholder: None }
    }
    pub fn rows(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }
    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    pub fn show(self, ui: &mut Ui) -> FieldResponse<String> {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let mut d: Draft = ctx.data(|m| m.get_temp(self.id)).unwrap_or_default();
        if !d.init || d.committed != self.value && !d.focused {
            d.draft = self.value.to_string();
        }
        if !d.init || d.committed != self.value {
            d.committed = self.value.to_string();
        }
        d.init = true;
        let height = self.rows as f32 * 20.0 + 12.0 + 2.0;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let spec = EditSpec {
            id: self.id,
            rect,
            style: TextStyle::BODY,
            align: Align::Min,
            multiline: true,
            password: false,
            placeholder: self.placeholder,
            enabled: true,
            invalid: false,
        };
        let mut text = d.draft.clone();
        let out = edit(ui, &spec, &mut text);
        let mut committed = None;
        let mut flush = |d: &mut Draft| {
            d.deadline = None;
            if d.draft != d.committed {
                d.committed = d.draft.clone();
                committed = Some(d.draft.clone());
            }
        };
        if out.changed {
            d.draft = text;
            d.deadline = Some(now + DEBOUNCE);
        }
        if out.response.gained_focus() {
            d.focused = true;
        }
        if out.response.lost_focus() || (d.focused && !out.response.has_focus()) {
            d.focused = false;
            flush(&mut d);
        }
        if let Some(deadline) = d.deadline {
            if now >= deadline {
                flush(&mut d);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(deadline - now));
            }
        }
        ctx.data_mut(|m| m.insert_temp(self.id, d));
        FieldResponse { response: out.response, committed }
    }
}

/// A bare `.input` text box for local state that is not a setting (search
/// boxes, API key entry). Edits `text` in place.
pub fn input(
    ui: &mut Ui,
    id_salt: impl Hash + std::fmt::Debug,
    text: &mut String,
    width: f32,
    placeholder: Option<&str>,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    let spec = EditSpec {
        id: Id::unique(("flow-input", id_salt)),
        rect,
        style: TextStyle::BODY,
        align: Align::Min,
        multiline: false,
        password: false,
        placeholder,
        enabled: true,
        invalid: false,
    };
    edit(ui, &spec, text).response
}

/// A search box: `<input type="search">` in an `.input` with a leading 15 px
/// `Search` icon (`gap-2`). As in Chromium, Escape clears it (focus stays),
/// and a 10 px clear button shows while it has text and is hovered or focused.
pub fn search_input(
    ui: &mut Ui,
    id_salt: impl Hash + std::fmt::Debug,
    text: &mut String,
    width: f32,
    placeholder: Option<&str>,
) -> Response {
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let id = Id::unique(("flow-input", id_salt));
    if ctx.memory(|m| m.had_focus_last_frame(id)) && ctx.input(|i| i.key_pressed(Key::Escape)) {
        text.clear();
        ctx.memory_mut(|m| m.request_focus(id));
    }
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    let clear_shown =
        |ui: &Ui, text: &str| !text.is_empty() && (ctx.memory(|m| m.has_focus(id)) || ui.rect_contains_pointer(rect));
    let clear = clear_shown(ui, text);
    let spec = EditSpec {
        id,
        rect,
        style: TextStyle::BODY,
        align: Align::Min,
        multiline: false,
        password: false,
        placeholder,
        enabled: true,
        invalid: false,
    };
    // The clear button takes 10 px plus `margin-inline-start: 1px` from the text.
    let right = if clear { 10.0 + 1.0 + 10.0 } else { 10.0 };
    let mut out = edit_padded(ui, &spec, text, 10.0 + 15.0 + 8.0, right);
    let inner = rect.shrink(1.0);
    let icon = Rect::from_min_size(pos2(inner.left() + 10.0, inner.center().y - 7.5), egui::Vec2::splat(15.0));
    icons::paint(ui, Icon::Search, icon, 15.0, 2.0, t.fg_2);
    if clear {
        let button = Rect::from_min_size(pos2(inner.right() - 20.0, inner.center().y - 5.0), egui::Vec2::splat(10.0));
        let response = ui.interact(button, id.with("clear"), Sense::click());
        let stroke = Stroke::new(1.5, if response.hovered() { t.fg } else { t.fg_2 });
        let cross = button.shrink(1.0);
        ui.painter().line_segment([cross.left_top(), cross.right_bottom()], stroke);
        ui.painter().line_segment([cross.right_top(), cross.left_bottom()], stroke);
        if response.clicked() {
            text.clear();
            ctx.memory_mut(|m| m.request_focus(id));
            out.response.mark_changed();
        }
    }
    if clear_shown(ui, text) != clear {
        ctx.request_repaint();
    }
    out.response
}
