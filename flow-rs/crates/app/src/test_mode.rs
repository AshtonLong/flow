//! `FLOW_TEST=1`: end-to-end tests drive the real app without the desktop, as
//! the Electron suite did through its `__flow` global.
//!
//! - Commands arrive on stdin, one per line: `hotkey hold-start`,
//!   `hotkey hold-end`, `hotkey toggle`, `hotkey cancel`, `hotkey lock`,
//!   `hotkey paste-last`, `settings [page]`, `quit`.
//! - Events leave on stdout as JSON lines: `{"type":"ready"}`,
//!   `{"type":"insert","text":…}`, `{"type":"dictation",…}`,
//!   `{"type":"overlay",…}`.
//! - The global key hook is not started and nothing is pasted anywhere.
//! - `FLOW_FAKE_MIC=<wav>` (read by flow-audio) replaces the microphone.

use std::io::{BufRead, Write};

use flow_core::matcher::HotkeyEvent;
use flow_core::types::{DictationEvent, OverlayState};
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::AppMsg;

pub fn enabled() -> bool {
    std::env::var("FLOW_TEST").as_deref() == Ok("1")
}

static STDOUT: Mutex<()> = Mutex::new(());

fn emit(value: serde_json::Value) {
    let _guard = STDOUT.lock();
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

pub fn ready() {
    emit(json!({ "type": "ready" }));
}

pub fn inserted(text: &str, target: Option<&str>) {
    emit(json!({ "type": "insert", "text": text, "target": target }));
}

pub fn dictation(event: &DictationEvent) {
    emit(json!({ "type": "dictation", "event": event }));
}

pub fn overlay(state: &OverlayState) {
    emit(json!({ "type": "overlay", "state": state }));
}

fn hotkey(name: &str) -> Option<HotkeyEvent> {
    Some(match name {
        "hold-start" => HotkeyEvent::HoldStart,
        "hold-end" => HotkeyEvent::HoldEnd { aborted: false },
        "hold-abort" => HotkeyEvent::HoldEnd { aborted: true },
        "lock" => HotkeyEvent::Lock,
        "toggle" => HotkeyEvent::Toggle,
        "cancel" => HotkeyEvent::Cancel,
        "paste-last" => HotkeyEvent::PasteLast,
        _ => return None,
    })
}

/// Commands that are not hotkeys.
pub enum Command {
    OpenSettings(Option<String>),
    Quit,
}

/// Reads commands from stdin on a thread of its own until EOF (which quits).
pub fn spawn_reader(tx: UnboundedSender<AppMsg>, on_command: impl Fn(Command) + Send + 'static) {
    std::thread::Builder::new()
        .name("flow-test-stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                let mut words = line.split_whitespace();
                match (words.next(), words.next()) {
                    (Some("hotkey"), Some(name)) => {
                        if let Some(event) = hotkey(name) {
                            let _ = tx.send(AppMsg::Hotkey(event));
                        }
                    }
                    (Some("settings"), page) => on_command(Command::OpenSettings(page.map(str::to_string))),
                    (Some("quit"), _) => {
                        on_command(Command::Quit);
                        return;
                    }
                    _ => {}
                }
            }
            on_command(Command::Quit);
        })
        .expect("spawn the test command reader");
}
