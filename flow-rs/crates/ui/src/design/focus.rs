//! Focus rings for keyboard focus only (`:focus-visible`: `outline: 2px solid
//! var(--focus); outline-offset: 1px`). A ring shows after Tab or arrow keys
//! and hides again on the next pointer press.
//!
//! Focus moved by code (a dialog focusing its Cancel button) follows the same
//! rule, as Chromium's does: a ring when the last input was the keyboard, none
//! after a click.

use egui::epaint::StrokeKind;
use egui::{Context, CornerRadius, FocusDirection, Id, Key, Modifiers, Rect, Response, Stroke, Ui};

fn key() -> Id {
    Id::unique("flow-focus-visible")
}

/// Tracks the input modality and keeps the arrow keys from moving focus. The
/// app calls this at the start of every frame.
pub fn update(ctx: &Context) {
    let (keyboard, pointer) = ctx.input(|i| {
        let keyboard = i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::Key {
                    key: Key::Tab
                        | Key::ArrowDown
                        | Key::ArrowUp
                        | Key::ArrowLeft
                        | Key::ArrowRight
                        | Key::Home
                        | Key::End,
                    pressed: true,
                    ..
                }
            )
        });
        (keyboard, i.pointer.any_pressed())
    });
    if keyboard || pointer {
        ctx.data_mut(|d| d.insert_temp(key(), keyboard && !pointer));
    }
    cancel_arrow_navigation(ctx);
}

/// Chromium moves focus with Tab only; the arrow keys belong to the focused
/// control, and the composite widgets that move focus with them (the nav
/// rail's tabs, radio cards, the prompts accordion, Select) do it themselves.
/// egui would move focus to the nearest widget in the arrow's direction, so
/// this cancels that for the pass. Call it before consuming the arrow keys.
pub fn cancel_arrow_navigation(ctx: &Context) {
    let (arrow, tab) = ctx.input(|i| {
        let pressed = |keys: &[Key]| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
                    if keys.contains(key) && (*key == Key::Tab || !modifiers.any()))
            })
        };
        (pressed(&[Key::ArrowUp, Key::ArrowDown, Key::ArrowLeft, Key::ArrowRight]), pressed(&[Key::Tab]))
    });
    if arrow && !tab {
        ctx.memory_mut(|m| m.move_focus(FocusDirection::None));
    }
}

/// Radix's roving focus over a vertical list of triggers (`Accordion`):
/// while one of `items` has focus, Down and Up move to the next and previous
/// one (wrapping around), Home and End to the first and last. What lies
/// between them, such as an open item's text box and buttons, is skipped.
/// Returns the index that took focus.
pub fn vertical_list(ui: &Ui, items: &[Response]) -> Option<usize> {
    let current = items.iter().position(Response::has_focus)?;
    let count = items.len();
    cancel_arrow_navigation(ui.ctx());
    let next = ui.input_mut(|i| {
        let none = Modifiers::NONE;
        if i.consume_key(none, Key::ArrowDown) {
            Some((current + 1) % count)
        } else if i.consume_key(none, Key::ArrowUp) {
            Some((current + count - 1) % count)
        } else if i.consume_key(none, Key::Home) {
            Some(0)
        } else if i.consume_key(none, Key::End) {
            Some(count - 1)
        } else {
            None
        }
    })?;
    items[next].request_focus();
    Some(next)
}

/// Whether focus came from the keyboard.
pub fn visible(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(key()).unwrap_or(false))
}

/// Paints the ring around `rect` when `response` has keyboard focus.
pub fn ring(ui: &Ui, response: &Response, rect: Rect, radius: f32) {
    if response.has_focus() && visible(ui.ctx()) {
        paint_ring(ui, rect, radius);
    }
}

/// The ring itself: 2 px, 1 px outside the element, following its radius.
pub fn paint_ring(ui: &Ui, rect: Rect, radius: f32) {
    let color = super::tokens(ui.ctx()).focus;
    ui.painter().rect_stroke(
        rect.expand(1.0),
        CornerRadius::from(radius + 1.0),
        Stroke::new(2.0, color),
        StrokeKind::Outside,
    );
}
