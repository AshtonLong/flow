//! Microphone level meters and the microphone picker. Port of
//! `components/LevelMeter.tsx` and `components/MicSelect.tsx`.
//!
//! [`MicMeter`] owns an open microphone, so it is a struct the page keeps:
//! dropping it (leaving the page) releases the device, and a new device
//! reopens it, like the React effect.

use std::sync::Arc;

use egui::epaint::StrokeKind;
use egui::{CornerRadius, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use flow_core::types::CaptureErrorReason;

use super::button::Button;
use super::notice::{Notice, Tone};
use super::select::{Select, SelectOption, SelectResponse};
use crate::adapters::audio::{CaptureError, LevelMonitor, Microphone, MonitorState};
use crate::design::text::{self, TextStyle};
use crate::design::tokens;

/// `.meter-track` / `.meter-fill`: 6 px tall, the one orange thing in the window.
pub fn level_meter(ui: &mut Ui, level: f32) -> Response {
    let t = tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::from(3.0), t.subtle_hover);
    // The raw RMS is tiny for normal speech; a square root reads more like loudness.
    let shown = level.clamp(0.0, 1.0).sqrt();
    if shown > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2(rect.width() * shown, rect.height()));
        painter
            .with_clip_rect(fill.intersect(ui.clip_rect()))
            .rect_filled(rect, CornerRadius::from(3.0), t.voice);
    }
    if let Some(border) = t.hc_border {
        painter.rect_stroke(
            rect,
            CornerRadius::from(3.0),
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
    }
    response
}

/// The help line for a capture error.
pub fn capture_help(error: &CaptureError) -> String {
    match error.reason {
        CaptureErrorReason::DeviceMissing => "This microphone is not connected. Plug it in, or pick another one.".into(),
        CaptureErrorReason::Permission => "Windows is blocking microphone access. In Windows Settings, open Privacy & security > Microphone and allow desktop apps.".into(),
        CaptureErrorReason::Busy => "Another app has exclusive use of this microphone. Close it, then try again.".into(),
        CaptureErrorReason::Unknown if !error.message.is_empty() => {
            format!("The microphone could not be opened. {}", error.message)
        }
        CaptureErrorReason::Unknown => "The microphone could not be opened.".into(),
    }
}

/// Opens the microphone and shows its live level for as long as it is kept.
#[derive(Default)]
pub struct MicMeter {
    device: Option<String>,
    monitor: Option<LevelMonitor>,
    announced: bool,
}

impl MicMeter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shows the meter for `device` (a microphone label, or `"default"`).
    /// Returns true once, when audio first flows (the TSX's `onListening`,
    /// used to refresh device labels).
    pub fn show(&mut self, ui: &mut Ui, device: &str) -> bool {
        if self.device.as_deref() != Some(device) || self.monitor.is_none() {
            self.open(ui, device);
        }
        let state = self
            .monitor
            .as_ref()
            .map(LevelMonitor::state)
            .unwrap_or(MonitorState::Opening);
        let mut listening_now = false;
        match state {
            MonitorState::Failed(error) => {
                let retry = Notice::new(Tone::Danger, &capture_help(&error))
                    .show_with_action(ui, |ui| Button::new("Try again").small().show(ui).clicked());
                if retry {
                    self.open(ui, device);
                }
            }
            state => {
                let t = tokens(ui.ctx());
                let listening = state == MonitorState::Listening;
                if listening && !self.announced {
                    self.announced = true;
                    listening_now = true;
                }
                let level = self
                    .monitor
                    .as_ref()
                    .map(LevelMonitor::level)
                    .unwrap_or(0.0);
                let (row, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
                let label = text::galley(
                    ui,
                    if listening {
                        "Input level"
                    } else {
                        "Opening…"
                    },
                    TextStyle::CAPTION,
                    t.fg_2,
                    76.0,
                );
                ui.painter().galley(
                    pos2(row.left(), row.center().y - label.size().y / 2.0),
                    label,
                    t.fg_2,
                );
                let meter = Rect::from_min_max(
                    pos2(row.left() + 76.0 + 12.0, row.center().y - 3.0),
                    pos2(row.right(), row.center().y + 3.0),
                );
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(meter));
                level_meter(&mut child, level);
            }
        }
        listening_now
    }

    fn open(&mut self, ui: &Ui, device: &str) {
        let ctx = ui.ctx().clone();
        self.monitor = None; // Release the old device first.
        self.device = Some(device.to_string());
        self.announced = false;
        self.monitor = Some(LevelMonitor::start(
            device,
            Arc::new(move || ctx.request_repaint()),
        ));
    }
}

/// Microphone picker. `audio.input_device` stores the microphone label (or
/// `"default"`), so a saved device that is unplugged is still listed.
pub fn mic_select(
    ui: &mut Ui,
    id_salt: &str,
    value: &str,
    mics: &[Microphone],
    width: f32,
) -> SelectResponse<String> {
    let mut options = vec![SelectOption::new("default".to_string(), "System default")];
    let mut seen: Vec<&str> = Vec::new();
    for mic in mics {
        if seen.contains(&mic.label.as_str()) {
            continue;
        }
        seen.push(&mic.label);
        options.push(SelectOption::new(mic.label.clone(), mic.label.clone()));
    }
    if value != "default" && !value.is_empty() && !seen.contains(&value) {
        options.push(SelectOption::new(value.to_string(), value).hint("Not connected"));
    }
    let current = if value.is_empty() {
        "default".to_string()
    } else {
        value.to_string()
    };
    Select::new(id_salt, Some(&current), &options)
        .width(width)
        .show(ui)
}
