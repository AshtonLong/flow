//! Modal dialogs over `.dialog-smoke`: a yes/no `ConfirmDialog` and an editor
//! `Modal` whose only action is Done. Port of `components/Dialog.tsx` and the
//! `.dialog` rule (12 px radius, `--shadow-dialog`, `--dialog-width`).
//!
//! Keep the open flag in the page struct and call `show` every frame:
//!
//! ```ignore
//! if ConfirmDialog::new("remove", "Remove this model?", &text, "Remove model").show(ui.ctx(), &mut self.confirm_remove) {
//!     cx.link.send(Request::ModelsRemove(id));
//! }
//! ```

use std::hash::Hash;

use egui::{
    Align, Area, Context, CornerRadius, Id, Key, Layout, Order, Rect, ScrollArea, Sense, Ui,
    UiBuilder, Vec2, pos2, vec2,
};

use super::button::{Button, IconButton};
use super::measure;
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, shadow, tokens};

struct Frame {
    id: Id,
    width: f32,
    /// Radix `Dialog` closes on a press outside; `AlertDialog` does not.
    close_outside: bool,
}

/// The shared dialog shell: smoke, the centred surface, header / body / footer.
fn dialog<R>(
    ctx: &Context,
    frame: Frame,
    open: &mut bool,
    header: impl FnOnce(&mut Ui, &mut bool),
    body: impl FnOnce(&mut Ui) -> R,
    footer: impl FnOnce(&mut Ui, &mut bool),
) -> Option<R> {
    if !*open {
        return None;
    }
    let t = tokens(ctx);
    let screen = ctx.content_rect();
    let width = (screen.width() - 48.0).min(frame.width);
    let max_h = screen.height() - 64.0;
    let natural = measure::remembered(ctx, frame.id.with("size"))
        .map(|s| s.y)
        .unwrap_or(200.0);
    let height = natural.min(max_h);
    let rect = Rect::from_center_size(screen.center(), vec2(width, height)).round();
    let mut result = None;
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        *open = false;
    }
    Area::new(frame.id)
        .order(Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, t.smoke);
            let backdrop = ui.allocate_rect(screen, Sense::click());
            let pressed_outside = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
            });
            if frame.close_outside && pressed_outside && backdrop.hovered() {
                *open = false;
            }
            shadow::paint_surface(
                ui.painter(),
                rect,
                12.0,
                t.flyout,
                &t.shadow_dialog,
                t.hc_border,
            );
            let clip = rect;

            // Header.
            let mut header_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(Rect::from_min_size(rect.min, vec2(width, f32::INFINITY)))
                    .layout(Layout::top_down(Align::Min)),
            );
            header_ui.set_clip_rect(clip);
            header(&mut header_ui, open);
            let header_h = header_ui.min_rect().height();

            // Footer, measured from last pass so the body knows its room.
            let footer_h = measure::remembered(ctx, frame.id.with("footer"))
                .map(|s| s.y)
                .unwrap_or(65.0);
            let body_max = (rect.height() - header_h - footer_h).max(0.0);
            let body_rect = Rect::from_min_size(
                pos2(rect.left(), rect.top() + header_h),
                vec2(width, body_max),
            );
            let mut body_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            body_ui.set_clip_rect(clip);
            let mut body_h = 0.0;
            ScrollArea::vertical()
                .id_salt(frame.id.with("body"))
                .max_height(body_max)
                .auto_shrink([false, true])
                .show(&mut body_ui, |ui| {
                    let start = ui.cursor().min.y;
                    result = Some(body(ui));
                    body_h = ui.cursor().min.y - start;
                });
            let body_shown = body_h.min(body_max);

            let footer_top = rect.top() + header_h + body_shown;
            let mut footer_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(Rect::from_min_size(
                        pos2(rect.left(), footer_top),
                        vec2(width, f32::INFINITY),
                    ))
                    .layout(Layout::top_down(Align::Min)),
            );
            footer_ui.set_clip_rect(clip);
            let footer_slot = footer_ui.painter().add(egui::Shape::Noop);
            footer(&mut footer_ui, open);
            let footer_rect = Rect::from_min_max(
                pos2(rect.left(), footer_top),
                pos2(rect.right(), footer_ui.min_rect().bottom()),
            );
            // `border-t border-line bg-inset`, inside the rounded bottom.
            footer_ui.painter().set(
                footer_slot,
                egui::Shape::Vec(vec![
                    egui::Shape::rect_filled(
                        footer_rect,
                        CornerRadius {
                            nw: 0,
                            ne: 0,
                            sw: 12,
                            se: 12,
                        },
                        t.inset,
                    ),
                    egui::Shape::line_segment(
                        [
                            pos2(footer_rect.left(), footer_rect.top() + 0.5),
                            pos2(footer_rect.right(), footer_rect.top() + 0.5),
                        ],
                        egui::Stroke::new(1.0, t.line),
                    ),
                ]),
            );
            measure::record(ctx, frame.id.with("footer"), footer_rect.size());
            measure::record(
                ctx,
                frame.id.with("size"),
                vec2(width, header_h + body_h + footer_rect.height()),
            );
        });
    result
}

/// The footer row: buttons right-aligned, 8 px apart, `px-6 py-4`.
fn footer_row(ui: &mut Ui, buttons: impl FnOnce(&mut Ui)) {
    ui.add_space(16.0);
    let width = ui.available_width();
    let row = Rect::from_min_size(ui.cursor().min + vec2(24.0, 0.0), vec2(width - 48.0, 32.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::right_to_left(Align::Center)),
    );
    child.spacing_mut().item_spacing = vec2(8.0, 0.0);
    buttons(&mut child);
    ui.advance_cursor_after_rect(row);
    ui.add_space(16.0);
}

/// A yes/no question before something is removed. Esc and Cancel both back out.
pub struct ConfirmDialog<'a> {
    id: Id,
    title: &'a str,
    description: &'a str,
    confirm_label: &'a str,
}

impl<'a> ConfirmDialog<'a> {
    /// `confirm_label` names the action: "Remove model", not "OK".
    pub fn new(
        id_salt: impl Hash + std::fmt::Debug,
        title: &'a str,
        description: &'a str,
        confirm_label: &'a str,
    ) -> Self {
        Self {
            id: Id::unique(("flow-confirm", id_salt)),
            title,
            description,
            confirm_label,
        }
    }

    /// Shows the dialog while `*open`. Returns true when confirmed (it then closes).
    pub fn show(self, ctx: &Context, open: &mut bool) -> bool {
        let mut confirmed = false;
        let (title, description, confirm_label) =
            (self.title, self.description, self.confirm_label);
        dialog(
            ctx,
            Frame {
                id: self.id,
                width: 440.0,
                close_outside: false,
            },
            open,
            |ui, _| {
                let t = tokens(ui.ctx());
                ui.add_space(24.0);
                inset(ui, 24.0, |ui| {
                    text::paragraph(
                        ui,
                        title,
                        TextStyle::SUBTITLE.weight(Weight::Heavy).heading(),
                        t.fg,
                    );
                    ui.add_space(8.0);
                    text::paragraph(ui, description, TextStyle::BODY, t.fg_2);
                });
                ui.add_space(20.0);
            },
            |_| {},
            |ui, open| {
                footer_row(ui, |ui| {
                    // Right to left: Cancel is drawn first so it ends up on the right.
                    if Button::new("Cancel").min_width(120.0).show(ui).clicked() {
                        *open = false;
                    }
                    if Button::new(confirm_label)
                        .accent()
                        .min_width(120.0)
                        .show(ui)
                        .clicked()
                    {
                        confirmed = true;
                        *open = false;
                    }
                });
            },
        );
        confirmed
    }
}

/// A modal editor. Changes inside apply as they are made, so the only action is Done.
pub struct Modal<'a> {
    id: Id,
    title: &'a str,
    description: Option<&'a str>,
    width: f32,
}

impl<'a> Modal<'a> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, title: &'a str) -> Self {
        Self {
            id: Id::unique(("flow-modal", id_salt)),
            title,
            description: None,
            width: 560.0,
        }
    }
    pub fn description(mut self, description: &'a str) -> Self {
        self.description = Some(description);
        self
    }
    /// `--dialog-width` (default 560).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Shows the modal while `*open`; `body` is laid out with 24 px side padding.
    pub fn show<R>(
        self,
        ctx: &Context,
        open: &mut bool,
        body: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        let (title, description) = (self.title, self.description);
        dialog(
            ctx,
            Frame {
                id: self.id,
                width: self.width,
                close_outside: true,
            },
            open,
            |ui, open| {
                let t = tokens(ui.ctx());
                let width = ui.available_width();
                let top = ui.cursor().min;
                // `px-6 pt-5 pb-3`, the close button `-mr-2`.
                let close_rect = Rect::from_min_size(
                    pos2(top.x + width - 24.0 + 8.0 - 32.0, top.y + 20.0),
                    Vec2::splat(32.0),
                );
                let mut close_ui = ui.new_child(UiBuilder::new().max_rect(close_rect));
                if IconButton::new("Close", Icon::X, 16.0)
                    .show(&mut close_ui)
                    .clicked()
                {
                    *open = false;
                }
                let text_rect = Rect::from_min_size(
                    pos2(top.x + 24.0, top.y + 20.0),
                    vec2(width - 48.0 - 32.0 - 16.0 + 8.0, f32::INFINITY),
                );
                let mut text_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(text_rect)
                        .layout(Layout::top_down(Align::Min)),
                );
                text::paragraph(
                    &mut text_ui,
                    title,
                    TextStyle::SUBTITLE.weight(Weight::Heavy).heading(),
                    t.fg,
                );
                if let Some(description) = description {
                    text_ui.add_space(2.0);
                    text::paragraph(&mut text_ui, description, TextStyle::CAPTION, t.fg_2);
                }
                let bottom = text_ui.min_rect().bottom().max(close_rect.bottom());
                ui.advance_cursor_after_rect(Rect::from_min_max(
                    top,
                    pos2(top.x + width, bottom + 12.0),
                ));
            },
            |ui| {
                let mut out = None;
                inset(ui, 24.0, |ui| out = Some(body(ui)));
                ui.add_space(20.0);
                out
            },
            |ui, open| {
                footer_row(ui, |ui| {
                    if Button::new("Done")
                        .accent()
                        .min_width(120.0)
                        .show(ui)
                        .clicked()
                    {
                        *open = false;
                    }
                });
            },
        )
        .flatten()
    }
}

/// Runs `content` with `x` points of padding on both sides.
fn inset(ui: &mut Ui, x: f32, content: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let rect = Rect::from_min_size(
        ui.cursor().min + vec2(x, 0.0),
        vec2(width - 2.0 * x, f32::INFINITY),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    content(&mut child);
    let used = child.min_rect();
    ui.advance_cursor_after_rect(Rect::from_min_max(
        ui.cursor().min,
        pos2(ui.cursor().min.x + width, used.bottom()),
    ));
}
