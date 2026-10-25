//! The profile editor (`Editor` in `Profiles.tsx`): name, matching, the
//! dictation switch and the tri-state overrides. Every change returns the
//! whole new profile; the page writes it at once.

use egui::{Margin, Ui, Vec2, vec2};
use flow_core::config::Profile;
use serde_json::Value;

use crate::components::{ChipsInput, Notice, Select, SelectOption, SettingRow, TextField, Toggle, Tone, card};
use crate::design::text::{self, TextStyle};
use crate::design::{Weight, tokens};
use crate::util::profiles::{get_override, other_overrides, set_override};

/// "Use default": the key is left out of the file.
const DEFAULT: &str = "__default__";

/// What the editor offers that comes from outside the profile.
pub(super) struct Options<'a> {
    /// Ready speech models (plus the profile's own when it is not ready).
    pub models: &'a [SelectOption<String>],
    pub prompts: &'a [SelectOption<String>],
    /// Whether another profile already has this name (case-insensitive).
    pub name_taken: &'a dyn Fn(&str) -> bool,
}

fn on_off() -> Vec<SelectOption<String>> {
    vec![SelectOption::new("true".to_string(), "On"), SelectOption::new("false".to_string(), "Off")]
}

fn shortcuts() -> Vec<SelectOption<String>> {
    ["Ctrl+V", "Ctrl+Shift+V", "Shift+Insert"].iter().map(|s| SelectOption::new(s.to_string(), *s)).collect()
}

fn methods() -> Vec<SelectOption<String>> {
    vec![SelectOption::new("paste".to_string(), "Paste"), SelectOption::new("type".to_string(), "Type")]
}

/// `String(value)` for the select.
fn encode(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn decode_string(v: &str) -> Value {
    Value::String(v.to_string())
}

fn decode_bool(v: &str) -> Value {
    Value::Bool(v == "true")
}

/// One tri-state override picker (`OverrideSelect`).
struct Override<'a> {
    section: &'static str,
    name: &'static str,
    options: &'a [SelectOption<String>],
    width: f32,
    decode: fn(&str) -> Value,
}

impl Override<'_> {
    fn show(self, ui: &mut Ui, profile: &Profile, session: u64, disabled: bool) -> Option<Profile> {
        let value = get_override(profile, self.section, self.name).map(encode).unwrap_or_else(|| DEFAULT.into());
        let mut all = vec![SelectOption::new(DEFAULT.to_string(), "Use default")];
        all.extend(self.options.iter().cloned());
        if !all.iter().any(|o| o.value == value) {
            all.push(SelectOption::new(value.clone(), value.clone()).hint("From the file"));
        }
        let id = ("profiles.override", session, self.section, self.name);
        let picked = Select::new(id, Some(&value), &all).width(self.width).disabled(disabled).show(ui).changed?;
        let next = if picked == DEFAULT { None } else { Some((self.decode)(&picked)) };
        Some(set_override(profile, self.section, self.name, next))
    }
}

/// A `.card px-4 py-3` with a label, a caption and a chips box.
fn chips_card(ui: &mut Ui, label: &str, description: &str, chips: ChipsInput<'_>) -> Option<Vec<String>> {
    let t = tokens(ui.ctx());
    card(ui, Margin::symmetric(16, 12), |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        text::paragraph(ui, label, TextStyle::BODY, t.fg);
        ui.add_space(2.0);
        text::paragraph(ui, description, TextStyle::CAPTION, t.fg_2);
        ui.add_space(8.0);
        chips.show(ui)
    })
    .0
}

/// Draws the editor; returns the changed profile.
pub(super) fn show(ui: &mut Ui, profile: &Profile, session: u64, options: &Options<'_>) -> Option<Profile> {
    let t = tokens(ui.ctx());
    let off = !profile.enabled;
    let mut next: Option<Profile> = None;

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
        SettingRow::new("Name", "Shown in this list only.").show(ui, |ui| {
            let validate = |v: &str| {
                let v = v.trim();
                if v.is_empty() {
                    Some("A profile needs a name.".to_string())
                } else if v != profile.name && (options.name_taken)(v) {
                    Some("Another profile has this name.".to_string())
                } else {
                    None
                }
            };
            let field = TextField::new(("profiles.name", session), &profile.name).width(260.0).validate(&validate);
            if let Some(name) = field.show(ui).committed {
                next = Some(Profile { name: name.trim().to_string(), ..profile.clone() });
            }
        });
        let apps = ChipsInput::new(("profiles.apps", session), &profile.match_process).placeholder("slack.exe").mono();
        if let Some(match_process) = chips_card(
            ui,
            "Apps",
            "The program file names this profile applies to, such as slack.exe. Press Enter after each one.",
            apps,
        ) {
            next = Some(Profile { match_process, ..profile.clone() });
        }
        let titles = ChipsInput::new(("profiles.titles", session), &profile.match_title).placeholder("Gmail");
        if let Some(match_title) = chips_card(
            ui,
            "Window titles",
            "Also applies when the window title contains any of these. Useful for one site in a browser.",
            titles,
        ) {
            next = Some(Profile { match_title, ..profile.clone() });
        }
        SettingRow::new(
            "Disable dictation in this app",
            "Hotkeys do nothing while it is focused. Good for games and password managers.",
        )
        .show(ui, |ui| {
            if let Some(v) = Toggle::new(off).show(ui).changed {
                next = Some(Profile { enabled: !v, ..profile.clone() });
            }
        });
    });

    ui.add_space(20.0);
    text::paragraph(ui, "Overrides", TextStyle::BODY.weight(Weight::Semibold), t.fg);
    ui.add_space(2.0);
    let note = if off {
        "Dictation is off in this app, so these have no effect."
    } else {
        "“Use default” follows your main settings and keeps the key out of the config file."
    };
    text::paragraph(ui, note, TextStyle::CAPTION, t.fg_2);
    ui.add_space(8.0);

    let (on_off, methods, shortcuts) = (on_off(), methods(), shortcuts());
    let rows: [(&str, &str, Override<'_>); 6] = [
        (
            "Model",
            "The speech model to use in this app.",
            Override { section: "model", name: "active", options: options.models, width: 260.0, decode: decode_string },
        ),
        (
            "AI cleanup",
            "Turn the language-model pass on or off here.",
            Override { section: "cleanup", name: "llm_enabled", options: &on_off, width: 220.0, decode: decode_bool },
        ),
        (
            "Cleanup prompt",
            "The style AI cleanup rewrites into.",
            Override {
                section: "cleanup",
                name: "llm_prompt",
                options: options.prompts,
                width: 220.0,
                decode: decode_string,
            },
        ),
        (
            "Remove filler words",
            "Whether “um”, “uh” and repeats are stripped.",
            Override {
                section: "cleanup",
                name: "filler_removal",
                options: &on_off,
                width: 220.0,
                decode: decode_bool,
            },
        ),
        (
            "Insertion method",
            "Paste the text, or type it key by key where paste is blocked.",
            Override { section: "insert", name: "method", options: &methods, width: 220.0, decode: decode_string },
        ),
        (
            "Paste shortcut",
            "The keys this app uses for paste. Terminals often want Ctrl+Shift+V.",
            Override {
                section: "insert",
                name: "paste_shortcut",
                options: &shortcuts,
                width: 220.0,
                decode: decode_string,
            },
        ),
    ];
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
        for (label, description, picker) in rows {
            SettingRow::new(label, description).disabled(off).show(ui, |ui| {
                if let Some(changed) = picker.show(ui, profile, session, off) {
                    next = Some(changed);
                }
            });
        }
    });

    let others = other_overrides(profile);
    if !others.is_empty() {
        ui.add_space(12.0);
        let text =
            format!("This profile also sets {} in the config file. Those are kept as they are.", others.join(", "));
        Notice::new(Tone::Info, &text).show(ui);
    }
    next
}
