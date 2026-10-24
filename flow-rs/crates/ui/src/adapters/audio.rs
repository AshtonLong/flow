//! The settings window's view of the microphone: the device list and a level
//! monitor for the meter. Stands in for `listMicrophones` / `startCapture` in
//! `src/renderer/common/capture.ts`.
//!
//! ADAPTER: `flow_audio` is being written in parallel. Its contract (PORTING.md)
//! is `list_microphones() -> Vec<Microphone>`, `same_device(a, b)` and a
//! `LevelMonitor`. Until it lands, this file answers with no devices and a
//! silent meter, so the window still lays out as it will. To wire it up, fill
//! in the three `ADAPTER:` spots below; nothing else in `flow-ui` touches audio.

use std::sync::Arc;

use flow_core::types::CaptureErrorReason;
use parking_lot::Mutex;

/// A capture device, labelled as stored in `audio.input_device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    pub label: String,
    pub id: String,
}

/// Connected microphones.
pub fn list_microphones() -> Vec<Microphone> {
    // ADAPTER: flow_audio::list_microphones().into_iter().map(|m| Microphone { label: m.label, id: m.id }).collect()
    Vec::new()
}

/// Electron labels carry a USB id such as `" (1532:0531)"`; compare without it.
pub fn same_device(a: &str, b: &str) -> bool {
    // ADAPTER: flow_audio::same_device(a, b)
    strip_usb_id(a) == strip_usb_id(b)
}

fn strip_usb_id(label: &str) -> &str {
    let trimmed = label.trim_end();
    if let Some(open) = trimmed.rfind(" (")
        && trimmed.ends_with(')')
    {
        let inner = &trimmed[open + 2..trimmed.len() - 1];
        let is_hex_pair = inner.len() == 9
            && inner.as_bytes()[4] == b':'
            && inner
                .chars()
                .enumerate()
                .all(|(i, c)| i == 4 || c.is_ascii_hexdigit());
        if is_hex_pair {
            return &trimmed[..open];
        }
    }
    trimmed
}

/// Why the microphone could not be opened. Mirrors `CaptureError` in capture.ts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureError {
    pub reason: CaptureErrorReason,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorState {
    /// The device is being opened.
    Opening,
    /// Audio is flowing; [`LevelMonitor::level`] is live.
    Listening,
    /// Opening failed, or the device went away.
    Failed(CaptureError),
}

struct Shared {
    state: MonitorState,
    level: f32,
}

/// Opens a microphone and keeps its latest level until dropped. Starting never
/// blocks; `wake` is called whenever the state or the level changes, so the
/// window repaints only while there is something new to draw.
pub struct LevelMonitor {
    shared: Arc<Mutex<Shared>>,
}

impl LevelMonitor {
    pub fn start(device: &str, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            state: MonitorState::Opening,
            level: 0.0,
        }));
        // ADAPTER: start flow_audio::LevelMonitor for `device` here. On each level
        // update store it in `shared.level` and call `wake()`; on open set
        // `Listening`, on failure or device loss set `Failed(..)` (device loss is
        // `DeviceMissing`, "Microphone was disconnected"), then call `wake()`.
        let _ = device;
        shared.lock().state = MonitorState::Listening;
        wake();
        Self { shared }
    }

    pub fn state(&self) -> MonitorState {
        self.shared.lock().state.clone()
    }

    /// The latest RMS level, 0 to 1.
    pub fn level(&self) -> f32 {
        self.shared.lock().level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_labels_without_usb_ids() {
        assert!(same_device(
            "Microphone (Razer Seiren Mini) (1532:0531)",
            "Microphone (Razer Seiren Mini)"
        ));
        assert!(same_device("Headset", "Headset"));
        assert!(!same_device("Headset", "Speakers"));
        assert_eq!(strip_usb_id("Mic (USB Audio)"), "Mic (USB Audio)");
    }
}
