//! Flow's Windows input and output: the global key hook, focused-app lookup,
//! simulated input and clipboard insertion. Ports `src/main/platform/*`,
//! `src/main/input/{hook,keymap}.ts` and `src/main/insert/*`.

pub mod clipboard;
pub mod hook;
pub mod injected;
pub mod inserter;
pub mod keymap;
pub mod platform;
pub mod vk;

pub use clipboard::{Clipboard, ClipboardEntry, ClipboardError, ClipboardSnapshot, Win32Clipboard, system_clipboard};
pub use hook::{HookBackend, InputEvent, InputHook, InputHookDeps, InputSink, Win32HookBackend};
pub use injected::{INJECT_TAG, shortcut_keys};
pub use inserter::{InsertOptions, Inserter, InserterOptions};
pub use platform::{NullPlatform, Platform, PlatformError, Win32Platform, create_platform};
