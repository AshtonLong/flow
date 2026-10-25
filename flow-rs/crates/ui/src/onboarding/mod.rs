//! First run: three steps instead of the settings pages. Pick a microphone,
//! set the hold-to-talk hotkey, choose a model, then try a dictation right
//! here. Port of `src/renderer/settings/onboarding/Onboarding.tsx`.
//!
//! The shell creates [`Onboarding`] while `general.onboarded` is false, draws
//! the 40 px title strip with the mark above `ui`'s rect, and drops it once
//! finishing (or skipping) sets `general.onboarded`, which switches the window
//! to the settings pages.
//!
//! Each step's state lives only while the step is shown, as React unmounts the
//! others: leaving the microphone step releases the microphone, leaving the
//! hotkey step cancels a recording, and the model step starts over.

mod hotkey;
mod key_form;
mod microphone;
mod model;
mod try_it;

#[cfg(test)]
mod tests;

use egui::{Align, Id, Layout, Rect, Ui, UiBuilder, pos2, vec2};
use flow_core::path;

use crate::components::hotkey_recorder;
use crate::components::{Button, link};
use crate::design::text::{self, TextStyle};
use crate::design::{Weight, tokens};
use crate::pages::{Cx, Page};
use crate::shell::scroll::{GUTTER, PageScroll};

pub(crate) const STEPS: [&str; 3] = ["Microphone", "Hotkey", "Model"];

/// What the current step keeps while it is shown.
enum Body {
    Microphone(microphone::StepMicrophone),
    Hotkey,
    Model(Box<model::StepModel>),
}

impl Body {
    fn for_step(step: usize) -> Self {
        match step {
            0 => Self::Microphone(microphone::StepMicrophone::default()),
            1 => Self::Hotkey,
            _ => Self::Model(Box::default()),
        }
    }
}

pub struct Onboarding {
    step: usize,
    body: Body,
    scroll: PageScroll,
    /// The content was taller than the view last pass, so the scrollbar takes its gutter
    /// (`overflow-y-auto` without a stable gutter).
    overflow: bool,
    /// The content height last pass.
    content_h: f32,
    /// Preview only: keep the page scrolled to the bottom until the pointer is used.
    scroll_end: bool,
}

impl Default for Onboarding {
    fn default() -> Self {
        Self {
            step: 0,
            body: Body::for_step(0),
            scroll: PageScroll::default(),
            overflow: false,
            content_h: 0.0,
            scroll_end: false,
        }
    }
}

impl Onboarding {
    /// Starts on a later step, for previews and screenshots (`--onboarding --demo <step>`):
    /// `hotkey`, `model`, `cloud` (the model step with the cloud choice picked), or `try`
    /// (the model step after a made-up dictation, scrolled to the bottom).
    pub fn with_demo(demo: Option<&str>) -> Self {
        let mut onboarding = Self::default();
        match demo {
            Some("hotkey") => onboarding.show_step(1),
            Some("model") => onboarding.show_step(2),
            Some("cloud") => {
                onboarding.show_step(2);
                if let Body::Model(step) = &mut onboarding.body {
                    step.choose(model::Choice::Cloud);
                }
            }
            Some("try") => {
                onboarding.show_step(2);
                if let Body::Model(step) = &mut onboarding.body {
                    step.simulate_dictation("Can you send me the quarterly numbers before the stand-up tomorrow?");
                }
                onboarding.scroll_end = true;
            }
            _ => {}
        }
        onboarding
    }

    /// The step shown: 0 microphone, 1 hotkey, 2 model.
    pub fn step(&self) -> usize {
        self.step
    }

    fn show_step(&mut self, step: usize) {
        self.step = step;
        self.body = Body::for_step(step);
    }

    fn go_to(&mut self, ui: &Ui, step: usize, cx: &Cx<'_>) {
        if step == self.step {
            return;
        }
        if matches!(self.body, Body::Hotkey) {
            // The recorder unmounts: a recording in progress is cancelled.
            hotkey_recorder::cancel_capture(ui.ctx(), cx.link);
        }
        self.show_step(step);
        // Moving between steps puts focus at the top of the new one: the next Tab
        // lands on its first control.
        ui.ctx().memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }

    fn finish(cx: &mut Cx<'_>) {
        cx.config.set(path!["general", "onboarded"], true);
    }
}

/// `grid grid-cols-3 gap-2`: a 3 px bar per step, filled up to the current one,
/// and its name under it.
fn step_list(ui: &mut Ui, step: usize) {
    let t = tokens(ui.ctx());
    let top = ui.cursor().min;
    let width = ui.available_width();
    let column = (width - 16.0) / 3.0;
    for (i, name) in STEPS.iter().enumerate() {
        let x = top.x + i as f32 * (column + 8.0);
        let bar = Rect::from_min_size(pos2(x, top.y), vec2(column, 3.0));
        ui.painter().rect_filled(bar, 1.5, if i <= step { t.accent_fill } else { t.line_strong });
        let color = if i == step { t.fg } else { t.fg_2 };
        let label = text::galley(ui, format!("{}. {name}", i + 1), TextStyle::CAPTION, color, column);
        ui.painter().galley(pos2(x, top.y + 3.0 + 6.0), label, color);
    }
    // Bar, `mt-1.5`, one caption line.
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, 3.0 + 6.0 + 16.0)));
}

/// What the footer asked for.
enum Footer {
    Skip,
    Back,
    Next,
}

/// `mt-8 flex items-center gap-2`: "Skip setup" on the left, Back and Next (or
/// Finish) on the right, both at least 96 px wide.
fn footer(ui: &mut Ui, rect: Rect, step: usize) -> Option<Footer> {
    let mut action = None;
    let mut left = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
    if link(&mut left, "Skip setup", TextStyle::BODY).clicked() {
        action = Some(Footer::Skip);
    }
    let mut right = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)));
    right.spacing_mut().item_spacing.x = 8.0;
    let last = step == STEPS.len() - 1;
    if Button::new(if last { "Finish" } else { "Next" }).accent().min_width(96.0).show(&mut right).clicked() {
        action = Some(Footer::Next);
    }
    if step > 0 && Button::new("Back").min_width(96.0).show(&mut right).clicked() {
        action = Some(Footer::Back);
    }
    action
}

impl Page for Onboarding {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        let area = ui.max_rect();
        // `main`: `overflow-y-auto px-6 pb-6`. The scrollbar takes room only when needed.
        let gutter = if self.overflow { GUTTER } else { 0.0 };
        let view_w = area.width() - gutter;
        let view_h = area.height();
        if self.scroll_end {
            if ui.input(|i| i.pointer.any_down() || i.smooth_scroll_delta().y != 0.0) {
                self.scroll_end = false;
            } else {
                self.scroll.scroll_to((self.content_h - view_h).max(0.0));
            }
        }
        let scroll_rect = Rect::from_min_max(area.min, pos2(area.right() - gutter + GUTTER, area.bottom()));
        let mut content_h = 0.0;
        let mut action = None;
        let Self { scroll, body, step, .. } = self;
        let step = *step;
        scroll.show(ui, scroll_rect, Id::unique("flow-onboarding-scroll"), |ui| {
            let origin = ui.max_rect().min;
            // `mx-auto flex min-h-full max-w-[560px] flex-col`
            let main_w = view_w - 48.0;
            let column_w = main_w.min(560.0);
            let left = origin.x + 24.0 + (main_w - column_w) / 2.0;
            let column = Rect::from_min_size(pos2(left, origin.y), vec2(column_w, f32::INFINITY));
            let mut col = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));

            // `header pt-4 pb-6`
            col.add_space(16.0);
            text::paragraph(&mut col, "Set up Flow", TextStyle::TITLE.weight(Weight::Black).heading(), t.fg);
            col.add_space(16.0);
            step_list(&mut col, step);
            col.add_space(24.0);

            // The step, `flex-1`.
            match body {
                Body::Microphone(body) => body.ui(&mut col, cx),
                Body::Hotkey => hotkey::ui(&mut col, cx),
                Body::Model(body) => body.ui(&mut col, cx),
            }

            // The footer sits at the bottom of the view, or `mt-8` after a long step.
            let content_bottom = col.cursor().min.y;
            let footer_top = (content_bottom + 32.0).max(origin.y + view_h - 24.0 - 32.0);
            let footer_rect = Rect::from_min_size(pos2(left, footer_top), vec2(column_w, 32.0));
            action = footer(ui, footer_rect, step);
            let bottom = footer_rect.bottom() + 24.0;
            ui.advance_cursor_after_rect(Rect::from_min_max(origin, pos2(origin.x + 1.0, bottom)));
            content_h = bottom - origin.y;
        });
        self.content_h = content_h;
        let overflow = content_h > view_h + 0.5;
        if overflow != self.overflow {
            self.overflow = overflow;
            ui.ctx().request_discard("flow-ui: the onboarding scrollbar came or went");
            ui.ctx().request_repaint();
        }

        match action {
            Some(Footer::Skip) => Self::finish(cx),
            Some(Footer::Back) => self.go_to(ui, self.step.saturating_sub(1), cx),
            Some(Footer::Next) if self.step == STEPS.len() - 1 => Self::finish(cx),
            Some(Footer::Next) => self.go_to(ui, self.step + 1, cx),
            None => {}
        }
    }
}

/// `h2.font-display.text-subtitle.font-[760]` and the `mt-1 text-fg-2` lead under it.
pub(crate) fn step_heading(ui: &mut Ui, title: &str, lead: &str) {
    let t = tokens(ui.ctx());
    text::paragraph(ui, title, TextStyle::SUBTITLE.weight(Weight::Heavy).heading(), t.fg);
    ui.add_space(4.0);
    text::paragraph(ui, lead, TextStyle::BODY, t.fg_2);
}
