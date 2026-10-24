//! Page scaffolding: the page title, a titled group of setting cards, the card
//! itself, and the label / explanation / control row used on every page.
//! Port of `components/Card.tsx`.
//!
//! ```ignore
//! page_header(ui, "General", None);
//! Group::new("App").show(ui, |ui| {
//!     SettingRow::new("Launch at login", "Start Flow in the system tray when you sign in to Windows.")
//!         .show(ui, |ui| Toggle::new(on).show(ui));
//! });
//! ```

use std::borrow::Cow;

use egui::{
    Align, Id, Layout, Margin, Rect, Response, Sense, Shape, Ui, UiBuilder, Vec2, pos2, vec2,
};

use super::measure;
use crate::design::text::{self, TextStyle};
use crate::design::{Weight, shadow, tokens};

/// The page title (`h1`, title size, weight 840) and an optional lead paragraph.
pub fn page_header(ui: &mut Ui, title: &str, lead: Option<&str>) {
    let t = tokens(ui.ctx());
    text::paragraph(
        ui,
        title,
        TextStyle::TITLE.weight(Weight::Black).heading(),
        t.fg,
    );
    if let Some(lead) = lead {
        ui.add_space(4.0);
        let max = 72.0 * text::ch(ui, TextStyle::BODY);
        text::paragraph_max(ui, lead, TextStyle::BODY, t.fg_2, max);
    }
    ui.add_space(24.0);
}

/// [`page_header`] with a lead built from UI (links, code).
pub fn page_header_with(ui: &mut Ui, title: &str, lead: impl FnOnce(&mut Ui)) {
    let t = tokens(ui.ctx());
    text::paragraph(
        ui,
        title,
        TextStyle::TITLE.weight(Weight::Black).heading(),
        t.fg,
    );
    ui.add_space(4.0);
    let max = 72.0 * text::ch(ui, TextStyle::BODY);
    let width = ui.available_width().min(max);
    ui.scope_builder(
        UiBuilder::new().max_rect(Rect::from_min_size(
            ui.cursor().min,
            vec2(width, f32::INFINITY),
        )),
        lead,
    );
    ui.add_space(24.0);
}

/// Deferred UI: a closure that draws into a `Ui` once.
type UiFn<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

/// A titled group of cards (`<section>`): heading, optional description, an
/// optional action aligned with the heading, then the cards 3 px apart.
pub struct Group<'a> {
    title: Cow<'a, str>,
    description: Option<Cow<'a, str>>,
    action: Option<UiFn<'a>>,
}

impl<'a> Group<'a> {
    pub fn new(title: impl Into<Cow<'a, str>>) -> Self {
        Self {
            title: title.into(),
            description: None,
            action: None,
        }
    }

    pub fn description(mut self, description: impl Into<Cow<'a, str>>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// A button aligned with the heading, e.g. "Add word".
    pub fn action(mut self, action: impl FnOnce(&mut Ui) + 'a) -> Self {
        self.action = Some(Box::new(action));
        self
    }

    pub fn show<R>(self, ui: &mut Ui, cards: impl FnOnce(&mut Ui) -> R) -> R {
        let t = tokens(ui.ctx());
        let id = ui.auto_id_with(("flow-group", self.title.as_ref()));
        let width = ui.available_width();
        let top = ui.cursor().min;
        let action_size = match self.action {
            Some(_) => measure::remembered(ui.ctx(), id.with("action")).unwrap_or(Vec2::ZERO),
            None => Vec2::ZERO,
        };
        let text_w = if self.action.is_some() {
            width - 16.0 - action_size.x
        } else {
            width
        }
        .max(40.0);
        let title = text::galley(
            ui,
            self.title.as_ref(),
            TextStyle::BODY.weight(Weight::Semibold).heading(),
            t.fg,
            text_w,
        );
        let description = self.description.as_ref().map(|d| {
            let max = 78.0 * text::ch(ui, TextStyle::CAPTION);
            text::galley(ui, d.as_ref(), TextStyle::CAPTION, t.fg_2, text_w.min(max))
        });
        let text_h = title.size().y
            + description
                .as_ref()
                .map(|d| 2.0 + d.size().y)
                .unwrap_or(0.0);
        // `min-h-8 items-end`: both sides sit on the bottom of the header.
        let header_h = text_h.max(32.0).max(action_size.y);
        let text_top = top.y + header_h - text_h;
        ui.painter()
            .galley(pos2(top.x, text_top), title.clone(), t.fg);
        if let Some(description) = description {
            ui.painter().galley(
                pos2(top.x, text_top + title.size().y + 2.0),
                description,
                t.fg_2,
            );
        }
        if let Some(action) = self.action {
            let area = Rect::from_min_max(
                pos2(top.x + width - action_size.x, top.y),
                pos2(top.x + width, top.y + header_h),
            );
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(area)
                    .layout(Layout::left_to_right(Align::Max)),
            );
            child.spacing_mut().item_spacing = vec2(8.0, 0.0);
            action(&mut child);
            measure::record(ui.ctx(), id.with("action"), child.min_rect().size());
        }
        ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, header_h)));
        ui.add_space(8.0);
        let inner = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
                cards(ui)
            })
            .inner;
        ui.add_space(32.0);
        inner
    }
}

/// A `.card`: the card surface (10 px radius, `--shadow-card`) behind whatever
/// `content` draws, with `padding` inside. Full width.
pub fn card<R>(ui: &mut Ui, padding: Margin, content: impl FnOnce(&mut Ui) -> R) -> (R, Response) {
    let t = tokens(ui.ctx());
    let slot = ui.painter().add(Shape::Noop);
    let width = ui.available_width();
    let top = ui.cursor().min;
    let inner_rect = Rect::from_min_size(
        pos2(top.x + padding.leftf(), top.y + padding.topf()),
        vec2(width - padding.sum().x, f32::INFINITY),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(inner_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    let inner = content(&mut child);
    let height = child.min_rect().height().max(0.0) + padding.sum().y;
    let rect = Rect::from_min_size(top, vec2(width, height));
    ui.painter().set(
        slot,
        shadow::surface(rect, 10.0, t.card, &t.shadow_card, t.hc_border),
    );
    let response = ui.allocate_rect(rect, Sense::hover());
    (inner, response)
}

/// What a [`SettingRow`] explains under its label.
enum Description<'a> {
    Text(Cow<'a, str>),
    Ui(UiFn<'a>),
}

/// One setting: label, a one-line explanation beneath it, and the control on
/// the right, vertically centred (`min-h-10 items-center gap-6`). Optional
/// children render full-width under the row (meters, errors).
pub struct SettingRow<'a> {
    label: Cow<'a, str>,
    description: Description<'a>,
    disabled: bool,
    id_salt: Option<Id>,
}

impl<'a> SettingRow<'a> {
    pub fn new(label: impl Into<Cow<'a, str>>, description: impl Into<Cow<'a, str>>) -> Self {
        Self {
            label: label.into(),
            description: Description::Text(description.into()),
            disabled: false,
            id_salt: None,
        }
    }

    /// A description built from UI (text plus a path line, say). Draw it at caption size.
    pub fn new_with(
        label: impl Into<Cow<'a, str>>,
        description: impl FnOnce(&mut Ui) + 'a,
    ) -> Self {
        Self {
            label: label.into(),
            description: Description::Ui(Box::new(description)),
            disabled: false,
            id_salt: None,
        }
    }

    /// Greys the label (`text-fg-3`), as the TSX does next to a disabled control.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Needed only when two rows on a page share a label.
    pub fn id_salt(mut self, salt: impl std::hash::Hash + std::fmt::Debug) -> Self {
        self.id_salt = Some(Id::unique(("flow-row", salt)));
        self
    }

    /// The row with a control. The control closure lays out left to right with
    /// 8 px gaps (`flex items-center gap-2`), right-aligned in the card.
    pub fn show<R>(self, ui: &mut Ui, control: impl FnOnce(&mut Ui) -> R) -> R {
        self.layout(ui, Some(control), None::<fn(&mut Ui)>)
            .expect("control given")
    }

    /// The row with a control and children under it.
    pub fn show_with<R>(
        self,
        ui: &mut Ui,
        control: impl FnOnce(&mut Ui) -> R,
        children: impl FnOnce(&mut Ui),
    ) -> R {
        self.layout(ui, Some(control), Some(children))
            .expect("control given")
    }

    /// A row without a control, with children under the text.
    pub fn show_children(self, ui: &mut Ui, children: impl FnOnce(&mut Ui)) {
        self.layout(ui, None::<fn(&mut Ui)>, Some(children));
    }

    /// A row with only its label and description.
    pub fn show_plain(self, ui: &mut Ui) {
        self.layout(ui, None::<fn(&mut Ui)>, None::<fn(&mut Ui)>);
    }

    fn layout<R>(
        self,
        ui: &mut Ui,
        control: Option<impl FnOnce(&mut Ui) -> R>,
        children: Option<impl FnOnce(&mut Ui)>,
    ) -> Option<R> {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let id = self
            .id_salt
            .unwrap_or_else(|| ui.auto_id_with(("flow-row", self.label.as_ref())));
        let slot = ui.painter().add(Shape::Noop);
        let width = ui.available_width();
        let top = ui.cursor().min;
        let left = top.x + 16.0;
        let right = top.x + width - 16.0;
        let content_top = top.y + 12.0;

        let control_size = if control.is_some() {
            measure::remembered(&ctx, id.with("control")).unwrap_or(Vec2::ZERO)
        } else {
            Vec2::ZERO
        };
        let text_w = (right
            - left
            - if control.is_some() {
                24.0 + control_size.x
            } else {
                0.0
            })
        .max(40.0);
        let label_color = if self.disabled { t.fg_3 } else { t.fg };
        let label = text::galley(
            ui,
            self.label.as_ref(),
            TextStyle::BODY,
            label_color,
            text_w,
        );
        let (description_h, description) = match self.description {
            Description::Text(text) => {
                let g = text::galley(ui, text.as_ref(), TextStyle::CAPTION, t.fg_2, text_w);
                (g.size().y, Some(Ok(g)))
            }
            Description::Ui(f) => (
                measure::remembered(&ctx, id.with("description"))
                    .map(|s| s.y)
                    .unwrap_or(16.0),
                Some(Err(f)),
            ),
        };
        let text_h = label.size().y + 2.0 + description_h;
        let row_h = text_h.max(40.0).max(control_size.y);
        let text_top = content_top + (row_h - text_h) / 2.0;
        let label_h = label.size().y;
        ui.painter()
            .galley(pos2(left, text_top), label, label_color);
        match description {
            Some(Ok(galley)) => {
                ui.painter()
                    .galley(pos2(left, text_top + label_h + 2.0), galley, t.fg_2)
            }
            Some(Err(f)) => {
                let area = Rect::from_min_size(
                    pos2(left, text_top + label_h + 2.0),
                    vec2(text_w, f32::INFINITY),
                );
                let mut child = ui.new_child(
                    UiBuilder::new()
                        .max_rect(area)
                        .layout(Layout::top_down(Align::Min)),
                );
                f(&mut child);
                measure::record(
                    &ctx,
                    id.with("description"),
                    vec2(text_w, child.min_rect().height()),
                );
            }
            None => {}
        }

        let result = control.map(|control| {
            let area = Rect::from_min_max(
                pos2(right - control_size.x, content_top),
                pos2(right, content_top + row_h),
            );
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(area)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            child.spacing_mut().item_spacing = vec2(8.0, 0.0);
            let r = control(&mut child);
            measure::record(&ctx, id.with("control"), child.min_rect().size());
            r
        });

        let mut bottom = content_top + row_h;
        if let Some(children) = children {
            let area = Rect::from_min_size(pos2(left, bottom), vec2(right - left, f32::INFINITY));
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(area)
                    .layout(Layout::top_down(Align::Min)),
            );
            children(&mut child);
            bottom += child.min_rect().height().max(0.0);
        }
        let rect = Rect::from_min_max(top, pos2(top.x + width, bottom + 12.0));
        ui.painter().set(
            slot,
            shadow::surface(rect, 10.0, t.card, &t.shadow_card, t.hc_border),
        );
        ui.advance_cursor_after_rect(rect);
        result
    }
}

/// One sentence and one action, in a card (`px-4 py-4`).
pub fn empty_state(ui: &mut Ui, sentence: &str, action: impl FnOnce(&mut Ui)) {
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let id = ui.auto_id_with(("flow-empty", sentence));
    let slot = ui.painter().add(Shape::Noop);
    let width = ui.available_width();
    let top = ui.cursor().min;
    let (left, right) = (top.x + 16.0, top.x + width - 16.0);
    let action_size = measure::remembered(&ctx, id).unwrap_or(Vec2::ZERO);
    let text_w = (right - left - 24.0 - action_size.x).max(40.0);
    let galley = text::galley(ui, sentence, TextStyle::BODY, t.fg_2, text_w);
    let row_h = galley.size().y.max(action_size.y);
    ui.painter().galley(
        pos2(left, top.y + 16.0 + (row_h - galley.size().y) / 2.0),
        galley,
        t.fg_2,
    );
    let area = Rect::from_min_max(
        pos2(right - action_size.x, top.y + 16.0),
        pos2(right, top.y + 16.0 + row_h),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(area)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.spacing_mut().item_spacing = vec2(8.0, 0.0);
    action(&mut child);
    measure::record(&ctx, id, child.min_rect().size());
    let rect = Rect::from_min_size(top, vec2(width, row_h + 32.0));
    ui.painter().set(
        slot,
        shadow::surface(rect, 10.0, t.card, &t.shadow_card, t.hc_border),
    );
    ui.advance_cursor_after_rect(rect);
}
