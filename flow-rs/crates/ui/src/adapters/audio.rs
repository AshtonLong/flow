//! The settings window's view of the microphone: the device list, a level
//! monitor for the meter and the test bench's clip recorder. Stands in for
//! `listMicrophones` / `startCapture` / `recordClip` in
//! `src/renderer/common/capture.ts`, over `flow_audio`.

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
    flow_audio::list_microphones().into_iter().map(|m| Microphone { label: m.label, id: m.id }).collect()
}

/// Electron labels carry a USB id such as `" (1532:0531)"`; compare without it.
pub fn same_device(a: &str, b: &str) -> bool {
    flow_audio::same_device(a, b)
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
    /// The open device. The opening thread also holds this until it is done, so
    /// a monitor dropped while opening releases the device as soon as it opens.
    device: Arc<Mutex<Option<flow_audio::LevelMonitor>>>,
}

impl LevelMonitor {
    pub fn start(device: &str, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Mutex::new(Shared { state: MonitorState::Opening, level: 0.0 }));
        let slot = Arc::new(Mutex::new(None));
        let label = device.to_string();
        let (thread_shared, thread_slot) = (shared.clone(), slot.clone());
        let spawned = std::thread::Builder::new().name("flow-mic-meter".into()).spawn(move || {
            let shared = thread_shared;
            let on_level: Arc<dyn Fn(f32) + Send + Sync> = {
                let (shared, wake) = (shared.clone(), wake.clone());
                Arc::new(move |level| {
                    shared.lock().level = level;
                    wake();
                })
            };
            let on_ended: Arc<dyn Fn() + Send + Sync> = {
                let (shared, wake) = (shared.clone(), wake.clone());
                Arc::new(move || {
                    let mut shared = shared.lock();
                    shared.level = 0.0;
                    shared.state = MonitorState::Failed(CaptureError {
                        reason: CaptureErrorReason::DeviceMissing,
                        message: "Microphone was disconnected".into(),
                    });
                    drop(shared);
                    wake();
                })
            };
            match flow_audio::LevelMonitor::open(&label, on_level, on_ended) {
                Ok(monitor) => {
                    {
                        let mut shared = shared.lock();
                        if shared.state == MonitorState::Opening {
                            shared.state = MonitorState::Listening;
                        }
                    }
                    *thread_slot.lock() = Some(monitor);
                }
                Err(err) => {
                    shared.lock().state =
                        MonitorState::Failed(CaptureError { reason: err.reason, message: err.message });
                }
            }
            wake();
        });
        if let Err(err) = spawned {
            shared.lock().state = MonitorState::Failed(CaptureError {
                reason: CaptureErrorReason::Unknown,
                message: format!("Could not open the microphone: {err}"),
            });
        }
        Self { shared, device: slot }
    }

    pub fn state(&self) -> MonitorState {
        self.shared.lock().state.clone()
    }

    /// The latest RMS level, 0 to 1.
    pub fn level(&self) -> f32 {
        self.shared.lock().level
    }
}

impl Drop for LevelMonitor {
    fn drop(&mut self) {
        self.device.lock().take();
    }
}

/// Records one clip for the test bench (`recordClip` in capture.ts). Starting
/// never blocks: the microphone opens on its own thread and [`Self::state`]
/// turns `Listening` once audio is being recorded. `wake` is called whenever
/// the state or the level changes. Dropping the recorder discards the clip
/// and releases the microphone (`cancel()`).
pub struct ClipRecorder {
    shared: Arc<Mutex<Shared>>,
    /// The open recorder; filled by the opening thread, like [`LevelMonitor`]'s.
    device: Arc<Mutex<Option<flow_audio::ClipRecorder>>>,
}

impl ClipRecorder {
    pub fn start(device: &str, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Mutex::new(Shared { state: MonitorState::Opening, level: 0.0 }));
        let slot = Arc::new(Mutex::new(None));
        let label = device.to_string();
        let (thread_shared, thread_slot) = (shared.clone(), slot.clone());
        let spawned = std::thread::Builder::new().name("flow-clip-recorder".into()).spawn(move || {
            let shared = thread_shared;
            let on_level: Arc<dyn Fn(f32) + Send + Sync> = {
                let (shared, wake) = (shared.clone(), wake.clone());
                Arc::new(move |level| {
                    shared.lock().level = level;
                    wake();
                })
            };
            match flow_audio::ClipRecorder::start(&label, Some(on_level), None) {
                Ok(recorder) => {
                    shared.lock().state = MonitorState::Listening;
                    *thread_slot.lock() = Some(recorder);
                }
                Err(err) => {
                    shared.lock().state =
                        MonitorState::Failed(CaptureError { reason: err.reason, message: err.message });
                }
            }
            wake();
        });
        if let Err(err) = spawned {
            shared.lock().state = MonitorState::Failed(CaptureError {
                reason: CaptureErrorReason::Unknown,
                message: format!("Could not open the microphone: {err}"),
            });
        }
        Self { shared, device: slot }
    }

    /// `Opening`, then `Listening` while recording, or `Failed` if the microphone could not be opened.
    pub fn state(&self) -> MonitorState {
        self.shared.lock().state.clone()
    }

    /// The latest level, 0 to 1.
    pub fn level(&self) -> f32 {
        self.shared.lock().level
    }

    /// Stops recording and returns the clip: 16 kHz mono. Empty if the
    /// microphone never opened.
    pub fn stop(self) -> Vec<f32> {
        let recorder = self.device.lock().take();
        recorder.map(flow_audio::ClipRecorder::stop).unwrap_or_default()
    }
}

impl Drop for ClipRecorder {
    fn drop(&mut self) {
        if let Some(recorder) = self.device.lock().take() {
            recorder.cancel();
        }
    }
}
