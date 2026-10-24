//! `FLOW_TEST=1`: end-to-end tests drive the real app without the desktop, as
//! the Electron suite did through its `__flow` global.
//!
//! - Commands arrive on stdin, one per line: `hotkey hold-start`,
//!   `hotkey hold-end`, `hotkey toggle`, `hotkey cancel`, `hotkey lock`,
//!   `hotkey paste-last`, `settings [page]`, `quit`, and
//!   `ipc {"id":1,"method":"ConfigGet"}`: a settings-window request, answered
//!   by the same handler the window uses (the Electron suite's
//!   `settings.evaluate(() => window.flow.…)`).
//! - `{"type":"reply","id":…,"reply":…}` answers an `ipc` command and
//!   `{"type":"settings","pid":…}` reports that the settings window opened.
//! - Events leave on stdout as JSON lines: `{"type":"ready"}`,
//!   `{"type":"insert","text":…}`, `{"type":"dictation",…}`,
//!   `{"type":"overlay",…}`.
//! - The global key hook is not started and nothing is pasted anywhere.
//! - `FLOW_FAKE_MIC=<wav>` (read by flow-audio) replaces the microphone.

use std::io::{BufRead, Write};

use flow_core::matcher::HotkeyEvent;
use flow_core::protocol::settings::{Reply, RequestEnvelope};
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

pub fn reply(id: u64, reply: &Reply) {
    emit(json!({ "type": "reply", "id": id, "reply": reply }));
}

pub fn settings_opened(pid: u32) {
    if enabled() {
        emit(json!({ "type": "settings", "pid": pid }));
    }
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
    Ipc(RequestEnvelope),
    Quit,
}

/// Reads commands from stdin on a thread of its own until EOF (which quits).
pub fn spawn_reader(tx: UnboundedSender<AppMsg>, on_command: impl Fn(Command) + Send + 'static) {
    std::thread::Builder::new()
        .name("flow-test-stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                // .NET writes a byte-order mark before the first line.
                let mut words = line.trim_start_matches('\u{feff}').split_whitespace();
                match (words.next(), words.next()) {
                    (Some("hotkey"), Some(name)) => {
                        if let Some(event) = hotkey(name) {
                            let _ = tx.send(AppMsg::Hotkey(event));
                        }
                    }
                    (Some("settings"), page) => on_command(Command::OpenSettings(page.map(str::to_string))),
                    (Some("ipc"), Some(_)) => {
                        let json = line.split_once("ipc").map_or("", |(_, rest)| rest.trim());
                        match serde_json::from_str::<RequestEnvelope>(json) {
                            Ok(envelope) => on_command(Command::Ipc(envelope)),
                            Err(err) => emit(json!({ "type": "error", "message": err.to_string() })),
                        }
                    }
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

/// Reads a 16-bit PCM mono WAV into float samples.
pub fn read_wav(path: &std::path::Path) -> std::io::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size =
            u32::from_le_bytes([bytes[offset + 4], bytes[offset + 5], bytes[offset + 6], bytes[offset + 7]]) as usize;
        if id == b"data" {
            let data = &bytes[offset + 8..(offset + 8 + size).min(bytes.len())];
            return Ok(data.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect());
        }
        offset += 8 + size + (size & 1);
    }
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no data chunk"))
}

/// `FLOW_FAKE_MIC=<wav>`: a microphone that "speaks" the whole file as soon as
/// it starts, in 100 ms chunks, then flushes when stopped.
pub struct FakeMic {
    samples: std::sync::Arc<Vec<f32>>,
    tx: UnboundedSender<AppMsg>,
}

impl FakeMic {
    pub fn from_env(tx: UnboundedSender<AppMsg>) -> Option<Self> {
        let path = std::env::var_os("FLOW_FAKE_MIC")?;
        match read_wav(std::path::Path::new(&path)) {
            Ok(samples) => Some(Self { samples: std::sync::Arc::new(samples), tx }),
            Err(err) => {
                log::error!(target: "test", "FLOW_FAKE_MIC: {err}");
                None
            }
        }
    }
}

impl crate::session::SessionCapture for FakeMic {
    fn start_capture(&self, _options: flow_core::types::CaptureStart) {
        use flow_audio::CaptureEvent;
        let _ = self.tx.send(AppMsg::Capture(CaptureEvent::Started));
        for chunk in self.samples.chunks(1600) {
            let _ = self.tx.send(AppMsg::Capture(CaptureEvent::Chunk(chunk.to_vec())));
        }
    }

    fn stop_capture(&self) {
        let _ = self.tx.send(AppMsg::Capture(flow_audio::CaptureEvent::Stopped));
    }
}
