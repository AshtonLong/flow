//! One open microphone: the port of `startCapture` and its `CaptureHandle`
//! (capture.ts) plus the AudioWorklet that batched audio into chunks
//! (capture-worklet.js).
//!
//! The stream is opened paused. [`Mic::resume`] starts it and attaches a sink;
//! [`Mic::flush`] ends a recording: the audio thread pushes the samples still in
//! the filters out, emits the last partial chunk and goes quiet. A mic kept warm
//! is paused rather than closed, so the next start skips opening the device.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use flow_core::types::CaptureErrorReason;
use parking_lot::{Condvar, Mutex};

use crate::analyser::Analyser;
use crate::devices;
use crate::dsp::Pipeline;
use crate::error::{self, CaptureError};
use crate::{CHUNK_SAMPLES, com};

/// How long a stop waits for the audio thread to flush (capture.ts: 250 ms).
pub(crate) const FLUSH_TIMEOUT: Duration = Duration::from_millis(250);
/// Level reports, about 60 a second (one animation frame).
const LEVEL_INTERVAL: Duration = Duration::from_micros(16_667);

pub(crate) enum MicEvent {
    /// The first audio after a resume has arrived.
    Flowing,
    /// 1600 samples, or fewer for the last chunk of a recording.
    Chunk(Vec<f32>),
}

pub(crate) type MicSink = Arc<dyn Fn(MicEvent) + Send + Sync>;
pub(crate) type LevelSink = Arc<dyn Fn(f32) + Send + Sync>;

/// Gain the AGC settled on per microphone, so the next recording starts there
/// instead of at unity.
static GAINS: LazyLock<Mutex<HashMap<String, f32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct State {
    pipeline: Pipeline,
    converted: Vec<f32>,
    out: Vec<f32>,
    chunk: Vec<f32>,
    /// The worklet's `active`: audio is taken in only between resume and flush.
    active: bool,
    keep_chunks: bool,
    sink: Option<MicSink>,
    level: Option<LevelSink>,
    level_due: Instant,
    flowing_sent: bool,
    flush_requested: bool,
}

struct Shared {
    state: Mutex<State>,
    flushed: Condvar,
    analyser: Arc<Analyser>,
    id: u64,
    ended: AtomicBool,
    on_ended: Box<dyn Fn(u64) + Send + Sync>,
    /// Opened as the system default, which follows Windows to a new default.
    follows_default: bool,
    gain_key: String,
}

pub(crate) struct Mic {
    stream: Option<cpal::Stream>,
    shared: Arc<Shared>,
    id: u64,
}

impl Mic {
    /// Opens `device` (a label or `"default"`), paused. `on_ended` runs once, on
    /// an audio thread, with the mic's id if the device goes away (onEnded in
    /// capture.ts).
    pub(crate) fn open(
        device: &str,
        analyser: Arc<Analyser>,
        on_ended: Box<dyn Fn(u64) + Send + Sync>,
    ) -> Result<Self, CaptureError> {
        com::ensure();
        let (dev, follows_default) = devices::resolve(device)?;
        let config = dev.default_input_config().map_err(|e| error::from_cpal(&e))?;
        let format = config.sample_format();
        if !supported(format) {
            return Err(CaptureError::new(
                CaptureErrorReason::Unknown,
                format!("Unsupported microphone format: {format}"),
            ));
        }
        let stream_config = config.config();
        let gain_key = devices::device_key(device);
        let gain = GAINS.lock().get(&gain_key).copied().unwrap_or(0.0);
        let pipeline = Pipeline::new(stream_config.sample_rate, stream_config.channels as usize, gain)
            .map_err(|e| CaptureError::new(CaptureErrorReason::Unknown, e.to_string()))?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                pipeline,
                converted: Vec::new(),
                out: Vec::new(),
                chunk: Vec::with_capacity(CHUNK_SAMPLES),
                active: false,
                keep_chunks: true,
                sink: None,
                level: None,
                level_due: Instant::now(),
                flowing_sent: false,
                flush_requested: false,
            }),
            flushed: Condvar::new(),
            analyser,
            id,
            ended: AtomicBool::new(false),
            on_ended,
            follows_default,
            gain_key,
        });
        let data_shared = shared.clone();
        let error_shared = shared.clone();
        let stream = dev
            .build_input_stream_raw(
                stream_config,
                format,
                move |data, _info| on_data(&data_shared, data),
                move |err| on_error(&error_shared, err),
                None,
            )
            .map_err(|e| error::from_cpal(&e))?;
        log::info!(
            target: "capture",
            "microphone opened: {} Hz, {} ch, {format}{}",
            stream_config.sample_rate,
            stream_config.channels,
            if follows_default { " (system default)" } else { "" }
        );
        Ok(Self { stream: Some(stream), shared, id })
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// True once the device has gone away.
    pub(crate) fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Acquire)
    }

    pub(crate) fn analyser(&self) -> Arc<Analyser> {
        self.shared.analyser.clone()
    }

    /// Starts (or restarts) taking audio. Chunks go to `sink` when `keep_chunks`.
    pub(crate) fn resume(
        &self,
        sink: Option<MicSink>,
        level: Option<LevelSink>,
        keep_chunks: bool,
    ) -> Result<(), CaptureError> {
        {
            let mut st = self.shared.state.lock();
            if let Err(err) = st.pipeline.reset() {
                log::warn!(target: "capture", "pipeline reset failed: {err}");
            }
            st.chunk.clear();
            st.active = true;
            st.keep_chunks = keep_chunks;
            st.sink = sink;
            st.level = level;
            st.level_due = Instant::now();
            st.flowing_sent = false;
            st.flush_requested = false;
        }
        // A fresh waveform, as a new AnalyserNode would give.
        self.shared.analyser.reset();
        if let Some(stream) = &self.stream {
            stream.play().map_err(|e| error::from_cpal(&e))?;
        }
        Ok(())
    }

    /// Ends a recording: the audio thread drains the filters and emits the last
    /// partial chunk. Waits at most [`FLUSH_TIMEOUT`]; if the audio thread does not
    /// answer (a stalled device), the flush happens here instead.
    pub(crate) fn flush(&self) {
        let mut st = self.shared.state.lock();
        if st.active && self.ended() {
            // No audio thread left to ask.
            flush_locked(&self.shared, &mut st);
        } else if st.active {
            st.flush_requested = true;
            let deadline = Instant::now() + FLUSH_TIMEOUT;
            while st.flush_requested {
                if self.shared.flushed.wait_until(&mut st, deadline).timed_out() {
                    break;
                }
            }
            if st.flush_requested {
                log::warn!(target: "capture", "flush timed out; finishing without the audio thread");
                flush_locked(&self.shared, &mut st);
            }
        }
        st.sink = None;
        st.level = None;
    }

    /// Stops the device without closing it (keep-warm).
    pub(crate) fn pause(&self) {
        if let Some(stream) = &self.stream
            && let Err(err) = stream.pause()
        {
            log::warn!(target: "capture", "could not pause the microphone: {err}");
        }
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        // Joins the audio thread; afterwards no callback can run.
        self.stream.take();
    }
}

fn supported(format: SampleFormat) -> bool {
    use SampleFormat as F;
    matches!(format, F::F32 | F::F64 | F::I16 | F::I24 | F::I32 | F::U8 | F::I8 | F::U16)
}

fn extend<T>(data: &cpal::Data, out: &mut Vec<f32>)
where
    T: SizedSample,
    f32: FromSample<T>,
{
    if let Some(samples) = data.as_slice::<T>() {
        out.extend(samples.iter().map(|&s| f32::from_sample_(s)));
    }
}

fn convert(data: &cpal::Data, out: &mut Vec<f32>) {
    use SampleFormat as F;
    match data.sample_format() {
        F::F32 => {
            if let Some(samples) = data.as_slice::<f32>() {
                out.extend_from_slice(samples);
            }
        }
        F::F64 => extend::<f64>(data, out),
        F::I16 => extend::<i16>(data, out),
        F::I24 => extend::<cpal::I24>(data, out),
        F::I32 => extend::<i32>(data, out),
        F::U8 => extend::<u8>(data, out),
        F::I8 => extend::<i8>(data, out),
        F::U16 => extend::<u16>(data, out),
        _ => {}
    }
}

/// The audio thread's callback: process, feed the analyser, batch into chunks.
fn on_data(shared: &Shared, data: &cpal::Data) {
    let mut guard = shared.state.lock();
    let st = &mut *guard;
    if !st.active {
        return;
    }
    st.converted.clear();
    convert(data, &mut st.converted);
    if st.converted.is_empty() {
        return;
    }
    if !st.flowing_sent {
        st.flowing_sent = true;
        if let Some(sink) = &st.sink {
            sink(MicEvent::Flowing);
        }
    }
    st.out.clear();
    st.pipeline.process(&st.converted, &mut st.out);
    deliver(shared, st);
    if st.flush_requested {
        flush_locked(shared, st);
    }
}

/// Hands `st.out` to the analyser, the level sink and the chunker.
fn deliver(shared: &Shared, st: &mut State) {
    if !st.out.is_empty() {
        shared.analyser.push(&st.out);
        if st.keep_chunks {
            let mut rest = &st.out[..];
            while !rest.is_empty() {
                let n = rest.len().min(CHUNK_SAMPLES - st.chunk.len());
                st.chunk.extend_from_slice(&rest[..n]);
                rest = &rest[n..];
                if st.chunk.len() == CHUNK_SAMPLES {
                    let full = std::mem::replace(&mut st.chunk, Vec::with_capacity(CHUNK_SAMPLES));
                    if let Some(sink) = &st.sink {
                        sink(MicEvent::Chunk(full));
                    }
                }
            }
        }
    }
    if let Some(level) = &st.level {
        let now = Instant::now();
        if now >= st.level_due {
            st.level_due += LEVEL_INTERVAL;
            if st.level_due + LEVEL_INTERVAL < now {
                st.level_due = now + LEVEL_INTERVAL;
            }
            level(shared.analyser.level());
        }
    }
}

/// The worklet's `flush`: stop taking audio and emit what is buffered.
fn flush_locked(shared: &Shared, st: &mut State) {
    st.out.clear();
    st.pipeline.drain(&mut st.out);
    deliver(shared, st);
    if st.keep_chunks && !st.chunk.is_empty() {
        let last = std::mem::replace(&mut st.chunk, Vec::with_capacity(CHUNK_SAMPLES));
        if let Some(sink) = &st.sink {
            sink(MicEvent::Chunk(last));
        }
    }
    st.active = false;
    st.flush_requested = false;
    GAINS.lock().insert(shared.gain_key.clone(), st.pipeline.agc().gain_db());
    shared.flushed.notify_all();
}

fn on_error(shared: &Shared, err: cpal::Error) {
    use cpal::ErrorKind as K;
    match err.kind() {
        K::Xrun | K::RealtimeDenied => {
            log::debug!(target: "capture", "microphone: {err}");
            return;
        }
        // The system default changed and Windows moved the stream to the new one.
        K::DeviceChanged | K::StreamInvalidated if shared.follows_default => {
            log::info!(target: "capture", "default microphone changed; following it");
            return;
        }
        _ => {}
    }
    log::warn!(target: "capture", "microphone stream ended: {err}");
    if !shared.ended.swap(true, Ordering::AcqRel) {
        (shared.on_ended)(shared.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::traits::HostTrait;

    #[test]
    #[ignore = "opens the real default microphone"]
    fn time_each_step_of_a_cold_open() {
        for round in 0..3 {
            let t0 = Instant::now();
            com::ensure();
            let (dev, _) = devices::resolve("default").unwrap();
            let t1 = Instant::now();
            let config = dev.default_input_config().unwrap();
            let t2 = Instant::now();
            let pipeline = Pipeline::new(config.sample_rate(), config.channels() as usize, 0.0).unwrap();
            let t3 = Instant::now();
            let (tx, rx) = std::sync::mpsc::channel();
            let stream = dev
                .build_input_stream_raw(
                    config.config(),
                    config.sample_format(),
                    move |_data, _| {
                        let _ = tx.send(Instant::now());
                    },
                    |_| {},
                    None,
                )
                .unwrap();
            let t4 = Instant::now();
            stream.play().unwrap();
            let t5 = Instant::now();
            let first = rx.recv().unwrap();
            drop(stream);
            drop(pipeline);
            eprintln!(
                "round {round}: resolve {:?}, config {:?}, pipeline {:?}, build {:?}, play {:?}, first data {:?} | total {:?}",
                t1 - t0,
                t2 - t1,
                t3 - t2,
                t4 - t3,
                t5 - t4,
                first - t5,
                first - t0
            );
        }
        // The current default endpoint opened directly, without the routing proxy.
        let host = cpal::default_host();
        let mics = devices::list_microphones();
        let t0 = Instant::now();
        let dev = host.input_devices().unwrap().find(|d| d.id().is_ok_and(|id| mics.iter().any(|m| m.id == id.id())));
        eprintln!("find by id {:?}", t0.elapsed());
        if let Some(dev) = dev {
            let t0 = Instant::now();
            let config = dev.default_input_config().unwrap();
            let t1 = Instant::now();
            let stream = dev.build_input_stream_raw(config.config(), config.sample_format(), |_, _| {}, |_| {}, None);
            eprintln!("specific: config {:?}, build {:?}", t1 - t0, t1.elapsed());
            drop(stream);
        }
    }
}
