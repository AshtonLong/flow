//! Everything OS-specific the dictation path needs: the focused window,
//! simulated input, the clipboard. Mirrors `src/main/platform/{types,index}.ts`.

mod null;
mod win32;

use std::sync::Arc;
use std::time::Duration;

use flow_core::types::FocusedApp;
use tokio_util::sync::CancellationToken;

pub use null::NullPlatform;
pub use win32::{TYPE_CHUNK_CHARS, TYPE_CHUNK_DELAY, Win32Platform};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("typing is unavailable on this platform")]
    Unavailable,
    /// UIPI makes SendInput insert nothing when the foreground window is elevated.
    #[error("SendInput was blocked")]
    Blocked,
    #[error("{0}")]
    Init(String),
}

/// Everything OS-specific the dictation path needs: the focused window, simulated input, the clipboard.
pub trait Platform: Send + Sync {
    /// False when the native layer is unavailable: paste still works, typing and profiles are disabled.
    fn available(&self) -> bool;
    /// Whether this process is elevated.
    fn self_elevated(&self) -> bool;
    /// Foreground window, executable name, title, pid and whether it is elevated.
    fn get_focused_app(&self) -> Option<FocusedApp>;
    fn is_window_alive(&self, window_id: &str) -> bool;
    /// Best-effort SetForegroundWindow. True when the window ends up in the foreground.
    fn focus_window(&self, window_id: &str) -> bool;
    /// False for the desktop, taskbar/shell windows or no foreground window: there is nowhere to type.
    fn is_text_target(&self, app: Option<&FocusedApp>) -> bool;
    /// Types text with SendInput `KEYEVENTF_UNICODE` (newlines as Enter), in chunks.
    /// Stops early and returns `Ok` when cancelled; fails if input is blocked.
    fn type_text(&self, text: &str, cancel: Option<&CancellationToken>) -> Result<(), PlatformError>;
    /// Presses and releases a shortcut such as `Ctrl+V` or `Ctrl+Shift+V` with SendInput.
    fn send_shortcut(&self, binding: &str) -> bool;
    /// Returns once Ctrl/Alt/Shift/Win are all physically up (GetAsyncKeyState), after `timeout`, or on cancel.
    fn wait_for_modifiers_up(&self, timeout: Duration, cancel: Option<&CancellationToken>);
    /// Whether a key or mouse button is down right now. `None` when it cannot be told.
    fn is_key_down(&self, key: &str) -> Option<bool>;
    /// Injects a no-op key (VK 0xE8, as AutoHotkey does) so releasing Win does not open the Start menu.
    fn mask_start_menu(&self);
    /// Puts text on the clipboard flagged to stay out of clipboard history and cloud clipboard
    /// (ExcludeClipboardContentFromMonitorProcessing, CanIncludeInClipboardHistory=0, CanUploadToCloudClipboard=0).
    /// Returns false if it could not, so the caller falls back to a plain clipboard write.
    fn write_clipboard_excluded(&self, text: &str) -> bool;
}

/// The Win32 implementation, else a null implementation with `available() == false`.
pub fn create_platform() -> Arc<dyn Platform> {
    if !cfg!(windows) {
        return Arc::new(NullPlatform::new(format!("no implementation for {}", std::env::consts::OS)));
    }
    match Win32Platform::new() {
        Ok(platform) => Arc::new(platform),
        Err(error) => Arc::new(NullPlatform::new(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod null_platform {
        use super::*;

        #[test]
        fn reports_itself_unavailable_and_degrades_every_call() {
            let platform = NullPlatform::new("koffi failed to load");
            assert!(!platform.available());
            assert_eq!(platform.reason(), "koffi failed to load");
            assert!(!platform.self_elevated());
            assert_eq!(platform.get_focused_app(), None);
            assert!(platform.is_window_alive("1"));
            assert!(!platform.focus_window("1"));
            assert!(platform.is_text_target(None));
            assert!(!platform.send_shortcut("Ctrl+V"));
            assert_eq!(platform.is_key_down("A"), None);
            assert!(!platform.write_clipboard_excluded("x"));
            platform.mask_start_menu();
            platform.wait_for_modifiers_up(Duration::from_millis(1500), None);
            let error = platform.type_text("x", None).unwrap_err();
            assert!(error.to_string().contains("unavailable"));
        }
    }

    mod create_platform {
        use super::*;

        #[test]
        fn never_panics_and_returns_a_usable_platform() {
            let platform = create_platform();
            if !cfg!(windows) {
                assert!(!platform.available());
            }
            let _ = platform.self_elevated();
        }
    }
}
