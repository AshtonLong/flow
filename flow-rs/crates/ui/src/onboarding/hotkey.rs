//! Step 2: set the hold-to-talk hotkey (`StepHotkey`).

use egui::{Align, Id, Layout, Margin, Rect, Ui, UiBuilder, pos2, vec2};
use flow_core::config::HotkeysConfig;
use flow_core::path;

use super::step_heading;
use crate::components::{HotkeyRecorder, card, measure};
use crate::design::text::{self, TextStyle};
use crate::design::tokens;
use crate::pages::Cx;

pub fn ui(ui: &mut Ui, cx: &mut Cx<'_>) {
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let value = cx.config.config().hotkeys.hold_to_talk.clone();
    let default = HotkeysConfig::default().hold_to_talk;
    step_heading(ui, "Set your hotkey", "Hold it to talk, release it and the text is typed wherever your cursor is.");
    ui.add_space(20.0);
    // `card mt-5 flex min-h-16 items-center justify-between gap-4 px-4 py-3`
    card(ui, Margin::symmetric(16, 12), |ui| {
        let id = Id::unique("flow-onboarding-hotkey");
        let top = ui.cursor().min;
        let width = ui.available_width();
        let label = text::galley(ui, "Hold to talk", TextStyle::BODY, t.fg, f32::INFINITY);
        // The recorder's height is known after drawing it (an error line can follow the keys).
        let recorder_h = measure::remembered(&ctx, id).map(|s| s.y).unwrap_or(32.0);
        let row_h = recorder_h.max(64.0 - 24.0);
        ui.painter().galley(pos2(top.x, top.y + (row_h - label.size().y) / 2.0), label.clone(), t.fg);
        let left = top.x + label.size().x + 16.0;
        let area =
            Rect::from_min_size(pos2(left, top.y + (row_h - recorder_h) / 2.0), vec2(top.x + width - left, recorder_h));
        let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Max)));
        let recorder =
            HotkeyRecorder::new("onboarding.hotkeys.hold_to_talk", &value, "Hold to talk").default_value(&default);
        if let Some(binding) = recorder.show(&mut child, cx.link, cx.events) {
            cx.config.set(path!["hotkeys", "hold_to_talk"], binding);
        }
        measure::record(&ctx, id, child.min_rect().size());
        ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, row_h)));
    });
    ui.add_space(12.0);
    text::paragraph(
        ui,
        "Any key, combination, lone modifier (such as Right Ctrl) or mouse button works. You can change it later on the Hotkeys page.",
        TextStyle::CAPTION,
        t.fg_2,
    );
}
