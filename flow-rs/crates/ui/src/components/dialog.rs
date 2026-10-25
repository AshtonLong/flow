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
//!
//! Focus works as in Radix's `FocusScope` and `DismissableLayer`:
//!
//! - On open, `AlertDialog` focuses Cancel; the TSX's `Modal` focuses the
//!   dialog itself (nothing inside), so its Close button shows no tooltip.
//!   The ring follows `:focus-visible` (see `design::focus`).
//! - Tab and Shift+Tab cycle through the dialog's controls only. With the
//!   dialog itself focused, Tab goes to the first control and Shift+Tab stays.
//! - Escape closes the dialog, also from a text box with a changed draft: the
//!   layer's capture-phase listener runs before the field's own handler. Text
//!   fields save their draft as the dialog closes, as React's do on unmount.
//!   A Select list or a tooltip open over the dialog takes Escape first.
//! - On close focus is not handed back: the TSX has no `Dialog.Trigger`, so
//!   Radix has nowhere to return it and Chromium leaves it on the document.
//!   The next Tab starts from the top of the window, as in egui with nothing
//!   focused.

use std::hash::Hash;

use egui::{
    Align, Area, Context, CornerRadius, FocusDirection, Id, Key, Layout, Modifiers, Order, Rect, Response, ScrollArea,
    Sense, Ui, UiBuilder, Vec2, pos2, vec2,
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
    /// The TSX's `onOpenAutoFocus` focuses the dialog itself, not a control.
    focus_self: bool,
}

fn layer_key() -> Id {
    Id::unique("flow-dismissable-layer")
}

fn closing_key() -> Id {
    Id::unique("flow-dialog-closing")
}

/// Called each pass by a layer drawn over a dialog that dismisses itself on
/// Escape (an open Select list, a tooltip). Radix hands Escape to the topmost
/// `DismissableLayer` only, so the dialog under it stays open.
pub(crate) fn layer_shown(ctx: &Context) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(layer_key(), pass));
}

/// Whether such a layer was shown in this pass or the last one.
fn layer_above(ctx: &Context) -> bool {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<u64>(layer_key())).is_some_and(|shown| shown + 1 >= pass)
}

/// True while a dialog that is closing draws its contents for the last time.
/// A text field saves its pending draft then, as `useDraft` does on unmount.
pub(crate) fn closing(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(closing_key())).unwrap_or(false)
}

/// Whether the dialog `id` was not on screen in the previous pass.
fn just_opened(ctx: &Context, id: Id) -> bool {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<u64>(id.with("shown"))).is_none_or(|shown| shown + 1 < pass)
}

/// Shift+Tab (or Tab) pressed this pass.
fn tab_pressed(ctx: &Context, shift: bool) -> bool {
    ctx.input(|i| {
        i.events.iter().any(|e| {
            matches!(e, egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. }
                if if shift { modifiers.shift_only() } else { !modifiers.any() })
        })
    })
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
    let natural = measure::remembered(ctx, frame.id.with("size")).map(|s| s.y).unwrap_or(200.0);
    let height = natural.min(max_h);
    let rect = Rect::from_center_size(screen.center(), vec2(width, height)).round();
    let mut result = None;

    if just_opened(ctx, frame.id) {
        if frame.focus_self {
            ctx.memory_mut(|m| {
                if let Some(focused) = m.focused() {
                    m.surrender_focus(focused);
                }
            });
        }
        // The control focused on open gets its ring next pass.
        ctx.request_repaint();
    }
    // `DismissableLayer`: Escape closes the topmost layer only.
    if !layer_above(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        *open = false;
    }
    // `FocusScope trapped loop`: egui keeps Tab among this layer's widgets (the
    // modal layer below) and wraps around the ends on the next pass.
    let (tab, shift_tab) = (tab_pressed(ctx, false), tab_pressed(ctx, true));
    if tab || shift_tab {
        ctx.request_repaint();
    }
    if shift_tab && ctx.memory(|m| m.focused().is_none()) {
        // The dialog itself has focus: Shift+Tab leaves the scope and is pulled back.
        ctx.memory_mut(|m| m.move_focus(FocusDirection::None));
    }

    Area::new(frame.id).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        // Nothing under the smoke can be focused or pressed.
        ui.memory_mut(|m| m.set_modal_layer(ui.layer_id()));
        ui.painter().rect_filled(screen, CornerRadius::ZERO, t.smoke);
        let backdrop = ui.allocate_rect(screen, Sense::CLICK);
        let pressed_outside =
            ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p)));
        if frame.close_outside && pressed_outside && backdrop.hovered() {
            *open = false;
        }
        shadow::paint_surface(ui.painter(), rect, 12.0, t.flyout, &t.shadow_dialog, t.hc_border);
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
        let footer_h = measure::remembered(ctx, frame.id.with("footer")).map(|s| s.y).unwrap_or(65.0);
        let body_max = (rect.height() - header_h - footer_h).max(0.0);
        let body_rect = Rect::from_min_size(pos2(rect.left(), rect.top() + header_h), vec2(width, body_max));
        let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body_rect).layout(Layout::top_down(Align::Min)));
        body_ui.set_clip_rect(clip);
        let mut body_h = 0.0;
        let closing = !*open;
        if closing {
            ctx.data_mut(|d| d.insert_temp(closing_key(), true));
        }
        ScrollArea::vertical().id_salt(frame.id.with("body")).max_height(body_max).auto_shrink([false, true]).show(
            &mut body_ui,
            |ui| {
                let start = ui.cursor().min.y;
                result = Some(body(ui));
                body_h = ui.cursor().min.y - start;
            },
        );
        if closing {
            ctx.data_mut(|d| d.remove::<bool>(closing_key()));
        }
        let body_shown = body_h.min(body_max);

        let footer_top = rect.top() + header_h + body_shown;
        let mut footer_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_size(pos2(rect.left(), footer_top), vec2(width, f32::INFINITY)))
                .layout(Layout::top_down(Align::Min)),
        );
        footer_ui.set_clip_rect(clip);
        let footer_slot = footer_ui.painter().add(egui::Shape::Noop);
        footer(&mut footer_ui, open);
        let footer_rect =
            Rect::from_min_max(pos2(rect.left(), footer_top), pos2(rect.right(), footer_ui.min_rect().bottom()));
        // `border-t border-line bg-inset`, inside the rounded bottom.
        footer_ui.painter().set(
            footer_slot,
            egui::Shape::Vec(vec![
                egui::Shape::rect_filled(footer_rect, CornerRadius { nw: 0, ne: 0, sw: 12, se: 12 }, t.inset),
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
        measure::record(ctx, frame.id.with("size"), vec2(width, header_h + body_h + footer_rect.height()));
    });
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(frame.id.with("shown"), pass));
    result
}

/// The footer row (`flex justify-end gap-2 px-6 py-4`): the buttons in
/// document order, so Tab meets them left to right, pushed to the right.
fn footer_row<const N: usize>(ui: &mut Ui, buttons: [Button<'_>; N]) -> [Response; N] {
    ui.add_space(16.0);
    let width = ui.available_width();
    let row = Rect::from_min_size(ui.cursor().min + vec2(24.0, 0.0), vec2(width - 48.0, 32.0));
    let widths = buttons.each_ref().map(|b| b.natural_width(ui));
    let mut x = row.right() - widths.iter().sum::<f32>() - 8.0 * N.saturating_sub(1) as f32;
    let mut i = 0;
    let responses = buttons.map(|button| {
        let slot = Rect::from_min_size(pos2(x, row.top()), vec2(widths[i], row.height()));
        x += widths[i] + 8.0;
        i += 1;
        let mut child = ui.new_child(UiBuilder::new().max_rect(slot).layout(Layout::left_to_right(Align::Center)));
        button.show(&mut child)
    });
    ui.advance_cursor_after_rect(row);
    ui.add_space(16.0);
    responses
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
        Self { id: Id::unique(("flow-confirm", id_salt)), title, description, confirm_label }
    }

    /// Shows the dialog while `*open`. Returns true when confirmed (it then closes).
    pub fn show(self, ctx: &Context, open: &mut bool) -> bool {
        let mut confirmed = false;
        let (title, description, confirm_label) = (self.title, self.description, self.confirm_label);
        let opening = just_opened(ctx, self.id);
        dialog(
            ctx,
            Frame { id: self.id, width: 440.0, close_outside: false, focus_self: false },
            open,
            |ui, _| {
                let t = tokens(ui.ctx());
                ui.add_space(24.0);
                inset(ui, 24.0, |ui| {
                    text::paragraph(ui, title, TextStyle::SUBTITLE.weight(Weight::Heavy).heading(), t.fg);
                    ui.add_space(8.0);
                    text::paragraph(ui, description, TextStyle::BODY, t.fg_2);
                });
                ui.add_space(20.0);
            },
            |_| {},
            |ui, open| {
                let [confirm, cancel] = footer_row(
                    ui,
                    [Button::new(confirm_label).accent().min_width(120.0), Button::new("Cancel").min_width(120.0)],
                );
                // `AlertDialog` focuses Cancel when it opens.
                if opening {
                    cancel.request_focus();
                }
                if cancel.clicked() {
                    *open = false;
                }
                if confirm.clicked() {
                    confirmed = true;
                    *open = false;
                }
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
        Self { id: Id::unique(("flow-modal", id_salt)), title, description: None, width: 560.0 }
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
    pub fn show<R>(self, ctx: &Context, open: &mut bool, body: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        let (title, description) = (self.title, self.description);
        dialog(
            ctx,
            Frame { id: self.id, width: self.width, close_outside: true, focus_self: true },
            open,
            |ui, open| {
                let t = tokens(ui.ctx());
                let width = ui.available_width();
                let top = ui.cursor().min;
                // `px-6 pt-5 pb-3`, the close button `-mr-2`.
                let close_rect =
                    Rect::from_min_size(pos2(top.x + width - 24.0 + 8.0 - 32.0, top.y + 20.0), Vec2::splat(32.0));
                let mut close_ui = ui.new_child(UiBuilder::new().max_rect(close_rect));
                if IconButton::new("Close", Icon::X, 16.0).show(&mut close_ui).clicked() {
                    *open = false;
                }
                let text_rect = Rect::from_min_size(
                    pos2(top.x + 24.0, top.y + 20.0),
                    vec2(width - 48.0 - 32.0 - 16.0 + 8.0, f32::INFINITY),
                );
                let mut text_ui =
                    ui.new_child(UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
                text::paragraph(&mut text_ui, title, TextStyle::SUBTITLE.weight(Weight::Heavy).heading(), t.fg);
                if let Some(description) = description {
                    text_ui.add_space(2.0);
                    text::paragraph(&mut text_ui, description, TextStyle::CAPTION, t.fg_2);
                }
                let bottom = text_ui.min_rect().bottom().max(close_rect.bottom());
                ui.advance_cursor_after_rect(Rect::from_min_max(top, pos2(top.x + width, bottom + 12.0)));
            },
            |ui| {
                let mut out = None;
                inset(ui, 24.0, |ui| out = Some(body(ui)));
                ui.add_space(20.0);
                out
            },
            |ui, open| {
                let [done] = footer_row(ui, [Button::new("Done").accent().min_width(120.0)]);
                if done.clicked() {
                    *open = false;
                }
            },
        )
        .flatten()
    }
}

/// Runs `content` with `x` points of padding on both sides.
fn inset(ui: &mut Ui, x: f32, content: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let rect = Rect::from_min_size(ui.cursor().min + vec2(x, 0.0), vec2(width - 2.0 * x, f32::INFINITY));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    content(&mut child);
    let used = child.min_rect();
    ui.advance_cursor_after_rect(Rect::from_min_max(ui.cursor().min, pos2(ui.cursor().min.x + width, used.bottom())));
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use egui::{Key, Modifiers, Ui};

    use super::*;
    use crate::components::TextField;
    use crate::design::focus;
    use crate::testing::Harness;

    /// A page with three buttons; "Delete" opens a `ConfirmDialog`, "Edit" a
    /// `Modal` with a name field.
    #[derive(Default)]
    struct Page {
        confirm: Cell<bool>,
        confirmed: Cell<bool>,
        modal: Cell<bool>,
        name: RefCell<String>,
    }

    impl Page {
        fn draw(&self, ui: &mut Ui) {
            focus::update(ui.ctx());
            Button::new("Before").show(ui);
            if Button::new("Delete").show(ui).clicked() {
                self.confirm.set(true);
            }
            if Button::new("Edit").show(ui).clicked() {
                self.modal.set(true);
            }
            let mut open = self.confirm.get();
            let dialog = ConfirmDialog::new("test.confirm", "Delete Slack?", "It goes for good.", "Delete profile");
            if dialog.show(ui.ctx(), &mut open) {
                self.confirmed.set(true);
            }
            self.confirm.set(open);

            let mut open = self.modal.get();
            let name = self.name.borrow().clone();
            let committed = Modal::new("test.modal", "Edit Slack")
                .show(ui.ctx(), &mut open, |ui| TextField::new("test.name", &name).show(ui).committed)
                .flatten();
            if let Some(name) = committed {
                *self.name.borrow_mut() = name;
            }
            self.modal.set(open);
        }
    }

    fn harness() -> (Harness, Page) {
        let page = Page { name: RefCell::new("Slack".into()), ..Page::default() };
        let mut h = Harness::new();
        frames(&mut h, &page, 2);
        (h, page)
    }

    fn frames(h: &mut Harness, page: &Page, n: usize) {
        for _ in 0..n {
            h.frame(|ui| page.draw(ui));
        }
    }

    fn press(h: &mut Harness, page: &Page, key: Key, modifiers: Modifiers) {
        h.key_with(key, modifiers);
        // egui hands focus around the end of the list on the following pass.
        frames(h, page, 2);
    }

    /// Clicks the text `label` painted last (the topmost).
    fn click(h: &mut Harness, page: &Page, label: &str) {
        let at = h.text_rects(label).last().unwrap_or_else(|| panic!("“{label}” is not on screen")).center();
        h.click(at, |ui| page.draw(ui));
        frames(h, page, 2);
    }

    /// Whether the focused widget is the one labelled `label`.
    fn focused_on(h: &Harness, label: &str) -> bool {
        let Some(text) = h.text_rects(label).last().copied() else { return false };
        h.focused_rect().is_some_and(|r| r.contains(text.center()))
    }

    /// Whether the last frame painted a focus ring anywhere.
    fn ring_shown(h: &Harness) -> bool {
        fn find(shape: &egui::Shape, color: egui::Color32) -> bool {
            match shape {
                egui::Shape::Rect(r) => r.stroke.width == 2.0 && r.stroke.color == color,
                egui::Shape::Vec(shapes) => shapes.iter().any(|s| find(s, color)),
                _ => false,
            }
        }
        let color = tokens(&h.ctx).focus;
        h.shapes.iter().any(|s| find(&s.shape, color))
    }

    #[test]
    fn a_confirm_dialog_focuses_cancel_with_a_ring_only_after_the_keyboard() {
        let (mut h, page) = harness();
        click(&mut h, &page, "Delete");
        assert!(page.confirm.get());
        assert!(focused_on(&h, "Cancel"), "AlertDialog focuses Cancel");
        assert!(!ring_shown(&h), "no ring after a click, as with Chromium's :focus-visible");
        press(&mut h, &page, Key::Escape, Modifiers::NONE);
        assert!(!page.confirm.get());

        // Tab to Delete, then Enter: the same focus, now with its ring.
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        assert!(focused_on(&h, "Delete"));
        press(&mut h, &page, Key::Enter, Modifiers::NONE);
        assert!(page.confirm.get());
        assert!(focused_on(&h, "Cancel"));
        assert!(ring_shown(&h));
        assert!(!page.confirmed.get());
    }

    #[test]
    fn tab_and_shift_tab_cycle_through_the_dialog_only() {
        let (mut h, page) = harness();
        click(&mut h, &page, "Delete");
        let mut seen = Vec::new();
        for _ in 0..4 {
            press(&mut h, &page, Key::Tab, Modifiers::NONE);
            seen.push(["Delete profile", "Cancel"].into_iter().find(|l| focused_on(&h, l)));
        }
        assert_eq!(seen, [Some("Delete profile"), Some("Cancel"), Some("Delete profile"), Some("Cancel")]);
        press(&mut h, &page, Key::Tab, Modifiers::SHIFT);
        assert!(focused_on(&h, "Delete profile"));
        press(&mut h, &page, Key::Tab, Modifiers::SHIFT);
        assert!(focused_on(&h, "Cancel"), "Shift+Tab wraps around too");
        // Enter on the focused confirm button confirms.
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        press(&mut h, &page, Key::Enter, Modifiers::NONE);
        assert!(page.confirmed.get());
        assert!(!page.confirm.get());
    }

    #[test]
    fn closing_a_dialog_leaves_nothing_focused() {
        let (mut h, page) = harness();
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        press(&mut h, &page, Key::Enter, Modifiers::NONE);
        assert!(focused_on(&h, "Cancel"));
        press(&mut h, &page, Key::Enter, Modifiers::NONE);
        assert!(!page.confirm.get());
        // The TSX has no `Dialog.Trigger`, so Radix does not hand focus back to Delete.
        assert_eq!(h.ctx.memory(|m| m.focused()), None);
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        assert!(focused_on(&h, "Before"), "the next Tab starts from the top");
    }

    #[test]
    fn a_modal_focuses_itself_then_tab_cycles_from_its_close_button() {
        let (mut h, page) = harness();
        click(&mut h, &page, "Edit");
        assert!(page.modal.get());
        assert_eq!(h.ctx.memory(|m| m.focused()), None, "the dialog itself has focus, not Close");
        press(&mut h, &page, Key::Tab, Modifiers::SHIFT);
        assert_eq!(h.ctx.memory(|m| m.focused()), None, "Shift+Tab from the dialog itself stays");

        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        let close = h.focused_rect().expect("Close is focused");
        assert_eq!(close.size(), egui::vec2(32.0, 32.0), "the Close icon button");
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        assert!(focused_on(&h, "Slack"), "then the name field");
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        assert!(focused_on(&h, "Done"));
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        assert_eq!(h.focused_rect(), Some(close), "around to Close, never to the page");
        press(&mut h, &page, Key::Tab, Modifiers::SHIFT);
        assert!(focused_on(&h, "Done"));
        assert!(page.modal.get());
    }

    #[test]
    fn escape_closes_a_tooltip_over_the_modal_first() {
        let (mut h, page) = harness();
        click(&mut h, &page, "Edit");
        press(&mut h, &page, Key::Tab, Modifiers::NONE);
        h.wait(0.6);
        frames(&mut h, &page, 1);
        assert!(h.shows("Close"), "the focused Close button shows its tooltip");
        press(&mut h, &page, Key::Escape, Modifiers::NONE);
        assert!(!h.shows("Close"), "Escape dismisses the tooltip, the topmost layer");
        assert!(page.modal.get(), "and leaves the dialog open");
        press(&mut h, &page, Key::Escape, Modifiers::NONE);
        assert!(!page.modal.get());
    }

    #[test]
    fn escape_in_a_changed_text_box_closes_the_modal_and_keeps_the_text() {
        let (mut h, page) = harness();
        click(&mut h, &page, "Edit");
        click(&mut h, &page, "Slack");
        h.key(Key::End);
        h.type_text("bot");
        frames(&mut h, &page, 1);
        assert_eq!(*page.name.borrow(), "Slack", "nothing is saved while typing");
        press(&mut h, &page, Key::Escape, Modifiers::NONE);
        assert!(!page.modal.get(), "Radix's capture-phase Escape listener runs before the field's own");
        assert_eq!(*page.name.borrow(), "Slackbot", "the draft is saved as the field unmounts");
    }
}
