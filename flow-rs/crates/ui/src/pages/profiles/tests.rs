//! Profiles page behaviour, through the headless harness. Positions are the
//! Electron layout's, less the column origin (256, 65).

use egui::{Key, pos2};
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

/// Opens the first profile in the editor.
fn edit_first(p: &mut PageHarness) {
    p.click_text("Edit");
    p.frame();
    assert!(p.h.shows("Edit Terminals"), "the editor is open");
}

/// Clicks the last run of `text` painted (the dialog is drawn over the page).
fn click_last(p: &mut PageHarness, text: &str) {
    let rect = *p.h.text_rects(text).last().unwrap_or_else(|| panic!("“{text}” is not on screen"));
    p.click(rect.center());
}

fn escape(p: &mut PageHarness) {
    p.h.key(Key::Escape);
    p.frame();
    p.frame();
}

#[test]
fn escape_in_the_name_field_closes_the_editor_and_keeps_the_new_name() {
    let mut p = harness(two_profiles());
    edit_first(&mut p);
    click_last(&mut p, "Terminals");
    p.h.key(Key::End);
    p.h.type_text(" 2");
    p.frame();
    assert_eq!(names(&p), ["Terminals", "Slack"], "nothing is saved while typing");
    escape(&mut p);
    // Radix's Escape listener (capture phase) closes the dialog before the field
    // can revert; the field's draft is saved as it unmounts.
    assert!(!p.h.shows("Edit Terminals") && !p.h.shows("Edit Terminals 2"), "the editor closed");
    assert_eq!(names(&p), ["Terminals 2", "Slack"]);
}

#[test]
fn escape_closes_an_open_list_before_the_editor() {
    let mut p = harness(two_profiles());
    // Tall enough for the whole editor, so its rows are not scrolled out of view.
    p.h.screen = egui::vec2(1000.0, 1400.0);
    edit_first(&mut p);
    p.frame();
    let defaults = p.h.text_rects("Use default").len();
    assert_eq!(defaults, 4, "the overrides left at their default");
    // "AI cleanup" is Off for Terminals: its select is the last "Off".
    click_last(&mut p, "Off");
    p.frame();
    assert_eq!(p.h.text_rects("Use default").len(), defaults + 1, "the list is open");
    escape(&mut p);
    assert_eq!(p.h.text_rects("Use default").len(), defaults, "the list closed");
    assert!(p.h.shows("Edit Terminals"), "the editor stays open");
    escape(&mut p);
    assert!(!p.h.shows("Edit Terminals"));
}

#[test]
fn escape_drops_an_app_name_that_was_not_entered() {
    let mut p = harness(two_profiles());
    edit_first(&mut p);
    // The chips box, right of its last chip.
    let chip = *p.h.text_rects("wezterm-gui.exe").last().expect("the Apps chips");
    p.click(chip.right_center() + egui::vec2(80.0, 0.0));
    p.h.type_text("alacritty.exe");
    p.frame();
    escape(&mut p);
    assert!(!p.h.shows("Edit Terminals"));
    // React unmounts the box without a blur, so the typed text is not added.
    assert_eq!(p.config().profiles[0].match_process, ["WindowsTerminal.exe", "wezterm-gui.exe"]);
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
