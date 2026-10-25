//! The content panel's vertical scroll with the window's thin scrollbar
//! (`::-webkit-scrollbar` in `styles.css`): a 14 px gutter that is always
//! reserved (`scrollbar-gutter: stable`), a 4 px `--fg-3` thumb inset 5 px
//! that widens to 6 px under the pointer. Wheel scrolling is egui's.

use egui::scroll_area::ScrollBarVisibility;
use egui::{CornerRadius, Id, Rect, ScrollArea, Sense, Ui, UiBuilder, pos2};

use crate::design::tokens;

pub const GUTTER: f32 = 14.0;

#[derive(Default)]
pub struct PageScroll {
    /// An offset to apply next frame (page change, thumb drag).
    set: Option<f32>,
    /// Where a thumb drag started: pointer y and scroll offset.
    drag_from: Option<(f32, f32)>,
    /// The largest offset as of last frame, to clamp `set` (egui does not clamp a set offset).
    max_offset: f32,
}

impl PageScroll {
    /// Scrolls back to the top on the next frame (page change).
    pub fn reset(&mut self) {
        self.set = Some(0.0);
        self.drag_from = None;
    }

    /// Scrolls to `offset` on the next frame.
    pub fn scroll_to(&mut self, offset: f32) {
        self.set = Some(offset);
    }

    pub fn show(&mut self, ui: &mut Ui, rect: Rect, id: Id, content: impl FnOnce(&mut Ui)) {
        let t = tokens(ui.ctx());
        let view = Rect::from_min_max(rect.min, pos2(rect.right() - GUTTER, rect.bottom()));
        let mut child = ui.new_child(UiBuilder::new().max_rect(view));
        child.set_clip_rect(view.intersect(ui.clip_rect()));
        let mut area = ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink([false, false])
            .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
            .animated(false);
        if let Some(offset) = self.set.take() {
            area = area.vertical_scroll_offset(offset.min(self.max_offset).max(0.0));
        }
        let output = area.show(&mut child, content);

        let content_h = output.content_size.y;
        let view_h = output.inner_rect.height();
        self.max_offset = (content_h - view_h).max(0.0);
        if content_h <= view_h + 0.5 {
            return;
        }
        let offset = output.state.clamped_offset().y;
        let max_offset = content_h - view_h;
        let track = Rect::from_min_max(pos2(rect.right() - GUTTER, rect.top()), rect.max);
        let thumb_len = (view_h / content_h * track.height()).max(28.0);
        let travel = (track.height() - thumb_len).max(1.0);
        let thumb_top = track.top() + travel * (offset / max_offset).clamp(0.0, 1.0);
        let slot = Rect::from_min_max(pos2(track.left(), thumb_top), pos2(track.right(), thumb_top + thumb_len));
        let response = ui.interact(track, id.with("scrollbar"), Sense::click_and_drag());
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if response.drag_started()
            && let Some(p) = pointer
        {
            if slot.contains(p) {
                self.drag_from = Some((p.y, offset));
            } else {
                // A press on the track pages towards the pointer.
                let page = if p.y < slot.top() { -view_h } else { view_h };
                self.set = Some((offset + page * 0.875).clamp(0.0, max_offset));
                ui.ctx().request_repaint();
            }
        }
        if response.dragged()
            && let (Some((from_y, from_offset)), Some(p)) = (self.drag_from, pointer)
        {
            self.set = Some((from_offset + (p.y - from_y) * max_offset / travel).clamp(0.0, max_offset));
            ui.ctx().request_repaint();
        }
        if response.drag_stopped() {
            self.drag_from = None;
        }
        let hot = self.drag_from.is_some() || pointer.is_some_and(|p| slot.contains(p) && response.hovered());
        let border = if hot { 4.0 } else { 5.0 };
        let thumb = slot.shrink(border);
        ui.painter().rect_filled(thumb, CornerRadius::from(thumb.width() / 2.0), t.fg_3);
    }
}
