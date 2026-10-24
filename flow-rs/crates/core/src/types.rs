//! Types shared across the main process, the engine and the settings window.
//! Mirrors `src/shared/types.ts`.

use serde::{Deserialize, Serialize};

/// Sample rate of all audio inside Flow: 16 kHz mono float32.
pub const SAMPLE_RATE: u32 = 16_000;

/// Audio length in milliseconds for a number of 16 kHz samples.
pub fn samples_to_ms(samples: usize) -> f64 {
    samples as f64 / SAMPLE_RATE as f64 * 1000.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub text: String,
    /// Model id that produced the text.
    pub model: String,
    pub audio_ms: f64,
    pub elapsed_ms: f64,
}

/// The focused window, used for per-app profiles and for insertion targeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusedApp {
    /// Native window handle as a decimal string (HWND on Windows).
    pub window_id: String,
    /// Executable name, e.g. `slack.exe`.
    pub process_name: String,
    pub title: String,
    pub pid: u32,
    /// True when the window belongs to an elevated (administrator) process.
    #[serde(default)]
    pub elevated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Idle,
    Recording,
    Transcribing,
    Inserting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPhase {
    #[default]
    Hidden,
    Listening,
    Transcribing,
    Done,
    Error,
    Notice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OverlayState {
    pub phase: OverlayPhase,
    /// Audio is being sent to a provider.
    pub cloud: bool,
    /// Recording is locked hands-free (double-tap or toggle).
    #[serde(default)]
    pub locked: bool,
    /// One line shown for `error` and `notice`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Number of earlier dictations still being transcribed.
    #[serde(default)]
    pub queued: u32,
}

impl OverlayState {
    pub fn hidden() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeInfo {
    pub dark: bool,
    /// System accent colour as `#rrggbb`.
    pub accent: String,
    pub high_contrast: bool,
    pub reduced_motion: bool,
}

impl Default for ThemeInfo {
    fn default() -> Self {
        Self { dark: false, accent: "#0067c0".into(), high_contrast: false, reduced_motion: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureErrorReason {
    DeviceMissing,
    Permission,
    Busy,
    Unknown,
}

impl CaptureErrorReason {
    /// The overlay line shown when the reason comes without a message of its own.
    pub fn default_message(self) -> &'static str {
        match self {
            Self::DeviceMissing => "Microphone not found",
            Self::Permission => "Microphone access is blocked",
            Self::Busy => "Microphone is in use by another app",
            Self::Unknown => "Microphone error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InsertOutcome {
    Pasted,
    Typed,
    Clipboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InsertFailure {
    NoTarget,
    TargetClosed,
    Elevated,
    Failed,
}

impl InsertFailure {
    /// The first half of the overlay notice: "<why> — copied to clipboard".
    pub fn describe(self) -> &'static str {
        match self {
            Self::NoTarget => "No text field focused",
            Self::TargetClosed => "The window was closed",
            Self::Elevated => "Cannot type into an administrator window",
            Self::Failed => "Could not insert the text",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertResult {
    pub outcome: InsertOutcome,
    /// Why the text went to the clipboard instead of the cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<InsertFailure>,
}

impl InsertResult {
    pub fn ok(outcome: InsertOutcome) -> Self {
        Self { outcome, reason: None }
    }
    pub fn clipboard(reason: InsertFailure) -> Self {
        Self { outcome: InsertOutcome::Clipboard, reason: Some(reason) }
    }
}

/// One finished (or failed) dictation, broadcast to an open settings window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DictationEvent {
    pub id: u64,
    pub ok: bool,
    pub text: String,
    pub raw_text: String,
    pub model: String,
    pub cloud: bool,
    pub audio_ms: f64,
    /// Key release to inserted text.
    pub elapsed_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<InsertOutcome>,
    /// Set when the active model failed and the fallback produced the text.
    #[serde(default)]
    pub fell_back: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    /// Unix epoch milliseconds.
    pub created_at: i64,
    pub text: String,
    pub raw_text: String,
    pub model: String,
    pub app: String,
    pub audio_ms: i64,
    pub elapsed_ms: i64,
}

/// A history entry before it has an id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewHistoryEntry {
    pub created_at: i64,
    pub text: String,
    pub raw_text: String,
    pub model: String,
    pub app: String,
    pub audio_ms: i64,
    pub elapsed_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDevice {
    pub name: String,
    pub kind: String,
    pub device_type: String,
    pub description: String,
    pub memory_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DegradedModule {
    pub module: String,
    pub effect: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    /// What the app runs on; "Rust" in place of the Electron version.
    pub runtime: String,
    pub platform: String,
    pub config_path: String,
    pub logs_path: String,
    pub models_path: String,
    /// Compute devices the local engine can use; empty until first probed.
    pub devices: Vec<BackendDevice>,
    /// Modules that failed to start, with the feature each one disables.
    pub degraded: Vec<DegradedModule>,
    pub paused: bool,
}

/// How the overlay pill looks; sent whenever the config or the system theme changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayAppearance {
    pub style: crate::config::OverlayStyle,
    pub size: crate::config::OverlaySize,
    pub show_waveform: bool,
    pub sounds: bool,
    pub theme: ThemeInfo,
}

/// What the session asks the capture to do at key-down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStart {
    /// A microphone label as stored in `audio.input_device`, or `"default"`.
    pub device_id: String,
    /// Leave the microphone stream open after stopping, for a faster next start.
    pub keep_warm: bool,
}
