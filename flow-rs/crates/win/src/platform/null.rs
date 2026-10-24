use std::time::Duration;

use flow_core::types::FocusedApp;
use tokio_util::sync::CancellationToken;

use super::{Platform, PlatformError};

/// Used when the Win32 layer cannot start or the OS has no implementation. Nothing
/// can be learnt about windows or keys, so the inserter pastes blind.
#[derive(Debug, Clone)]
pub struct NullPlatform {
    reason: String,
}

impl NullPlatform {
    /// `reason`: why the real implementation is not in use, for the "degraded" list.
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl Default for NullPlatform {
    fn default() -> Self {
        Self::new("unsupported platform")
    }
}

impl Platform for NullPlatform {
    fn available(&self) -> bool {
        false
    }
    fn self_elevated(&self) -> bool {
        false
    }
    fn get_focused_app(&self) -> Option<FocusedApp> {
        None
    }
    fn is_window_alive(&self, _window_id: &str) -> bool {
        true
    }
    fn focus_window(&self, _window_id: &str) -> bool {
        false
    }
    fn is_text_target(&self, _app: Option<&FocusedApp>) -> bool {
        true
    }
    fn type_text(&self, _text: &str, _cancel: Option<&CancellationToken>) -> Result<(), PlatformError> {
        Err(PlatformError::Unavailable)
    }
    fn send_shortcut(&self, _binding: &str) -> bool {
        false
    }
    fn wait_for_modifiers_up(&self, _timeout: Duration, _cancel: Option<&CancellationToken>) {}
    fn is_key_down(&self, _key: &str) -> Option<bool> {
        None
    }
    fn mask_start_menu(&self) {}
    fn write_clipboard_excluded(&self, _text: &str) -> bool {
        false
    }
}
