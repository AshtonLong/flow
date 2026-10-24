//! The config schema. `config.toml` is validated against this, and the settings
//! window edits the same shape. Every option has a default, so an empty file is
//! a valid config. Mirrors `src/shared/config.ts`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const CONFIG_VERSION: i64 = 1;
pub const DEFAULT_LOCAL_MODEL: &str = "parakeet-tdt-0.6b-v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSetting {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeSetting {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub launch_at_login: bool,
    pub theme: ThemeSetting,
    /// Only `"en"` in v1.
    pub language: String,
    pub debug_logging: bool,
    pub onboarded: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            theme: ThemeSetting::System,
            language: "en".into(),
            debug_logging: false,
            onboarded: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeysConfig {
    pub hold_to_talk: String,
    pub toggle: String,
    pub cancel: String,
    pub paste_last: String,
    pub double_tap_lock: bool,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self {
            hold_to_talk: "Ctrl+Win".into(),
            toggle: "Ctrl+Win+Space".into(),
            cancel: "Esc".into(),
            paste_last: "Alt+Shift+V".into(),
            double_tap_lock: true,
        }
    }
}

impl HotkeysConfig {
    /// The binding text for an action.
    pub fn binding(&self, action: HotkeyAction) -> &str {
        match action {
            HotkeyAction::HoldToTalk => &self.hold_to_talk,
            HotkeyAction::Toggle => &self.toggle,
            HotkeyAction::Cancel => &self.cancel,
            HotkeyAction::PasteLast => &self.paste_last,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    HoldToTalk,
    Toggle,
    Cancel,
    PasteLast,
}

impl HotkeyAction {
    pub const ALL: [Self; 4] = [Self::HoldToTalk, Self::Toggle, Self::Cancel, Self::PasteLast];
    /// The config key, e.g. `hold_to_talk`.
    pub fn key(self) -> &'static str {
        match self {
            Self::HoldToTalk => "hold_to_talk",
            Self::Toggle => "toggle",
            Self::Cancel => "cancel",
            Self::PasteLast => "paste_last",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub input_device: String,
    pub keep_mic_warm: bool,
    pub sounds: bool,
    /// 5 to 3600.
    pub max_recording_seconds: i64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: "default".into(),
            keep_mic_warm: false,
            sounds: true,
            max_recording_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeviceSetting {
    #[default]
    Auto,
    Cpu,
    Gpu,
}

impl DeviceSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub active: String,
    pub fallback: String,
    pub device: DeviceSetting,
    /// -1 (never unload) to 1440; 0 unloads at once.
    pub keep_loaded_minutes: f64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            active: DEFAULT_LOCAL_MODEL.into(),
            fallback: DEFAULT_LOCAL_MODEL.into(),
            device: DeviceSetting::Auto,
            keep_loaded_minutes: 10.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// Required for custom endpoints; optional override for built-in providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// A reference such as `secret:groq`. The key itself is never stored here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Model name sent to a custom OpenAI-compatible endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupConfig {
    pub filler_removal: bool,
    pub spoken_formatting: bool,
    pub spoken_punctuation: bool,
    pub llm_enabled: bool,
    pub llm_model: String,
    pub llm_prompt: String,
    /// 500 to 30000.
    pub llm_timeout_ms: i64,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            filler_removal: true,
            spoken_formatting: true,
            spoken_punctuation: true,
            llm_enabled: false,
            llm_model: "groq/openai/gpt-oss-20b".into(),
            llm_prompt: "clean".into(),
            llm_timeout_ms: 3000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptConfig {
    pub text: String,
}

/// Built-in cleanup prompts, in display order.
pub const DEFAULT_PROMPTS: [(&str, &str); 5] = [
    ("clean", "Fix grammar and punctuation. Remove false starts. Keep the speaker's words and tone."),
    (
        "email",
        "Rewrite as a clear, polite email body. Use complete sentences and paragraphs. Keep every fact and name.",
    ),
    (
        "casual",
        "Tidy this into a relaxed chat message. Keep it short and conversational. Do not add greetings or sign-offs.",
    ),
    (
        "code_comment",
        "Rewrite as a concise code comment. Keep identifiers and technical terms exactly as spoken. No markdown.",
    ),
    ("verbatim", "Fix only obvious transcription errors and punctuation. Do not rephrase or remove anything."),
];

pub fn default_prompt(name: &str) -> Option<&'static str> {
    DEFAULT_PROMPTS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InsertMethod {
    #[default]
    Paste,
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InsertConfig {
    pub method: InsertMethod,
    pub paste_shortcut: String,
    pub restore_clipboard: bool,
    pub trailing_space: bool,
    pub match_case: bool,
}

impl Default for InsertConfig {
    fn default() -> Self {
        Self {
            method: InsertMethod::Paste,
            paste_shortcut: "Ctrl+V".into(),
            restore_clipboard: true,
            trailing_space: true,
            match_case: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OverlayStyle {
    #[default]
    Pill,
    Minimal,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum OverlayPosition {
    #[default]
    BottomCenter,
    BottomLeft,
    BottomRight,
    TopCenter,
    TopLeft,
    TopRight,
}

impl OverlayPosition {
    pub const ALL: [Self; 6] = [
        Self::BottomCenter,
        Self::BottomLeft,
        Self::BottomRight,
        Self::TopCenter,
        Self::TopLeft,
        Self::TopRight,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BottomCenter => "bottom-center",
            Self::BottomLeft => "bottom-left",
            Self::BottomRight => "bottom-right",
            Self::TopCenter => "top-center",
            Self::TopLeft => "top-left",
            Self::TopRight => "top-right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OverlaySize {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    pub style: OverlayStyle,
    pub position: OverlayPosition,
    pub size: OverlaySize,
    pub show_waveform: bool,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            style: OverlayStyle::Pill,
            position: OverlayPosition::BottomCenter,
            size: OverlaySize::Medium,
            show_waveform: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    pub enabled: bool,
    /// 1 to 3650.
    pub retention_days: i64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { enabled: false, retention_days: 30 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub heard: Vec<String>,
    pub write: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub trigger: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalModelFile {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub path: String,
}

/// Sections a profile may override.
pub const PROFILE_SECTIONS: [&str; 6] = ["audio", "model", "cleanup", "insert", "overlay", "history"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub match_process: Vec<String>,
    #[serde(default)]
    pub match_title: Vec<String>,
    /// `false` disables dictation entirely while the app is focused.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Per-app override sections are loose here; the merged result is re-validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insert: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Map<String, Value>>,
}

fn yes() -> bool {
    true
}

impl Profile {
    /// The override table for a section name from [`PROFILE_SECTIONS`].
    pub fn section(&self, name: &str) -> Option<&Map<String, Value>> {
        match name {
            "audio" => self.audio.as_ref(),
            "model" => self.model.as_ref(),
            "cleanup" => self.cleanup.as_ref(),
            "insert" => self.insert.as_ref(),
            "overlay" => self.overlay.as_ref(),
            "history" => self.history.as_ref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: i64,
    pub general: GeneralConfig,
    pub hotkeys: HotkeysConfig,
    pub audio: AudioConfig,
    pub model: ModelConfig,
    pub providers: IndexMap<String, ProviderConfig>,
    pub cleanup: CleanupConfig,
    pub prompts: IndexMap<String, PromptConfig>,
    pub insert: InsertConfig,
    pub overlay: OverlayConfig,
    pub history: HistoryConfig,
    pub dictionary: Vec<DictionaryEntry>,
    pub snippets: Vec<Snippet>,
    pub local_models: Vec<LocalModelFile>,
    pub profiles: Vec<Profile>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            general: GeneralConfig::default(),
            hotkeys: HotkeysConfig::default(),
            audio: AudioConfig::default(),
            model: ModelConfig::default(),
            providers: IndexMap::new(),
            cleanup: CleanupConfig::default(),
            prompts: IndexMap::new(),
            insert: InsertConfig::default(),
            overlay: OverlayConfig::default(),
            history: HistoryConfig::default(),
            dictionary: Vec::new(),
            snippets: Vec::new(),
            local_models: Vec::new(),
            profiles: Vec::new(),
        }
    }
}

impl Config {
    /// The config as a plain JSON value (what patches and validation operate on).
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("config serialises")
    }
}

/// The prompt text for a prompt name, falling back to the built-in templates.
pub fn resolve_prompt(config: &Config, name: &str) -> String {
    if let Some(p) = config.prompts.get(name) {
        return p.text.clone();
    }
    default_prompt(name).or_else(|| default_prompt("clean")).unwrap_or_default().to_string()
}

/// Built-in and user prompt names, built-ins first.
pub fn prompt_names(config: &Config) -> Vec<String> {
    let mut names: Vec<String> = DEFAULT_PROMPTS.iter().map(|(n, _)| n.to_string()).collect();
    for name in config.prompts.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// One segment of a config path: a table key or an array index.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathSeg {
    Index(usize),
    Key(String),
}

impl From<&str> for PathSeg {
    fn from(value: &str) -> Self {
        Self::Key(value.to_string())
    }
}
impl From<String> for PathSeg {
    fn from(value: String) -> Self {
        Self::Key(value)
    }
}
impl From<usize> for PathSeg {
    fn from(value: usize) -> Self {
        Self::Index(value)
    }
}

impl std::fmt::Display for PathSeg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(i) => write!(f, "{i}"),
            Self::Key(k) => f.write_str(k),
        }
    }
}

pub type ConfigPath = Vec<PathSeg>;

/// Builds a [`ConfigPath`]: `path!["audio", "sounds"]`, `path!["dictionary", 0usize]`.
#[macro_export]
macro_rules! path {
    ($($seg:expr),* $(,)?) => { vec![$($crate::config::PathSeg::from($seg)),*] };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigIssue {
    /// Path of the offending key, e.g. `["audio", "max_recording_seconds"]`.
    pub path: ConfigPath,
    pub message: String,
    /// 1-based line in `config.toml`, when it can be located.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// One edit to the config file. `value: None` removes the key (or splices an array item).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigPatch {
    pub path: ConfigPath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

impl ConfigPatch {
    pub fn set(path: ConfigPath, value: impl Into<Value>) -> Self {
        Self { path, value: Some(value.into()) }
    }
    pub fn remove(path: ConfigPath) -> Self {
        Self { path, value: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub config: Config,
    pub issues: Vec<ConfigIssue>,
    pub path: String,
}
