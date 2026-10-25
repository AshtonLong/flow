//! Step 1: pick the microphone and watch its level (`StepMicrophone`).

use egui::{Margin, Ui};
use flow_core::path;

use super::step_heading;
use crate::components::{MicMeter, card, mic_select};
use crate::design::text::{self, TextStyle};
use crate::design::tokens;
use crate::pages::Cx;

/// The open microphone for the meter: dropped (released) when the step is left.
#[derive(Default)]
pub struct StepMicrophone {
    meter: MicMeter,
}

impl StepMicrophone {
    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        let device = cx.config.config().audio.input_device.clone();
        step_heading(ui, "Pick your microphone", "Say something. The bar should move as you speak.");
        ui.add_space(20.0);
        // `card mt-5 px-4 py-4`
        let mics = cx.mics.list().to_vec();
        let listening = card(ui, Margin::same(16), |ui| {
            let width = ui.available_width();
            if let Some(v) = mic_select(ui, "onboarding.audio.input_device", &device, &mics, width).changed {
                cx.config.set(path!["audio", "input_device"], v);
            }
            ui.add_space(16.0);
            self.meter.show(ui, &device)
        })
        .0;
        // Device labels can change once the microphone is open.
        if listening {
            cx.mics.refresh();
        }
        ui.add_space(12.0);
        text::paragraph(
            ui,
            "Flow only listens while you hold the hotkey. This check stops when you move on.",
            TextStyle::CAPTION,
            t.fg_2,
        );
    }
}
