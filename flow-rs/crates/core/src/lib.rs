//! Flow's platform-independent logic, shared by the main process, the engine
//! process and the settings window: the config schema and store, hotkey
//! bindings and the hotkey matcher, the model catalog, text cleanup rules,
//! per-app profiles, voice-detection segment logic and the IPC protocols.

pub mod catalog;
pub mod cleanup;
pub mod config;
pub mod errors;
pub mod hotkeys;
pub mod matcher;
pub mod profiles;
pub mod protocol;
pub mod split;
pub mod types;
pub mod vad;
pub mod wav;
