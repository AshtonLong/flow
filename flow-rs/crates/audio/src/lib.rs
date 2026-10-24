//! Microphone capture shared by the dictation session (the overlay's half of
//! `src/renderer/overlay/main.ts`) and the settings window (level meter, test
//! bench). Port of `src/renderer/common/capture.ts` and `capture-worklet.js`.
//!
//! Chromium gave the Electron app 16 kHz mono with noise suppression and
//! automatic gain already applied; here the microphone is opened natively
//! (WASAPI through cpal) and [`dsp::Pipeline`] does the same work: noise
//! suppression (RNNoise at 48 kHz), a gentle automatic gain, then a resample to
//! 16 kHz. Audio leaves in 1600-sample (100 ms) chunks, like the AudioWorklet.

mod analyser;
mod com;
mod devices;
mod dictation;
pub mod dsp;
mod error;
mod mic;
mod monitor;

pub use analyser::{Analyser, FFT_SIZE, FREQUENCY_BIN_COUNT};
pub use devices::{DeviceWatcher, Microphone, list_microphones, same_device};
pub use dictation::{CaptureEvent, DictationCapture};
pub use error::CaptureError;
pub use monitor::{ClipRecorder, LevelMonitor};

/// Sample rate of everything that leaves this crate.
pub const CAPTURE_SAMPLE_RATE: u32 = flow_core::types::SAMPLE_RATE;

/// Samples per chunk: 100 ms at 16 kHz, the AudioWorklet's batch size.
pub const CHUNK_SAMPLES: usize = 1600;
