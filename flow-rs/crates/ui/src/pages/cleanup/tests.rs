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

#[test]
fn a_local_rule_toggles() {
    let mut p = harness(json!({ "general": { "onboarded": true } }));
    assert!(p.config().cleanup.filler_removal);
    // "Remove filler words": the switch in the first card of "Local rules".
    p.click(pos2(658.0, 175.0));
    assert!(!p.config().cleanup.filler_removal);
    assert_eq!(p.backend.raw()["cleanup"]["filler_removal"], json!(false));
}
