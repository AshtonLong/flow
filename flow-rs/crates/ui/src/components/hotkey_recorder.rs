//! Shows a binding as key caps with a Record button. Recording uses the
//! global hook in the main process (`HotkeysCapture`), so Win, lone modifiers
//! and mouse buttons work; partial chords arrive as `HotkeysCaptureProgress`.
//! Port of `components/HotkeyRecorder.tsx`.

use std::hash::Hash;

use egui::{Align, Id, Layout, Ui};
use flow_core::protocol::settings::{Event, Request};

use super::button::{Button, IconButton};
use super::key_caps::key_caps;
use crate::adapters::hotkeys::normalize_binding;
use crate::backend::{Link, Pending};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, tokens};
use crate::util::format::error_message;

#[derive(Clone, Default)]
struct State {
    recording: bool,
    partial: String,
    error: Option<String>,
    capture: Option<Pending<Option<String>>>,
    restore_focus: bool,
}

fn active_id() -> Id {
    Id::unique("flow-hotkey-recorder-active")
}

/// True while a recorder listens: the window then drops key events, so keys
/// cannot click buttons, move focus or scroll the page.
pub fn capturing(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<Option<Id>>(active_id())).flatten().is_some()
}

/// Cancels the capture in progress, if any (page change, window blur). The
/// window takes keys again at once, even if the recorder is not drawn again
/// (it unmounted, as in the TSX's cleanup).
pub fn cancel_capture(ctx: &egui::Context, link: &Link) {
    if capturing(ctx) {
        link.send(Request::HotkeysCaptureCancel);
        ctx.data_mut(|d| d.insert_temp(active_id(), None::<Id>));
    }
}

pub struct HotkeyRecorder<'a> {
    id: Id,
    value: &'a str,
    default_value: Option<&'a str>,
    action_name: &'a str,
}

impl<'a> HotkeyRecorder<'a> {
    /// `action_name` is for screen readers: "Hold to talk".
    pub fn new(id_salt: impl Hash + std::fmt::Debug, value: &'a str, action_name: &'a str) -> Self {
        Self { id: Id::unique(("flow-hotkey", id_salt)), value, default_value: None, action_name }
    }

    /// Enables the reset button.
    pub fn default_value(mut self, default: &'a str) -> Self {
        self.default_value = Some(default);
        self
    }

    /// Returns the new binding when one was recorded (or reset).
    pub fn show(self, ui: &mut Ui, link: &Link, events: &[Event]) -> Option<String> {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let mut state: State = ctx.data(|d| d.get_temp(self.id)).unwrap_or_default();
        let mut changed = None;

        if state.recording {
            for event in events {
                if let Event::HotkeysCaptureProgress(partial) = event {
                    state.partial = partial.clone();
                }
            }
            // Leaving the window cancels, as on blur in the TSX.
            if !ctx.input(|i| i.focused) {
                link.send(Request::HotkeysCaptureCancel);
            }
            if let Some(result) = state.capture.as_mut().and_then(Pending::take) {
                match result {
                    Ok(Some(binding)) => match normalize_binding(&binding) {
                        Some(binding) => changed = Some(binding),
                        None => state.error = Some(format!("Flow cannot use “{binding}” as a hotkey.")),
                    },
                    Ok(None) => {}
                    Err(err) => state.error = Some(error_message(&err)),
                }
                state.recording = false;
                state.capture = None;
                state.restore_focus = true;
                ctx.data_mut(|d| {
                    if d.get_temp::<Option<Id>>(active_id()).flatten() == Some(self.id) {
                        d.insert_temp(active_id(), None::<Id>);
                    }
                });
            }
        }

        let mut start = false;
        // `flex flex-col items-end gap-1`, the row right-aligned: laid out right
        // to left, so its items are added last first. Allocated at its height so
        // a centring parent (the `SettingRow` control) centres it.
        let height = if state.error.is_some() { 32.0 + 4.0 + 16.0 } else { 32.0 };
        let size = egui::vec2(ui.available_width(), height);
        ui.allocate_ui_with_layout(size, Layout::top_down(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            // A finite row height: centring in an unbounded height (a scroll area) breaks.
            let width = ui.available_width();
            ui.allocate_ui_with_layout(egui::vec2(width, 32.0), Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if state.recording {
                    if Button::new("Cancel").show(ui).clicked() {
                        link.send(Request::HotkeysCaptureCancel);
                    }
                    let hint =
                        if state.partial.is_empty() { "Press keys… release to save" } else { "Release to save" };
                    text::label(ui, hint, TextStyle::BODY, t.fg_2);
                    if !state.partial.is_empty() {
                        key_caps(ui, &state.partial, false);
                    }
                } else {
                    if let Some(default) = self.default_value {
                        let same = normalize_binding(self.value) == normalize_binding(default);
                        let label = format!("Reset to {default}");
                        if IconButton::new(label, Icon::RotateCcw, 15.0).disabled(same).show(ui).clicked() {
                            changed = Some(default.to_string());
                        }
                    }
                    let record = Button::new("Record").show(ui);
                    // `aria-label`: "Record a new hotkey for <action>".
                    record.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            true,
                            format!("Record a new hotkey for {}", self.action_name),
                        )
                    });
                    if state.restore_focus {
                        state.restore_focus = false;
                        record.request_focus();
                    }
                    if record.clicked() {
                        start = true;
                    }
                    key_caps(ui, self.value, false);
                }
            });
            if let Some(error) = &state.error {
                text::label(ui, error, TextStyle::CAPTION, t.danger);
            }
        });

        if start {
            // Only one recorder listens at a time.
            cancel_capture(&ctx, link);
            state = State { recording: true, capture: Some(link.call(Request::HotkeysCapture)), ..State::default() };
            ctx.data_mut(|d| d.insert_temp(active_id(), Some(self.id)));
        }
        ctx.data_mut(|d| d.insert_temp(self.id, state));
        changed
    }
}
