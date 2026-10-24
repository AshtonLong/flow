//! General: app, microphone and overlay settings. Port of `pages/General.tsx`,
//! and the exemplar for the other pages: read the config through `cx`, build
//! rows from components, write each change with `cx.config.set`.

use egui::Ui;
use flow_core::config::{OverlayPosition, OverlaySize, OverlayStyle, ThemeSetting};
use flow_core::path;

use super::{Cx, Page};
use crate::components::{
    Group, MicMeter, NumberField, Select, SelectOption, SettingRow, Toggle, mic_select, page_header,
};
use crate::util::format::format_duration;

fn themes() -> [SelectOption<ThemeSetting>; 3] {
    [
        SelectOption::new(ThemeSetting::System, "Use Windows setting"),
        SelectOption::new(ThemeSetting::Light, "Light"),
        SelectOption::new(ThemeSetting::Dark, "Dark"),
    ]
}

fn styles() -> [SelectOption<OverlayStyle>; 3] {
    [
        SelectOption::new(OverlayStyle::Pill, "Pill"),
        SelectOption::new(OverlayStyle::Minimal, "Minimal"),
        SelectOption::new(OverlayStyle::None, "Hidden"),
    ]
}

fn position_label(position: OverlayPosition) -> &'static str {
    match position {
        OverlayPosition::BottomCenter => "Bottom centre",
        OverlayPosition::BottomLeft => "Bottom left",
        OverlayPosition::BottomRight => "Bottom right",
        OverlayPosition::TopCenter => "Top centre",
        OverlayPosition::TopLeft => "Top left",
        OverlayPosition::TopRight => "Top right",
    }
}

fn positions() -> Vec<SelectOption<OverlayPosition>> {
    OverlayPosition::ALL.iter().map(|&p| SelectOption::new(p, position_label(p))).collect()
}

fn sizes() -> [SelectOption<OverlaySize>; 3] {
    [
        SelectOption::new(OverlaySize::Small, "Small"),
        SelectOption::new(OverlaySize::Medium, "Medium"),
        SelectOption::new(OverlaySize::Large, "Large"),
    ]
}

#[derive(Default)]
pub struct General {
    /// The open microphone for the level meter; released when the page closes.
    meter: MicMeter,
}

impl Page for General {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let config = cx.config.config();
        let (general, audio, overlay) = (config.general.clone(), config.audio.clone(), config.overlay.clone());
        let overlay_off = overlay.style == OverlayStyle::None;

        page_header(ui, "General", None);

        Group::new("App").show(ui, |ui| {
            SettingRow::new("Launch at login", "Start Flow in the system tray when you sign in to Windows.").show(
                ui,
                |ui| {
                    if let Some(v) = Toggle::new(general.launch_at_login).show(ui).changed {
                        cx.config.set(path!["general", "launch_at_login"], v);
                    }
                },
            );
            SettingRow::new("Theme", "Light, dark, or whatever Windows is set to.").show(ui, |ui| {
                if let Some(v) = Select::new("general.theme", Some(&general.theme), &themes()).show(ui).changed {
                    cx.config.set(path!["general", "theme"], v);
                }
            });
            SettingRow::new("Language", "Flow understands English for now. More languages come in a later version.")
                .show(ui, |ui| {
                    let english = [SelectOption::new("en", "English")];
                    Select::new("general.language", Some(&"en"), &english).disabled(true).show(ui);
                });
        });

        Group::new("Microphone").show(ui, |ui| {
            let mics = cx.mics.list().to_vec();
            let mut listening = false;
            SettingRow::new("Microphone", "The device Flow listens to. Speak to check the level below.").show_with(
                ui,
                |ui| {
                    if let Some(v) = mic_select(ui, "audio.input_device", &audio.input_device, &mics, 280.0).changed {
                        cx.config.set(path!["audio", "input_device"], v);
                    }
                },
                |ui| {
                    ui.add_space(12.0);
                    listening = self.meter.show(ui, &audio.input_device);
                },
            );
            // Device labels can change once the microphone is open.
            if listening {
                cx.mics.refresh();
            }
            SettingRow::new(
                "Keep microphone warm",
                "Recording starts faster, but the Windows microphone indicator stays on all the time.",
            )
            .show(ui, |ui| {
                if let Some(v) = Toggle::new(audio.keep_mic_warm).show(ui).changed {
                    cx.config.set(path!["audio", "keep_mic_warm"], v);
                }
            });
            SettingRow::new("Sounds", "Play a short sound when recording starts and stops.").show(ui, |ui| {
                if let Some(v) = Toggle::new(audio.sounds).show(ui).changed {
                    cx.config.set(path!["audio", "sounds"], v);
                }
            });
            let description = format!(
                "A recording stops by itself after {}, in case a key gets stuck.",
                format_duration(audio.max_recording_seconds as f64 * 1000.0)
            );
            SettingRow::new("Maximum recording length", description).show(ui, |ui| {
                let field = NumberField::new("audio.max_recording_seconds", audio.max_recording_seconds, 5..=3600)
                    .unit("seconds");
                if let Some(v) = field.show(ui).committed {
                    cx.config.set(path!["audio", "max_recording_seconds"], v);
                }
            });
        });

        Group::new("Overlay").description("The small indicator shown on screen while you dictate.").show(ui, |ui| {
            SettingRow::new("Style", "Pill shows status and waveform, Minimal is a small dot, Hidden shows nothing.")
                .show(ui, |ui| {
                    if let Some(v) = Select::new("overlay.style", Some(&overlay.style), &styles()).show(ui).changed {
                        cx.config.set(path!["overlay", "style"], v);
                    }
                });
            SettingRow::new("Position", "Where on the screen the overlay appears.").disabled(overlay_off).show(
                ui,
                |ui| {
                    let options = positions();
                    let select =
                        Select::new("overlay.position", Some(&overlay.position), &options).disabled(overlay_off);
                    if let Some(v) = select.show(ui).changed {
                        cx.config.set(path!["overlay", "position"], v);
                    }
                },
            );
            SettingRow::new("Size", "How large the overlay is drawn.").disabled(overlay_off).show(ui, |ui| {
                let options = sizes();
                let select = Select::new("overlay.size", Some(&overlay.size), &options).disabled(overlay_off);
                if let Some(v) = select.show(ui).changed {
                    cx.config.set(path!["overlay", "size"], v);
                }
            });
            SettingRow::new("Show waveform", "Draw your voice as a live waveform while recording.")
                .disabled(overlay_off)
                .show(ui, |ui| {
                    if let Some(v) = Toggle::new(overlay.show_waveform).disabled(overlay_off).show(ui).changed {
                        cx.config.set(path!["overlay", "show_waveform"], v);
                    }
                });
        });
    }
}
