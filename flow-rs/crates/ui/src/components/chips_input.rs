//! A list of short strings edited as chips. Enter or a comma adds the typed
//! text; Backspace in an empty box removes the last chip. Port of
//! `components/ChipsInput.tsx`.

use std::hash::Hash;

use egui::epaint::StrokeKind;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, CornerRadius, Frame, Id, Key, Layout, Margin, Rect, Sense, Stroke, TextBuffer, TextEdit, Ui, UiBuilder,
    Vec2, pos2, vec2,
};

use super::text_field::{InputLook, paint_input_frame};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, icons, tokens};

/// Adds the comma-separated parts of `raw` that are not already there (case-insensitive).
pub fn add_parts(values: &[String], raw: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let mut next = values.to_vec();
    for part in parts {
        if !next.iter().any(|v| v.to_lowercase() == part.to_lowercase()) {
            next.push(part.to_string());
        }
    }
    (next.len() != values.len()).then_some(next)
}

pub struct ChipsInput<'a> {
    id: Id,
    values: &'a [String],
    placeholder: Option<&'a str>,
    mono: bool,
}

impl<'a> ChipsInput<'a> {
    pub fn new(id_salt: impl Hash + std::fmt::Debug, values: &'a [String]) -> Self {
        Self { id: Id::unique(("flow-chips", id_salt)), values, placeholder: None, mono: false }
    }
    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Returns the new list when it changed.
    pub fn show(self, ui: &mut Ui) -> Option<Vec<String>> {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let mut typed: String = ctx.data(|d| d.get_temp(self.id)).unwrap_or_default();
        let chip_style = if self.mono { TextStyle::CAPTION.mono() } else { TextStyle::CAPTION };
        let input_style = if self.mono { TextStyle::BODY.mono().size(13.0, 20.0) } else { TextStyle::BODY };
        let width = ui.available_width();
        let inner_w = width - 2.0 - 12.0;
        let galleys: Vec<_> =
            self.values.iter().map(|v| text::galley(ui, v.as_str(), chip_style, t.fg, f32::INFINITY)).collect();
        // Flow layout: chips 4 px apart, the text box takes the rest of the last line (at least 72).
        let mut positions = Vec::new();
        let (mut x, mut y) = (0.0f32, 0.0f32);
        for g in &galleys {
            let w = 8.0 + g.size().x + 2.0 + 20.0 + 2.0 + 2.0;
            if x > 0.0 && x + w > inner_w {
                x = 0.0;
                y += 28.0;
            }
            positions.push((pos2(x, y), w));
            x += w + 4.0;
        }
        if x > 0.0 && inner_w - x < 72.0 {
            x = 0.0;
            y += 28.0;
        }
        let input_slot = (pos2(x, y), (inner_w - x).max(72.0));
        let height = (y + 24.0 + 6.0 + 2.0).max(32.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let edit_id = self.id.with("edit");
        let focused = ctx.memory(|m| m.has_focus(edit_id));
        paint_input_frame(
            ui.painter(),
            rect,
            InputLook { hovered: ui.rect_contains_pointer(rect), focused, ..InputLook::default() },
        );
        let origin = rect.min + vec2(1.0 + 6.0, 1.0 + 3.0);

        let mut next: Option<Vec<String>> = None;
        for (i, (g, (p, w))) in galleys.into_iter().zip(positions).enumerate() {
            let chip = Rect::from_min_size(origin + p.to_vec2(), vec2(w, 24.0));
            let painter = ui.painter();
            painter.rect_filled(chip, CornerRadius::from(4.0), t.subtle_hover);
            painter.rect_stroke(chip, CornerRadius::from(4.0), Stroke::new(1.0, t.line), StrokeKind::Inside);
            painter.galley(pos2(chip.left() + 8.0, chip.center().y - g.size().y / 2.0), g, t.fg);
            let button =
                Rect::from_min_size(pos2(chip.right() - 2.0 - 1.0 - 20.0, chip.center().y - 10.0), Vec2::splat(20.0));
            let response = ui.interact(button, self.id.with(("remove", i)), Sense::click());
            let color = if response.hovered() { t.fg } else { t.fg_2 };
            if response.hovered() {
                ui.painter().rect_filled(button, CornerRadius::from(4.0), t.subtle_hover);
            }
            icons::paint(ui, Icon::X, button, 12.0, 2.0, color);
            crate::design::focus::ring(ui, &response, button, 4.0);
            if response.clicked() {
                let value = &self.values[i];
                next = Some(self.values.iter().filter(|v| *v != value).cloned().collect());
            }
        }

        let input_rect = Rect::from_min_size(origin + input_slot.0.to_vec2(), vec2(input_slot.1, 24.0));
        if typed.is_empty()
            && self.values.is_empty()
            && let Some(placeholder) = self.placeholder
        {
            let g = text::galley(ui, placeholder, input_style, t.fg_3, f32::INFINITY);
            ui.painter().galley(pos2(input_rect.left() + 4.0, input_rect.center().y - g.size().y / 2.0), g, t.fg_3);
        }
        // Enter and Backspace are read before the TextEdit sees them.
        let (enter, backspace) = if focused {
            ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Backspace)))
        } else {
            (false, false)
        };
        let was_empty = typed.is_empty();
        let mut child = ui.new_child(UiBuilder::new().max_rect(input_rect).layout(Layout::top_down(Align::Min)));
        let mut layouter = move |ui: &Ui, buffer: &dyn TextBuffer, wrap: f32| {
            let mut job = LayoutJob::single_section(buffer.as_str().to_owned(), input_style.format(t.fg));
            job.wrap = TextWrapping { max_width: wrap, ..TextWrapping::default() };
            crate::design::text::snap_baselines(ui.painter().layout_job(job), input_style)
        };
        let output = TextEdit::singleline(&mut typed)
            .id(edit_id)
            .frame(Frame::NONE.inner_margin(Margin { left: 4, right: 4, top: 2, bottom: 2 }))
            .desired_width(input_rect.width() - 8.0)
            .min_size(input_rect.size())
            .layouter(&mut layouter)
            .show(&mut child);
        let response = output.response.response;
        // A click on the box (not on a chip) focuses the text box.
        if ui.interact(rect, self.id.with("box"), Sense::click()).clicked() {
            response.request_focus();
        }
        if typed.contains(',') || enter || response.lost_focus() {
            let raw = std::mem::take(&mut typed);
            if let Some(added) = add_parts(next.as_deref().unwrap_or(self.values), &raw) {
                next = Some(added);
            }
            if enter {
                response.request_focus();
            }
        } else if backspace && was_empty && !self.values.is_empty() {
            next = Some(self.values[..self.values.len() - 1].to_vec());
        }
        ctx.data_mut(|d| d.insert_temp(self.id, typed));
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_new_parts_only() {
        let values = vec!["tory".to_string()];
        assert_eq!(add_parts(&values, " Tory, towery ,"), Some(vec!["tory".to_string(), "towery".to_string()]));
        assert_eq!(add_parts(&values, "TORY"), None);
        assert_eq!(add_parts(&values, " , "), None);
    }
}
