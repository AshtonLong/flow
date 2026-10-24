//! The settings window's microphone uses: the live input-level meter (`MicMeter`
//! in LevelMeter.tsx, which kept no audio) and the test bench's `recordClip`.
//! Both open the microphone with the same processing as dictation, so the meter
//! shows what a model would hear.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::analyser::Analyser;
use crate::error::CaptureError;
use crate::mic::{Mic, MicEvent, MicSink};

/// Reports the level of an open microphone until dropped.
pub struct LevelMonitor {
    mic: Mic,
}

impl LevelMonitor {
    /// Opens `device` (a label or `"default"`) and calls `on_level` about 60
    /// times a second with the RMS level, 0..1. Blocks while the device opens.
    /// `on_ended` runs once if the device goes away. The microphone is released
    /// when the monitor is dropped.
    pub fn open(
        device: &str,
        on_level: Arc<dyn Fn(f32) + Send + Sync>,
        on_ended: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, CaptureError> {
        let mic = Mic::open(device, Arc::new(Analyser::new()), Box::new(move |_| on_ended()))?;
        mic.resume(None, Some(on_level), false)?;
        Ok(Self { mic })
    }

    /// The live analyser, for drawing a waveform.
    pub fn analyser(&self) -> Arc<Analyser> {
        self.mic.analyser()
    }

    /// True once the device has gone away.
    pub fn ended(&self) -> bool {
        self.mic.ended()
    }
}

/// Records until stopped and returns the whole clip (`recordClip`).
pub struct ClipRecorder {
    mic: Mic,
    chunks: Arc<Mutex<Vec<f32>>>,
}

impl ClipRecorder {
    /// Opens `device` and starts recording. Blocks while the device opens.
    pub fn start(
        device: &str,
        on_level: Option<Arc<dyn Fn(f32) + Send + Sync>>,
        on_ended: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<Self, CaptureError> {
        let mic = Mic::open(
            device,
            Arc::new(Analyser::new()),
            Box::new(move |_| {
                if let Some(f) = &on_ended {
                    f();
                }
            }),
        )?;
        let chunks = Arc::new(Mutex::new(Vec::new()));
        let target = chunks.clone();
        let sink: MicSink = Arc::new(move |event| {
            if let MicEvent::Chunk(pcm) = event {
                target.lock().extend_from_slice(&pcm);
            }
        });
        mic.resume(Some(sink), on_level, true)?;
        Ok(Self { mic, chunks })
    }

    pub fn analyser(&self) -> Arc<Analyser> {
        self.mic.analyser()
    }

    /// True once the device has gone away.
    pub fn ended(&self) -> bool {
        self.mic.ended()
    }

    /// Samples recorded so far.
    pub fn len(&self) -> usize {
        self.chunks.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Flushes the last partial chunk, releases the microphone and returns the
    /// clip: 16 kHz mono.
    pub fn stop(self) -> Vec<f32> {
        self.mic.flush();
        let Self { mic, chunks } = self;
        drop(mic);
        std::mem::take(&mut *chunks.lock())
    }

    /// Releases the microphone and discards the clip.
    pub fn cancel(self) {
        drop(self);
    }
}
