//! The settings window ↔ main process protocol. Mirrors `SettingsApi` in
//! `src/shared/ipc.ts`.
//!
//! The settings window is a child process (`Flow.exe --settings`) started by
//! the main process with piped stdin and stdout. Each line on the pipe is one
//! JSON message: requests flow up (window → main) and responses and events flow
//! down (main → window). API keys flow one way only: into the main process.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog::{CatalogSnapshot, DownloadProgress};
use crate::config::{ConfigPatch, ConfigSnapshot};
use crate::protocol::Pcm;
#[allow(unused_imports)] // Named in the docs of `Request`.
use crate::types::{AppInfo, HistoryEntry, Transcript};
use crate::types::{DictationEvent, ThemeInfo};

/// A call from the settings window. Each variant documents its result type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    // ── Config ──────────────────────────────────────────────────────────
    /// → [`ConfigSnapshot`]
    ConfigGet,
    /// Patches values in place in `config.toml`. → [`ConfigSnapshot`]
    ConfigSet(Vec<ConfigPatch>),
    /// Opens `config.toml` in the default editor. → `()`
    ConfigOpenFile,
    /// Writes the settings, without API keys, to a path the window picked. → `()`
    ConfigExport(String),
    /// Replaces the settings from a file the window picked. → [`ConfigSnapshot`]
    ConfigImport(String),

    // ── Models ──────────────────────────────────────────────────────────
    /// → [`CatalogSnapshot`]
    ModelsList,
    /// Starts a download; progress arrives as [`Event::ModelsProgress`]. → `()`
    ModelsDownload(String),
    /// → `()`
    ModelsCancel(String),
    /// → `()`
    ModelsRemove(String),
    /// → `()`
    ModelsActivate(String),
    /// Re-fetches the signed remote catalog. → [`CatalogSnapshot`]
    ModelsRefresh,

    // ── Secrets ─────────────────────────────────────────────────────────
    /// Stores a key encrypted with DPAPI. Keys are never sent back. → `()`
    SecretsSet { provider: String, key: String },
    /// → `bool`
    SecretsHas(String),
    /// → `()`
    SecretsDelete(String),
    /// Makes one cheap authenticated request to check the stored key. → [`KeyTestResult`]
    SecretsTest(String),

    // ── Test bench ──────────────────────────────────────────────────────
    /// Runs one recorded phrase through each model; results arrive as
    /// [`Event::BenchResult`]. → run id (`u64`)
    BenchRun { audio: Pcm, model_ids: Vec<String> },

    // ── Hotkey recorder ─────────────────────────────────────────────────
    /// Records the next binding with the global hook (so `Win` and mouse buttons
    /// work). Partial chords arrive as [`Event::HotkeysCaptureProgress`].
    /// → `Option<String>` (None when cancelled)
    HotkeysCapture,
    /// → `()`
    HotkeysCaptureCancel,

    // ── History ─────────────────────────────────────────────────────────
    /// → `Vec<HistoryEntry>`
    HistorySearch { query: String, limit: u32, offset: u32 },
    /// → `()`
    HistoryDelete(i64),
    /// → `()`
    HistoryClear,
    /// Re-applies the current cleanup settings to the stored raw transcript.
    /// → `Option<HistoryEntry>`
    HistoryRerun(i64),

    // ── Dictation ───────────────────────────────────────────────────────
    /// Transcribes audio recorded in the settings window with the active
    /// model. → [`Transcript`]
    DictationTest(Pcm),

    // ── App ─────────────────────────────────────────────────────────────
    /// → [`AppInfo`]
    AppInfo,
    /// → `()`
    AppOpenLogs,
    /// → `()`
    AppOpenModels,
    /// Only `https://` URLs are opened. → `()`
    AppOpenExternal(String),
    /// → `()`
    AppCopy(String),
    /// → `()`
    AppSetPaused(bool),
    /// → `String` (one line describing the result)
    AppCheckUpdates,
    /// → [`ThemeInfo`]
    ThemeGet,
}

/// A message from the window to the main process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

/// A push from the main process to the window. Sent rarely, so the size of
/// the snapshot variants doesn't matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload")]
pub enum Event {
    ConfigChanged(ConfigSnapshot),
    ModelsProgress(DownloadProgress),
    ModelsChanged(CatalogSnapshot),
    BenchResult(BenchResult),
    HotkeysCaptureProgress(String),
    DictationEvent(DictationEvent),
    ThemeChanged(ThemeInfo),
    /// Bring the window to the front (the tray asked for Settings again), and
    /// optionally switch to a page such as `models`.
    Focus {
        page: Option<String>,
    },
    /// The main process is quitting: close the window.
    Quit,
}

/// A message from the main process to the window.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Downstream {
    Response {
        id: u64,
        #[serde(flatten)]
        result: Reply,
    },
    Event(Event),
}

/// The outcome of a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Reply {
    Ok { result: Value },
    Err { error: String },
}

impl Reply {
    pub fn ok<T: Serialize>(value: &T) -> Self {
        match serde_json::to_value(value) {
            Ok(result) => Self::Ok { result },
            Err(err) => Self::Err { error: err.to_string() },
        }
    }
    pub fn err(message: impl Into<String>) -> Self {
        Self::Err { error: message.into() }
    }
    /// Decodes the result into the type the request documents.
    pub fn decode<T: for<'de> Deserialize<'de>>(self) -> Result<T, String> {
        match self {
            Self::Ok { result } => serde_json::from_value(result).map_err(|e| e.to_string()),
            Self::Err { error } => Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchResult {
    /// Identifies the run so stale results can be ignored.
    pub run_id: u64,
    pub model_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Wall-clock milliseconds for this model, including load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
    /// USD, cloud models only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True on the last result of a run.
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyTestResult {
    pub ok: bool,
    pub message: String,
}

/// Validation of requests on the main side, mirroring `ipc-schemas.ts`.
pub mod limits {
    /// At most this many patches per `ConfigSet`.
    pub const MAX_PATCHES: usize = 200;
    /// 16 kHz mono PCM, capped at ten minutes.
    pub const MAX_PCM_SAMPLES: usize = 16_000 * 600;
    pub const MAX_BENCH_MODELS: usize = 24;
    pub const MAX_MODEL_ID: usize = 200;
    pub const MAX_SECRET: usize = 1000;
    pub const MAX_COPY: usize = 100_000;
    pub const MAX_HISTORY_QUERY: usize = 500;
    pub const MAX_HISTORY_LIMIT: u32 = 500;

    /// `^[a-z0-9][a-z0-9._-]*$`, case-insensitive, at most 64 characters.
    pub fn is_provider_id(id: &str) -> bool {
        let bytes = id.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 64
            && bytes[0].is_ascii_alphanumeric()
            && bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PathSeg;

    #[test]
    fn requests_round_trip_as_json_lines() {
        let env = RequestEnvelope {
            id: 7,
            request: Request::ConfigSet(vec![ConfigPatch::set(
                vec![PathSeg::from("audio"), PathSeg::from("sounds")],
                false,
            )]),
        };
        let line = serde_json::to_string(&env).unwrap();
        assert!(!line.contains('\n'));
        let back: RequestEnvelope = serde_json::from_str(&line).unwrap();
        assert_eq!(back, env);

        let unit = serde_json::to_string(&RequestEnvelope { id: 1, request: Request::ConfigGet }).unwrap();
        let back: RequestEnvelope = serde_json::from_str(&unit).unwrap();
        assert_eq!(back.request, Request::ConfigGet);
    }

    #[test]
    fn downstream_messages_are_distinguishable() {
        let response = Downstream::Response { id: 3, result: Reply::ok(&true) };
        let text = serde_json::to_string(&response).unwrap();
        match serde_json::from_str::<Downstream>(&text).unwrap() {
            Downstream::Response { id, result } => {
                assert_eq!(id, 3);
                assert!(result.decode::<bool>().unwrap());
            }
            other => panic!("unexpected {other:?}"),
        }
        let event = Downstream::Event(Event::HotkeysCaptureProgress("Ctrl".into()));
        let text = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Downstream>(&text).unwrap(), event);
        let err = Downstream::Response { id: 4, result: Reply::err("nope") };
        let text = serde_json::to_string(&err).unwrap();
        match serde_json::from_str::<Downstream>(&text).unwrap() {
            Downstream::Response { result, .. } => assert_eq!(result.decode::<()>(), Err("nope".into())),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn provider_ids() {
        assert!(limits::is_provider_id("groq"));
        assert!(limits::is_provider_id("my-server.local_1"));
        assert!(!limits::is_provider_id("-x"));
        assert!(!limits::is_provider_id(""));
        assert!(!limits::is_provider_id("a/b"));
    }
}
