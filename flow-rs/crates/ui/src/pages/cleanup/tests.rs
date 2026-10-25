//! Cleanup page behaviour, through the headless harness. Positions are the
//! Electron layout's, less the column origin (256, 65).

use egui::{Key, pos2};
use flow_core::protocol::settings::Request;
use serde_json::{Value, json};

use crate::pages::PageId;
use crate::testing::PageHarness;

fn dictionary() -> Value {
    json!({
        "general": { "onboarded": true },
        "dictionary": [
            { "heard": ["tory", "towery"], "write": "Tauri" },
            { "heard": ["pre a", "priya"], "write": "Priya" },
            { "heard": ["k eight s", "kates"], "write": "k8s" },
        ],
    })
}

fn harness(raw: Value) -> PageHarness {
    let mut p = PageHarness::new(PageId::Cleanup, raw);
    for _ in 0..3 {
        p.frame();
    }
    p
}

fn writes(p: &PageHarness) -> usize {
    p.backend.requests.lock().iter().filter(|r| matches!(r, Request::ConfigSet(_))).count()
}

/// The first row's "Write" box, right of its text, and its Remove button.
const FIRST_WRITE: egui::Pos2 = pos2(620.0, 488.0);
const FIRST_REMOVE: egui::Pos2 = pos2(663.0, 488.0);
const ADD_WORD: egui::Pos2 = pos2(640.0, 411.0);

#[test]
fn editing_a_dictionary_row_writes_the_whole_list() {
    let mut p = harness(dictionary());
    p.click(FIRST_WRITE);
    p.h.type_text("x ");
    p.frame();
    p.h.key(Key::Enter);
    p.frame();
    p.frame();
    assert_eq!(
        p.backend.raw()["dictionary"],
        json!([
            { "heard": ["tory", "towery"], "write": "Taurix" },
            { "heard": ["pre a", "priya"], "write": "Priya" },
            { "heard": ["k eight s", "kates"], "write": "k8s" },
        ]),
        "the written text is trimmed"
    );
}

#[test]
fn a_new_row_is_not_written_until_it_is_complete() {
    let mut p = harness(dictionary());
    p.click(ADD_WORD);
    p.frame();
    assert_eq!(writes(&p), 0, "an empty row stays on screen only");

    // Removing a saved row writes the complete rows, still without the empty one.
    p.click(FIRST_REMOVE);
    p.frame();
    assert_eq!(writes(&p), 1);
    assert_eq!(
        p.backend.raw()["dictionary"],
        json!([
            { "heard": ["pre a", "priya"], "write": "Priya" },
            { "heard": ["k eight s", "kates"], "write": "k8s" },
        ])
    );
    assert_eq!(p.config().dictionary.len(), 2);
}

/// The prompts accordion with a user prompt, on a window tall enough to show it.
fn prompts() -> PageHarness {
    let mut p = harness(json!({
        "general": { "onboarded": true },
        "prompts": { "standup": { "text": "Bullet points, one per update." } },
    }));
    p.h.screen = egui::vec2(1000.0, 3000.0);
    p.frame();
    p.frame();
    p
}

/// Whether the focused widget holds the last run of `text` painted (an accordion header).
fn focused_on(p: &PageHarness, text: &str) -> bool {
    let label = *p.h.text_rects(text).last().unwrap_or_else(|| panic!("“{text}” is not on screen"));
    p.h.focused_rect().is_some_and(|r| r.contains(label.center()))
}

fn press(p: &mut PageHarness, key: Key) {
    p.h.key(key);
    p.frame();
    p.frame();
}

#[test]
fn arrow_keys_move_between_prompt_headers_only() {
    let mut p = prompts();
    // A click opens "Email" and focuses its header, as it focuses the <button>.
    let email = p.h.text_rects("Email").last().expect("the Email header").center();
    p.click(email);
    p.frame();
    assert!(p.h.shows("A built-in prompt. Reset brings back the original wording."), "Email is open");
    assert!(focused_on(&p, "Email"));

    // Down skips the open item's text box and goes to the next header.
    press(&mut p, Key::ArrowDown);
    assert!(focused_on(&p, "Casual"));
    press(&mut p, Key::ArrowUp);
    press(&mut p, Key::ArrowUp);
    assert!(focused_on(&p, "Clean"));
    press(&mut p, Key::ArrowUp);
    assert!(focused_on(&p, "standup"), "Up from the first wraps to the last");
    press(&mut p, Key::ArrowDown);
    assert!(focused_on(&p, "Clean"), "Down from the last wraps to the first");
    press(&mut p, Key::End);
    assert!(focused_on(&p, "standup"));
    press(&mut p, Key::Home);
    assert!(focused_on(&p, "Clean"));
    // Left and Right do nothing in a vertical accordion.
    press(&mut p, Key::ArrowRight);
    assert!(focused_on(&p, "Clean"));
    assert!(p.h.shows("A built-in prompt. Reset brings back the original wording."), "Email is still open");
}

#[test]
fn arrow_keys_inside_an_open_prompt_stay_there() {
    let mut p = prompts();
    let standup = p.h.text_rects("standup").last().expect("the standup header").center();
    p.click(standup);
    p.frame();
    // Tab into the text box, then on to "Delete prompt".
    press(&mut p, Key::Tab);
    let text_box = p.h.focused_rect().expect("the prompt text box");
    assert!(text_box.height() > 48.0, "the three-row text box");
    press(&mut p, Key::ArrowDown);
    assert_eq!(p.h.focused_rect(), Some(text_box), "Down moves the caret, not the focus");
    press(&mut p, Key::Tab);
    assert!(focused_on(&p, "Delete prompt"));
    for key in [Key::ArrowUp, Key::ArrowDown, Key::Home, Key::End] {
        press(&mut p, key);
        assert!(focused_on(&p, "Delete prompt"), "{key:?} leaves the focus on the button");
    }
}

#[test]
fn a_local_rule_toggles() {
    let mut p = harness(json!({ "general": { "onboarded": true } }));
    assert!(p.config().cleanup.filler_removal);
    // "Remove filler words": the switch in the first card of "Local rules".
    p.click(pos2(658.0, 175.0));
    assert!(!p.config().cleanup.filler_removal);
    assert_eq!(p.backend.raw()["cleanup"]["filler_removal"], json!(false));
}
