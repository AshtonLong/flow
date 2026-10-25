//! Profiles page behaviour, through the headless harness. Positions are the
//! Electron layout's, less the column origin (256, 65).

use egui::pos2;
use serde_json::{Value, json};

use crate::pages::PageId;
use crate::testing::PageHarness;

fn two_profiles() -> Value {
    json!({
        "general": { "onboarded": true },
        "profiles": [
            {
                "name": "Terminals",
                "match_process": ["WindowsTerminal.exe", "wezterm-gui.exe"],
                "insert": { "paste_shortcut": "Ctrl+Shift+V" },
                "cleanup": { "llm_enabled": false },
            },
            { "name": "Slack", "match_process": ["slack.exe"], "cleanup": { "llm_prompt": "casual" } },
        ],
    })
}

fn harness(raw: Value) -> PageHarness {
    let mut p = PageHarness::new(PageId::Profiles, raw);
    for _ in 0..3 {
        p.frame();
    }
    p
}

fn names(p: &PageHarness) -> Vec<String> {
    p.config().profiles.iter().map(|p| p.name.clone()).collect()
}

/// Delete on the first card, then the dialog's buttons (440 px wide, centred in 1000×700).
const DELETE_FIRST: egui::Pos2 = pos2(663.0, 184.0);
const CONFIRM: egui::Pos2 = pos2(507.0, 410.0);
const CANCEL: egui::Pos2 = pos2(636.0, 410.0);

#[test]
fn removing_a_profile_asks_for_confirmation_first() {
    let mut p = harness(two_profiles());
    p.click(DELETE_FIRST);
    p.frame();
    assert_eq!(names(&p), ["Terminals", "Slack"], "nothing is removed before confirming");
    p.click(CANCEL);
    p.frame();
    assert_eq!(names(&p), ["Terminals", "Slack"], "Cancel keeps the profile");

    p.click(DELETE_FIRST);
    p.frame();
    p.click(CONFIRM);
    p.frame();
    assert_eq!(names(&p), ["Slack"]);
    assert_eq!(
        p.backend.raw()["profiles"],
        json!([{ "name": "Slack", "match_process": ["slack.exe"], "cleanup": { "llm_prompt": "casual" } }]),
        "the remaining profile is written without defaults"
    );
}

#[test]
fn adding_a_profile_writes_a_free_name_and_opens_the_editor() {
    let mut p = harness(json!({ "general": { "onboarded": true }, "profiles": [{ "name": "New profile" }] }));
    // "Add profile" beside the group heading.
    p.click(pos2(634.0, 119.0));
    p.frame();
    assert_eq!(names(&p), ["New profile", "New profile 2"]);
    assert_eq!(p.backend.raw()["profiles"][1], json!({ "name": "New profile 2" }));
    // The editor is open over the page, so the page's button does nothing.
    p.click(pos2(634.0, 119.0));
    p.frame();
    assert_eq!(names(&p).len(), 2, "the modal editor covers the page");
    // Its Done button (bottom right of the 640 px dialog) closes it.
    p.click(pos2(736.0, 635.0));
    p.frame();
    // Closed: the page's own Add profile button works again.
    p.click(pos2(634.0, 119.0));
    p.frame();
    assert_eq!(names(&p).len(), 3);
}

#[test]
fn a_suggestion_is_added_with_its_overrides() {
    let mut p = harness(json!({ "general": { "onboarded": true } }));
    // Empty state, then "Suggestions": Terminals' Add is the first card's button.
    p.click(pos2(654.0, 304.0));
    p.frame();
    assert_eq!(names(&p), ["Terminals"]);
    assert_eq!(
        p.backend.raw()["profiles"][0],
        json!({
            "name": "Terminals",
            "match_process": ["WindowsTerminal.exe", "wezterm-gui.exe", "alacritty.exe"],
            "cleanup": { "llm_enabled": false },
            "insert": { "paste_shortcut": "Ctrl+Shift+V" },
        })
    );
}
