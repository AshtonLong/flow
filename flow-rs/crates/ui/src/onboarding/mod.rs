//! The first-run flow, shown instead of the shell until `general.onboarded`.
//! PORT: `src/renderer/settings/onboarding/Onboarding.tsx` — not ported yet (stub).
//!
//! To port: keep the `Onboarding` struct (the shell creates it while
//! `general.onboarded` is false and drops it afterwards) and draw the wizard in
//! `ui`. The window's 40 px title strip is already drawn above `ui`'s rect;
//! finishing sets `cx.config.set(path!["general", "onboarded"], true)`, which
//! switches the window to the shell.

use egui::{Align, Layout, Rect, Ui, UiBuilder, pos2, vec2};
use flow_core::path;

use crate::components::{Button, Notice, Tone};
use crate::design::text::{self, TextStyle};
use crate::design::{Weight, tokens};
use crate::pages::{Cx, Page};

#[derive(Default)]
pub struct Onboarding {}

impl Page for Onboarding {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        let area = ui.max_rect();
        let width = area.width().min(560.0) - 48.0;
        let column =
            Rect::from_min_size(pos2(area.center().x - width / 2.0, area.top() + 64.0), vec2(width, area.height()));
        let mut ui = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));
        text::paragraph(&mut ui, "Welcome to Flow", TextStyle::TITLE.weight(Weight::Black).heading(), t.fg);
        ui.add_space(16.0);
        Notice::new(Tone::Info, "The first-run setup has not been ported yet (onboarding/Onboarding.tsx).")
            .show(&mut ui);
        ui.add_space(16.0);
        if Button::new("Skip setup").accent().show(&mut ui).clicked() {
            cx.config.set(path!["general", "onboarded"], true);
        }
    }
}
