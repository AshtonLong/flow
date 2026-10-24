//! The capture half of `src/renderer/overlay/main.ts`: the session asks for a
//! recording at key-down and stops it at key-up. Opening a microphone takes a
//! moment, so a start can resolve after the user already let go; generations
//! tell such a late open that it is stale. With `keep_warm` the microphone stays
//! open (paused) after a recording so the next one starts at once, and a device
//! change drops the warm stream.
//!
//! Commands run in order on one controller thread; opening happens on a second
//! thread so a stop never waits for an open.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};
use flow_core::types::{CaptureErrorReason, CaptureStart};
use parking_lot::Mutex;

use crate::analyser::Analyser;
use crate::devices::DeviceWatcher;
use crate::error::CaptureError;
use crate::mic::{Mic, MicEvent, MicSink};

/// What the capture tells the session. Events arrive in order: `Started`, then
/// chunks, then `Stopped` after the last chunk (or `Error` instead).
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureEvent {
    /// Audio is flowing: the pill may now appear, which means "speak now".
    Started,
    /// 1600 samples of 16 kHz mono (fewer for the last chunk).
    Chunk(Vec<f32>),
    /// The recording is over and every chunk has been delivered.
    Stopped,
    /// The microphone could not be opened, or was lost while recording.
    Error { reason: CaptureErrorReason, message: String },
}

type Sink = Arc<dyn Fn(CaptureEvent) + Send + Sync>;

enum Cmd {
    Start(CaptureStart),
    Stop,
    DeviceChanged,
    Opened { generation: u64, device: String, keep_warm: bool, result: Result<Mic, CaptureError> },
    Ended(u64),
    Shutdown,
}

struct OpenRequest {
    generation: u64,
    device: String,
    keep_warm: bool,
}

/// State the audio thread reads when it forwards audio.
struct Flags {
    recording: AtomicBool,
    announced: AtomicBool,
    sink: Sink,
}

/// Microphone capture for dictation. Cheap to call from any thread: every
/// method only queues a command.
pub struct DictationCapture {
    tx: Sender<Cmd>,
    analyser: Arc<Analyser>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DictationCapture {
    pub fn new(sink: Arc<dyn Fn(CaptureEvent) + Send + Sync>) -> Self {
        let (tx, rx) = unbounded();
        let analyser = Arc::new(Analyser::new());
        let controller_tx = tx.clone();
        let controller_analyser = analyser.clone();
        let thread = std::thread::Builder::new()
            .name("flow-capture".into())
            .spawn(move || Controller::run(rx, controller_tx, controller_analyser, sink))
            .map_err(|err| log::error!(target: "capture", "could not start the capture thread: {err}"))
            .ok();
        Self { tx, analyser, thread: Mutex::new(thread) }
    }

    /// Opens (or resumes) the microphone. `Started` follows once audio flows.
    pub fn start(&self, opts: &CaptureStart) {
        let _ = self.tx.send(Cmd::Start(opts.clone()));
    }

    /// Flushes the last partial chunk, then sends `Stopped`. The microphone is
    /// released unless the start asked to keep it warm.
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    /// The waveform source for the overlay. The same analyser serves every
    /// recording.
    pub fn analyser(&self) -> Arc<Analyser> {
        self.analyser.clone()
    }

    /// A device was added, removed or changed: a warm microphone that is not
    /// recording is released. Called automatically from Windows notifications.
    pub fn device_changed(&self) {
        let _ = self.tx.send(Cmd::DeviceChanged);
    }
}

impl Drop for DictationCapture {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(thread) = self.thread.lock().take() {
            let _ = thread.join();
        }
    }
}

struct Controller {
    opener: Option<Sender<OpenRequest>>,
    flags: Arc<Flags>,
    handle: Option<Mic>,
    /// Device the open handle belongs to, so a warm handle is not reused for another mic.
    handle_device: String,
    /// Bumped on every start and stop; a start that resolves late can tell it is stale.
    generation: u64,
    keep_warm: bool,
}

impl Controller {
    fn run(rx: Receiver<Cmd>, tx: Sender<Cmd>, analyser: Arc<Analyser>, sink: Sink) {
        crate::com::ensure();
        let (open_tx, open_rx) = unbounded::<OpenRequest>();
        let opener_tx = tx.clone();
        let opener = std::thread::Builder::new()
            .name("flow-capture-open".into())
            .spawn(move || open_loop(open_rx, opener_tx, analyser))
            .map_err(|err| log::error!(target: "capture", "could not start the opener thread: {err}"))
            .ok();
        // A warm microphone must not outlive a device change (unplug, new default).
        let change_tx = tx;
        let watcher = DeviceWatcher::new(Arc::new(move || {
            let _ = change_tx.send(Cmd::DeviceChanged);
        }));

        let mut this = Controller {
            opener: Some(open_tx),
            flags: Arc::new(Flags { recording: AtomicBool::new(false), announced: AtomicBool::new(false), sink }),
            handle: None,
            handle_device: String::new(),
            generation: 0,
            keep_warm: false,
        };
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Start(opts) => this.begin(opts),
                Cmd::Stop => this.end(),
                Cmd::DeviceChanged => {
                    if !this.recording() {
                        this.drop_handle();
                    }
                }
                Cmd::Opened { generation, device, keep_warm, result } => {
                    this.opened(generation, device, keep_warm, result)
                }
                Cmd::Ended(id) => this.ended(id),
                Cmd::Shutdown => break,
            }
        }
        drop(watcher);
        this.drop_handle();
        this.opener = None;
        if let Some(opener) = opener {
            let _ = opener.join();
        }
    }

    fn recording(&self) -> bool {
        self.flags.recording.load(Ordering::SeqCst)
    }

    fn set_recording(&self, on: bool) {
        self.flags.recording.store(on, Ordering::SeqCst);
    }

    fn emit(&self, event: CaptureEvent) {
        (self.flags.sink)(event);
    }

    fn fail(&self, err: CaptureError) {
        log::warn!(target: "capture", "capture failed ({:?}): {}", err.reason, err.message);
        self.emit(CaptureEvent::Error { reason: err.reason, message: err.message });
    }

    /// Forwards a mic's audio while recording; announces the first audio.
    fn mic_sink(&self) -> MicSink {
        let flags = self.flags.clone();
        Arc::new(move |event| match event {
            MicEvent::Flowing => {
                if flags.recording.load(Ordering::SeqCst) && !flags.announced.swap(true, Ordering::SeqCst) {
                    (flags.sink)(CaptureEvent::Started);
                }
            }
            MicEvent::Chunk(pcm) => {
                if !flags.recording.load(Ordering::SeqCst) {
                    return;
                }
                if !flags.announced.swap(true, Ordering::SeqCst) {
                    (flags.sink)(CaptureEvent::Started);
                }
                (flags.sink)(CaptureEvent::Chunk(pcm));
            }
        })
    }

    fn drop_handle(&mut self) {
        self.handle = None;
        self.handle_device.clear();
    }

    /// beginCapture
    fn begin(&mut self, opts: CaptureStart) {
        self.keep_warm = opts.keep_warm;
        self.generation += 1;
        let generation = self.generation;
        self.set_recording(true);
        self.flags.announced.store(false, Ordering::SeqCst);
        if let Some(handle) = &self.handle
            && !handle.ended()
            && self.handle_device == opts.device_id
        {
            match handle.resume(Some(self.mic_sink()), None, true) {
                Ok(()) => {
                    log::debug!(target: "capture", "resumed the warm microphone");
                    return;
                }
                Err(err) => log::warn!(target: "capture", "warm microphone failed to resume: {}", err.message),
            }
        }
        self.drop_handle();
        let request = OpenRequest { generation, device: opts.device_id, keep_warm: opts.keep_warm };
        if self.opener.as_ref().is_none_or(|o| o.send(request).is_err()) {
            self.set_recording(false);
            self.fail(CaptureError::new(CaptureErrorReason::Unknown, "Microphone error"));
        }
    }

    fn opened(&mut self, generation: u64, device: String, keep_warm: bool, result: Result<Mic, CaptureError>) {
        let mic = match result {
            Ok(mic) => mic,
            Err(err) => {
                if generation == self.generation {
                    self.set_recording(false);
                    self.fail(err);
                }
                return;
            }
        };
        if generation != self.generation {
            // Stopped (or restarted) before the microphone opened.
            if keep_warm && self.handle.is_none() && !mic.ended() {
                self.handle = Some(mic);
                self.handle_device = device;
            }
            return;
        }
        self.drop_handle();
        if mic.ended() {
            self.set_recording(false);
            self.fail(CaptureError::new(CaptureErrorReason::DeviceMissing, "Microphone disconnected"));
            return;
        }
        match mic.resume(Some(self.mic_sink()), None, true) {
            Ok(()) => {
                self.handle = Some(mic);
                self.handle_device = device;
            }
            Err(err) => {
                self.set_recording(false);
                self.fail(err);
            }
        }
    }

    /// endCapture
    fn end(&mut self) {
        self.generation += 1;
        if !self.recording() {
            self.emit(CaptureEvent::Stopped);
            return;
        }
        let keep = match &self.handle {
            Some(handle) => {
                // Chunks keep flowing during the flush, so recording stays on until it completes.
                handle.flush();
                let keep = self.keep_warm && !handle.ended();
                if keep {
                    handle.pause();
                }
                keep
            }
            None => true,
        };
        if !keep {
            self.drop_handle();
        }
        self.set_recording(false);
        self.emit(CaptureEvent::Stopped);
    }

    /// The handle's onEnded: the device went away.
    fn ended(&mut self, id: u64) {
        if self.handle.as_ref().map(Mic::id) != Some(id) {
            // Already released; a released handle has nothing to report.
            return;
        }
        self.drop_handle();
        if self.recording() {
            self.set_recording(false);
            self.fail(CaptureError::new(CaptureErrorReason::DeviceMissing, "Microphone disconnected"));
        }
    }
}

/// Opens microphones off the controller thread and reports back.
fn open_loop(rx: Receiver<OpenRequest>, tx: Sender<Cmd>, analyser: Arc<Analyser>) {
    crate::com::ensure();
    for request in rx {
        let ended_tx = tx.clone();
        let started = std::time::Instant::now();
        let result = Mic::open(
            &request.device,
            analyser.clone(),
            Box::new(move |id| {
                let _ = ended_tx.send(Cmd::Ended(id));
            }),
        );
        log::debug!(target: "capture", "open took {:?}", started.elapsed());
        let opened = Cmd::Opened {
            generation: request.generation,
            device: request.device,
            keep_warm: request.keep_warm,
            result,
        };
        if tx.send(opened).is_err() {
            break;
        }
    }
}
