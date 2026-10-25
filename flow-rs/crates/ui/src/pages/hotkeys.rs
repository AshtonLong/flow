//! Hotkeys: one recorder per action, warnings for bindings that clash or do not
//! parse, and the double-tap lock. Port of `pages/Hotkeys.tsx`.

use egui::Ui;
use flow_core::config::{HotkeyAction, HotkeysConfig};
use flow_core::hotkeys::{ConflictKind, hotkey_conflicts};
use flow_core::path;

use super::{Cx, Page};
use crate::components::{Group, HotkeyRecorder, SettingRow, Toggle, page_header};
use crate::design::text::{self, TextStyle};
use crate::design::tokens;

/// `ACTIONS`: the label and explanation of each action, in config order.
pub fn action_info(action: HotkeyAction) -> (&'static str, &'static str) {
    match action {
        HotkeyAction::HoldToTalk => ("Hold to talk", "Records while held. Release to type what you said."),
        HotkeyAction::Toggle => ("Toggle", "Press once to start recording and again to stop."),
        HotkeyAction::Cancel => ("Cancel", "While recording or transcribing, discards it and types nothing."),
        HotkeyAction::PasteLast => ("Paste last", "Inserts the previous transcript again."),
    }
}

/// The label of an action by config key (`hold_to_talk` → "Hold to talk").
fn label_of(key: &str) -> Option<&'static str> {
    HotkeyAction::ALL.into_iter().find(|a| a.key() == key).map(|a| action_info(a).0)
}

/// What is wrong with an action's binding, if anything: it does not parse, or
/// another action has the same keys.
pub fn problem_for(hotkeys: &HotkeysConfig, action: HotkeyAction) -> Option<String> {
    let conflicts = hotkey_conflicts(hotkeys);
    let mine: Vec<_> = conflicts.iter().filter(|c| c.action == action.key()).collect();
    if mine.iter().any(|c| c.kind == ConflictKind::Invalid) {
        return Some(format!("“{}” is not a key Flow recognises. Record a new one.", hotkeys.binding(action)));
    }
    let same: Vec<&str> =
        mine.iter().filter(|c| c.kind == ConflictKind::Same).map(|c| label_of(&c.other).unwrap_or(&c.other)).collect();
    if !same.is_empty() {
        return Some(format!("Same keys as {}. Only one of them can respond, so change one.", same.join(" and ")));
    }
    None
}

#[derive(Default)]
pub struct Hotkeys {}

impl Page for Hotkeys {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        let hotkeys = cx.config.config().hotkeys.clone();
        let defaults = HotkeysConfig::default();

        page_header(
            ui,
            "Hotkeys",
            Some(
                "Any key, combination, lone modifier (such as Right Ctrl) or mouse button works. Hotkeys work in \
                 every app, including full-screen ones.",
            ),
        );

        Group::new("Actions").show(ui, |ui| {
            for action in HotkeyAction::ALL {
                let (label, description) = action_info(action);
                let problem = problem_for(&hotkeys, action);
                let control = |ui: &mut Ui| {
                    let recorder = HotkeyRecorder::new(("hotkeys", action.key()), hotkeys.binding(action), label)
                        .default_value(defaults.binding(action));
                    if let Some(binding) = recorder.show(ui, cx.link, cx.events) {
                        cx.config.set(path!["hotkeys", action.key()], binding);
                    }
                };
                let row = SettingRow::new(label, description);
                match problem {
                    // `role="alert" mt-2 text-caption text-danger`
                    Some(problem) => row.show_with(ui, control, |ui| {
                        ui.add_space(8.0);
                        text::paragraph(ui, problem, TextStyle::CAPTION, t.danger);
                    }),
                    None => row.show(ui, control),
                }
            }
        });

        Group::new("Hands-free").show(ui, |ui| {
            SettingRow::new(
                "Double-tap to lock",
                "Double-tap the hold-to-talk hotkey to keep recording until you tap it again.",
            )
            .show(ui, |ui| {
                if let Some(v) = Toggle::new(hotkeys.double_tap_lock).show(ui).changed {
                    cx.config.set(path!["hotkeys", "double_tap_lock"], v);
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(edit: impl FnOnce(&mut HotkeysConfig)) -> HotkeysConfig {
        let mut hotkeys = HotkeysConfig::default();
        edit(&mut hotkeys);
        hotkeys
    }

    #[test]
    fn the_defaults_have_no_problems() {
        let hotkeys = HotkeysConfig::default();
        for action in HotkeyAction::ALL {
            assert_eq!(problem_for(&hotkeys, action), None);
        }
    }

    #[test]
    fn names_the_actions_that_share_keys() {
        let hotkeys = with(|h| {
            h.toggle = "Ctrl+Win".into();
            h.cancel = "Win+Ctrl".into();
        });
        assert_eq!(
            problem_for(&hotkeys, HotkeyAction::HoldToTalk).as_deref(),
            Some("Same keys as Toggle and Cancel. Only one of them can respond, so change one.")
        );
        assert_eq!(
            problem_for(&hotkeys, HotkeyAction::Cancel).as_deref(),
            Some("Same keys as Hold to talk and Toggle. Only one of them can respond, so change one.")
        );
        assert_eq!(problem_for(&hotkeys, HotkeyAction::PasteLast), None);
    }

    fn page(hotkeys: serde_json::Value) -> crate::testing::PageHarness {
        let mut p = crate::testing::PageHarness::new(
            crate::pages::PageId::Hotkeys,
            serde_json::json!({ "general": { "onboarded": true }, "hotkeys": hotkeys }),
        );
        for _ in 0..4 {
            p.frame();
        }
        p
    }

    #[test]
    fn shows_a_clash_under_both_rows_and_resets_to_the_default() {
        let mut p = page(serde_json::json!({ "toggle": "Ctrl+Win" }));
        assert!(p.h.shows("Same keys as Toggle. Only one of them can respond, so change one."));
        assert!(p.h.shows("Same keys as Hold to talk. Only one of them can respond, so change one."));
        // The reset button is 8 px right of Toggle's Record button (12 px padding after its label).
        let rows = p.h.text_rects("Record");
        assert_eq!(rows.len(), 4);
        let toggle_record = rows[1];
        p.click(egui::pos2(toggle_record.right() + 12.0 + 8.0 + 16.0, toggle_record.center().y));
        for _ in 0..3 {
            p.frame();
        }
        assert_eq!(p.config().hotkeys.toggle, "Ctrl+Win+Space");
        assert!(!p.h.shows("Same keys as Toggle. Only one of them can respond, so change one."));
    }

    #[test]
    fn record_asks_the_main_process_and_saves_what_comes_back() {
        let mut p = page(serde_json::json!({}));
        p.backend.respond_with(|request| match request {
            flow_core::protocol::settings::Request::HotkeysCapture => {
                Some(flow_core::protocol::settings::Reply::ok(&Some("ctrl+alt+k")))
            }
            _ => None,
        });
        let rows = p.h.text_rects("Record");
        p.click(rows[3].center());
        for _ in 0..3 {
            p.frame();
        }
        assert!(p.requests().contains(&flow_core::protocol::settings::Request::HotkeysCapture));
        assert_eq!(p.config().hotkeys.paste_last, "Ctrl+Alt+K", "the binding is saved normalised");
    }

    #[test]
    fn double_tap_lock_toggles() {
        let mut p = page(serde_json::json!({}));
        assert!(p.config().hotkeys.double_tap_lock);
        let on = p.h.text_rect("On").unwrap();
        p.click(egui::pos2(on.right() + 12.0 + 21.0, on.center().y));
        assert!(!p.config().hotkeys.double_tap_lock);
        assert_eq!(p.backend.raw()["hotkeys"]["double_tap_lock"], serde_json::json!(false));
    }

    #[test]
    fn flags_a_binding_that_does_not_parse() {
        let hotkeys = with(|h| h.paste_last = "Ctrl+Banana".into());
        assert_eq!(
            problem_for(&hotkeys, HotkeyAction::PasteLast).as_deref(),
            Some("“Ctrl+Banana” is not a key Flow recognises. Record a new one.")
        );
    }
}
