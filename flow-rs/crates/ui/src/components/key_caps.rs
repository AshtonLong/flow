//! A binding drawn as key caps: `Ctrl+Win` → [Ctrl] [Win]
//! (`components/KeyCaps.tsx`, `.keycap`).

use egui::epaint::StrokeKind;
use egui::{Color32, CornerRadius, Response, Sense, Stroke, Ui, pos2, vec2};

use crate::adapters::hotkeys::display_tokens;
use crate::design::text::{self, TextStyle};
use crate::design::{Weight, tokens};

/// Key caps for `binding`, or "Not set". `muted` fades them to 60 %.
pub fn key_caps(ui: &mut Ui, binding: &str, muted: bool) -> Response {
    let t = tokens(ui.ctx());
    let tokens_list = display_tokens(binding);
    if tokens_list.is_empty() {
        return text::label(ui, "Not set", TextStyle::BODY, t.fg_3);
    }
    let opacity = if muted { 0.6 } else { 1.0 };
    let style = TextStyle::CAPTION.weight(Weight::Strong).mono();
    let galleys: Vec<_> = tokens_list
        .iter()
        .map(|k| text::galley(ui, k.as_str(), style, Color32::BLACK, f32::INFINITY))
        .collect();
    let widths: Vec<f32> = galleys
        .iter()
        .map(|g| (g.size().x + 16.0).max(28.0))
        .collect();
    let total = widths.iter().sum::<f32>() + 4.0 * (widths.len() as f32 - 1.0);
    // `margin-bottom: 3px` leaves room for the wall.
    let (rect, response) = ui.allocate_exact_size(vec2(total, 29.0), Sense::hover());
    let painter = ui.painter();
    let mut x = rect.left();
    for (galley, width) in galleys.into_iter().zip(widths) {
        let cap = egui::Rect::from_min_size(pos2(x, rect.top()), vec2(width, 26.0));
        let r = CornerRadius::from(6.0);
        painter.rect_filled(
            cap.translate(vec2(0.0, 4.0)),
            r,
            Color32::from_black_alpha(51).gamma_multiply(opacity),
        );
        painter.rect_filled(
            cap.translate(vec2(0.0, 3.0)),
            r,
            t.wall.gamma_multiply(opacity),
        );
        painter.rect_filled(cap, r, Color32::WHITE.gamma_multiply(opacity));
        painter.rect_stroke(
            cap,
            r,
            Stroke::new(1.0, Color32::from_black_alpha(20).gamma_multiply(opacity)),
            StrokeKind::Inside,
        );
        let color = Color32::BLACK.gamma_multiply(opacity);
        painter.galley(
            pos2(
                cap.center().x - galley.size().x / 2.0,
                cap.center().y - galley.size().y / 2.0,
            ),
            galley,
            color,
        );
        x += width + 4.0;
    }
    response
}
