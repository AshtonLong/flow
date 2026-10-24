//! Every component on one page, for checking the look against the CSS
//! (`--gallery` in the preview example). Not part of the app.

use egui::Ui;

use crate::components::{
    BadgeTone, Button, ChipsInput, ConfirmDialog, Group, HotkeyRecorder, IconButton, Modal, Notice,
    NumberField, Select, SelectOption, SettingRow, TextArea, TextField, Toggle, Tone, badge,
    checkbox, key_caps, level_meter, page_header, progress_bar, radio, spinner,
};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, icons, tokens};
use crate::pages::Cx;

pub(crate) struct Gallery {
    toggle: bool,
    check: bool,
    choice: u8,
    select: String,
    chips: Vec<String>,
    text: String,
    number: i64,
    area: String,
    binding: String,
    confirm: bool,
    modal: bool,
    /// An overlay to open on the first frame (screenshots).
    demo: Option<String>,
}

impl Gallery {
    pub(crate) fn new(demo: Option<String>) -> Self {
        Self {
            demo,
            ..Self::default()
        }
    }
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            demo: None,
            toggle: true,
            check: true,
            choice: 0,
            select: "medium".into(),
            chips: vec!["tory".into(), "towery".into()],
            text: "Ctrl+V".into(),
            number: 300,
            area: "Fix grammar and punctuation.".into(),
            binding: "Ctrl+Win".into(),
            confirm: false,
            modal: false,
        }
    }
}

impl Gallery {
    pub(crate) fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        match self.demo.take().as_deref() {
            Some("select") => crate::components::select::open_select(ui.ctx(), "gallery.select"),
            Some("confirm") => self.confirm = true,
            Some("modal") => self.modal = true,
            Some("issues") => {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::unique("flow-issues-banner"), true));
            }
            _ => {}
        }
        page_header(
            ui,
            "Components",
            Some("Every component of the settings window, in the current theme."),
        );
        let issues = [
            flow_core::config::ConfigIssue {
                path: flow_core::path!["audio", "max_recording_seconds"],
                message: "Too small: expected number to be >=5".into(),
                line: Some(14),
            },
            flow_core::config::ConfigIssue {
                path: flow_core::path!["dictionary", 2usize, "write"],
                message: "Invalid input: expected string, received number".into(),
                line: Some(23),
            },
        ];
        crate::components::issues_banner(ui, &issues, cx.link);

        Group::new("Buttons")
            .description("Standard, accent, subtle, small, danger, disabled, icon.")
            .show(ui, |ui| {
                SettingRow::new("Keycaps", "Press one: it moves 2 px into its wall.").show(
                    ui,
                    |ui| {
                        Button::new("Record").show(ui);
                        Button::new("Set active").accent().show(ui);
                        Button::new("Download").icon(Icon::Download, 15.0).show(ui);
                    },
                );
                SettingRow::new("Quiet and small", "Subtle buttons stay flat.").show(ui, |ui| {
                    Button::new("Open config file").small().subtle().show(ui);
                    Button::new("Remove").small().danger(true).show(ui);
                    IconButton::new("Delete", Icon::Trash2, 15.0).show(ui);
                    IconButton::new("Copy", Icon::Copy, 15.0)
                        .variant(crate::components::Variant::Standard)
                        .show(ui);
                });
                SettingRow::new("Disabled", "No wall, faded text.").show(ui, |ui| {
                    Button::new("Record").disabled(true).show(ui);
                    Button::new("Set active").accent().disabled(true).show(ui);
                    IconButton::new("Reset", Icon::RotateCcw, 15.0)
                        .disabled(true)
                        .show(ui);
                });
            });

        Group::new("Controls").show(ui, |ui| {
            SettingRow::new("Toggle", "With its On/Off text.").show(ui, |ui| {
                if let Some(v) = Toggle::new(self.toggle).show(ui).changed {
                    self.toggle = v;
                }
                Toggle::new(false).disabled(true).hide_state().show(ui);
            });
            SettingRow::new("Checkbox and radio", "20 px, accent when checked.").show(ui, |ui| {
                if let (_, Some(v)) = checkbox(ui, self.check, false) {
                    self.check = v;
                }
                checkbox(ui, false, false);
                for i in 0..2 {
                    if radio(ui, self.choice == i, false).clicked() {
                        self.choice = i;
                    }
                }
            });
            SettingRow::new("Select", "Opens below, or above when there is no room.").show(
                ui,
                |ui| {
                    let options = [
                        SelectOption::new("small".to_string(), "Small"),
                        SelectOption::new("medium".to_string(), "Medium").hint("Recommended"),
                        SelectOption::new("large".to_string(), "Large"),
                        SelectOption::new("huge".to_string(), "Huge").disabled(true),
                    ];
                    if let Some(v) = Select::new("gallery.select", Some(&self.select), &options)
                        .show(ui)
                        .changed
                    {
                        self.select = v;
                    }
                },
            );
            SettingRow::new(
                "Text and numbers",
                "Drafts commit on blur, Enter or a pause.",
            )
            .show(ui, |ui| {
                if let Some(v) = TextField::new("gallery.text", &self.text)
                    .width(140.0)
                    .show(ui)
                    .committed
                {
                    self.text = v;
                }
                if let Some(v) = NumberField::new("gallery.number", self.number, 5..=3600)
                    .unit("seconds")
                    .show(ui)
                    .committed
                {
                    self.number = v;
                }
            });
            SettingRow::new("Hotkey", "Records through the main process.").show(ui, |ui| {
                let recorder = HotkeyRecorder::new("gallery.hotkey", &self.binding, "Hold to talk")
                    .default_value("Ctrl+Win");
                if let Some(v) = recorder.show(ui, cx.link, cx.events) {
                    self.binding = v;
                }
            });
            SettingRow::new("Chips", "Enter or a comma adds; Backspace removes.").show_children(
                ui,
                |ui| {
                    ui.add_space(8.0);
                    if let Some(v) = ChipsInput::new("gallery.chips", &self.chips)
                        .placeholder("Heard as…")
                        .show(ui)
                    {
                        self.chips = v;
                    }
                    ui.add_space(8.0);
                    if let Some(v) = TextArea::new("gallery.area", &self.area).show(ui).committed {
                        self.area = v;
                    }
                },
            );
        });

        Group::new("Status").show(ui, |ui| {
            SettingRow::new("Badges and key caps", "Pills and keys.").show(ui, |ui| {
                badge(ui, "Default", BadgeTone::Neutral);
                badge(ui, "Active", BadgeTone::Accent);
                badge(ui, "Connected", BadgeTone::Ok);
                badge(ui, "Key not working", BadgeTone::Warn);
                key_caps(ui, "Ctrl+Win+Space", false);
            });
            SettingRow::new("Progress", "Determinate, looping, spinner, meter.").show_children(
                ui,
                |ui| {
                    ui.add_space(8.0);
                    progress_bar(ui, Some(0.42));
                    ui.add_space(8.0);
                    progress_bar(ui, None);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        spinner(ui, 15.0, t.fg_2);
                    });
                    ui.add_space(8.0);
                    level_meter(ui, 0.04);
                },
            );
            for (tone, text) in [
                (
                    Tone::Info,
                    "Flow keeps everything on this PC unless you pick a cloud model.",
                ),
                (Tone::Ok, "The key works."),
                (Tone::Warn, "This model needs a GPU."),
                (
                    Tone::Danger,
                    "The change was not saved: the file is read-only.",
                ),
            ] {
                Notice::new(tone, text).show(ui);
            }
        });

        Group::new("Dialogs").show(ui, |ui| {
            SettingRow::new("Confirm and modal", "Smoke behind, centred.").show(ui, |ui| {
                if Button::new("Confirm…").show(ui).clicked() {
                    self.confirm = true;
                }
                if Button::new("Modal…").show(ui).clicked() {
                    self.modal = true;
                }
            });
        });
        ConfirmDialog::new(
            "gallery.confirm",
            "Remove Whisper Small?",
            "This deletes the 493 MB model file from this PC. You can download it again at any time.",
            "Remove model",
        )
        .show(ui.ctx(), &mut self.confirm);
        let (binding, link, events) = (&mut self.binding, cx.link, cx.events);
        Modal::new("gallery.modal", "Edit profile")
            .description("Applies as you change it.")
            .show(ui.ctx(), &mut self.modal, |ui| {
                text::paragraph(ui, "Hold to talk", TextStyle::BODY, t.fg);
                ui.add_space(8.0);
                if let Some(v) =
                    HotkeyRecorder::new("gallery.modal.hotkey", binding, "Hold to talk")
                        .show(ui, link, events)
                {
                    *binding = v;
                }
            });

        Group::new("Icons").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                for icon in Icon::ALL {
                    icons::show(ui, icon, 16.0, 1.75, t.fg);
                }
            });
        });
    }
}
