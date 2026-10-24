//! Keeps one broken page from blanking the whole window. Port of
//! `components/ErrorBoundary.tsx`: a panic while drawing the page is caught,
//! logged, and replaced by a notice with "Try again". The error clears when
//! `reset_key` changes (another page).

use std::panic::{AssertUnwindSafe, catch_unwind};

use egui::Ui;

use super::button::Button;
use super::notice::{Notice, Tone};

#[derive(Default)]
pub struct ErrorBoundary {
    error: Option<String>,
    reset_key: String,
}

impl ErrorBoundary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn show(&mut self, ui: &mut Ui, reset_key: &str, body: impl FnOnce(&mut Ui)) {
        if self.reset_key != reset_key {
            self.reset_key = reset_key.to_string();
            self.error = None;
        }
        if let Some(error) = &self.error {
            ui.add_space(24.0);
            let message = format!("This page ran into a problem and could not be shown: {error}");
            let retry = Notice::new(Tone::Danger, &message)
                .show_with_action(ui, |ui| Button::new("Try again").small().show(ui).clicked());
            if retry {
                self.error = None;
            }
            return;
        }
        // Draw into a child so a half-drawn page can be dropped wholesale.
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap()).layout(*ui.layout()));
        let outcome = catch_unwind(AssertUnwindSafe(|| body(&mut child)));
        match outcome {
            Ok(()) => {
                let used = child.min_rect();
                ui.advance_cursor_after_rect(used);
            }
            Err(panic) => {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "Something went wrong".into());
                log::error!(target: "settings", "Settings page crashed: {message}");
                self.error = Some(message);
                ui.ctx().request_discard("a page panicked");
                ui.ctx().request_repaint();
            }
        }
    }
}
