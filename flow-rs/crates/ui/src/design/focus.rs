//! Focus rings for keyboard focus only (`:focus-visible`: `outline: 2px solid
//! var(--focus); outline-offset: 1px`). A ring shows after Tab or arrow keys
//! and hides again on the next pointer press.

use egui::epaint::StrokeKind;
use egui::{Context, CornerRadius, Id, Key, Rect, Response, Stroke, Ui};

fn key() -> Id {
    Id::unique("flow-focus-visible")
}

/// Tracks the input modality. The app calls this at the start of every frame.
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
