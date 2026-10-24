//! The engine: a child process (`Flow.exe --engine`) that owns voice detection
//! and local models. It is started on the first dictation and exits when the
//! main process decides the model has been idle long enough, which returns all
//! its memory. Port of `src/engine/index.ts`.
//!
//! Requests arrive as frames on stdin and responses leave as frames on stdout
//! (`flow_core::protocol::engine`); diagnostics go to stderr only. Model work
//! (probe, load, transcribe, unload) is serialised on one worker thread, since
//! the native library allows one compute per model; voice detection does not
//! touch the model, so `trim` runs alongside it on its own thread. `cancel`
//! aborts its target through the library's abort callback.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use flow_core::protocol::engine::{
    EngineDevice, EngineLoadResult, EngineModelRef, EngineProbeResult, EngineRequest, EngineResponse, EngineResult,
    EngineTranscribeResult, EngineTrimResult, MAX_HEADER_BYTES, MAX_PCM_SAMPLES, VadKind, env, write_frame,
};
use flow_core::types::{BackendDevice, samples_to_ms};
use flow_core::vad::{DEFAULT_SEGMENT_OPTIONS, energy_probs, extract_segments, segments_from_probs};
use parking_lot::Mutex;

use crate::silero::SileroVad;
use crate::tcpp::{self, BackendInfo, DeviceType, Feature, TcError, TranscribeModel, TranscribeOptions, TranscriptionResult};

/// Stack for the model worker. ggml's Vulkan model initialisation needs more
/// than small default stacks (the bindings raise koffi's async stack to 2 MiB).
const WORKER_STACK: usize = 8 * 1024 * 1024;

/// The speech library as the engine uses it; the native implementation is
/// [`NativeSpeech`], tests substitute a fake.
pub(crate) trait Speech: Send + 'static {
    type Model: SpeechModel;
    fn version(&self) -> Result<String, TcError>;
    fn devices(&self) -> Result<Vec<BackendInfo>, TcError>;
    fn load(&self, path: &str, device: &BackendInfo) -> Result<Self::Model, TcError>;
}

pub(crate) trait SpeechModel {
    fn backend(&self) -> String;
    fn supports(&self, feature: Feature) -> bool;
    fn transcribe(&self, pcm: &[f32], opts: &TranscribeOptions<'_>, abort: &AtomicBool) -> Result<TranscriptionResult, TcError>;
}

/// transcribe.cpp through `transcribe.dll`.
pub(crate) struct NativeSpeech;

impl Speech for NativeSpeech {
    type Model = TranscribeModel;

    fn version(&self) -> Result<String, TcError> {
        Ok(tcpp::version()?.version)
    }

    fn devices(&self) -> Result<Vec<BackendInfo>, TcError> {
        tcpp::get_available_backends()
    }

    fn load(&self, path: &str, device: &BackendInfo) -> Result<TranscribeModel, TcError> {
        TranscribeModel::load(path, Some(device))
    }
}

impl SpeechModel for TranscribeModel {
    fn backend(&self) -> String {
        TranscribeModel::backend(self)
    }

    fn supports(&self, feature: Feature) -> bool {
        TranscribeModel::supports(self, feature)
    }

    fn transcribe(&self, pcm: &[f32], opts: &TranscribeOptions<'_>, abort: &AtomicBool) -> Result<TranscriptionResult, TcError> {
        TranscribeModel::transcribe(self, pcm, opts, Some(abort))
    }
}

/// `Flow.exe --engine`: serves requests on stdin/stdout until stdin closes.
/// Returns the process exit code.
pub fn run_engine_process() -> i32 {
    let output = protocol_output();
    // The native library is chatty; route its diagnostics only when debugging.
    tcpp::set_log_to_stderr(std::env::var(env::DEBUG).is_ok_and(|v| v == "1"));
    serve(NativeSpeech, io::stdin().lock(), output)
}

/// Takes stdout for the protocol and points the process's standard output at
/// stderr, so nothing else (a stray `printf` in native code) can corrupt the
/// frame stream.
#[cfg(windows)]
fn protocol_output() -> Box<dyn Write + Send> {
    use std::os::windows::io::FromRawHandle;
    const STD_OUTPUT_HANDLE: u32 = -11_i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12_i32 as u32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
        fn SetStdHandle(which: u32, handle: *mut std::ffi::c_void) -> i32;
    }
    let invalid = |h: *mut std::ffi::c_void| h.is_null() || h as isize == -1;
    // SAFETY: standard handle queries; the stdout handle is owned by the
    // returned File from here on, and the process no longer refers to it.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let err = GetStdHandle(STD_ERROR_HANDLE);
        if !invalid(out) && !invalid(err) && SetStdHandle(STD_OUTPUT_HANDLE, err) != 0 {
            return Box::new(io::BufWriter::new(std::fs::File::from_raw_handle(out)));
        }
    }
    Box::new(io::stdout())
}

#[cfg(not(windows))]
fn protocol_output() -> Box<dyn Write + Send> {
    Box::new(io::stdout())
}

/// Writes responses; shared by the reader, the model worker and the VAD thread.
#[derive(Clone)]
struct Replier(Arc<Mutex<Box<dyn Write + Send>>>);

impl Replier {
    fn send(&self, response: &EngineResponse, pcm: &[f32]) {
        let mut out = self.0.lock();
        // A failed write means the main process is gone; stdin will close too.
        if let Err(err) = write_frame(&mut *out, response, pcm) {
            log_line(&format!("[engine] could not send a response: {err}"));
        }
    }

    fn reply(&self, id: u64, result: Result<(EngineResult, Vec<f32>), TcError>, aborted: bool) {
        match result {
            Ok((result, pcm)) => self.send(&EngineResponse::Ok { id, result }, &pcm),
            Err(err) => self.send(
                &EngineResponse::Err { id, error: err.message.clone(), aborted: err.is_abort() || aborted },
                &[],
            ),
        }
    }
}

fn log_line(line: &str) {
    let _ = writeln!(io::stderr(), "{line}");
}

/// Voice detection: Silero when its model loads, else the energy detector.
#[derive(Default)]
pub(crate) struct Vad {
    tried: bool,
    silero: Option<SileroVad>,
}

impl Vad {
    fn detector(&mut self) -> Option<&mut SileroVad> {
        if !self.tried {
            self.tried = true;
            match SileroVad::new() {
                Ok(vad) => self.silero = Some(vad),
                Err(err) => log_line(&format!("[engine] Silero VAD unavailable, using energy detection: {err}")),
            }
        }
        self.silero.as_mut()
    }

    fn kind(&mut self) -> VadKind {
        if self.detector().is_some() { VadKind::Silero } else { VadKind::Energy }
    }

    /// Drops leading and trailing silence and long pauses. The trimmed audio is
    /// empty when no speech was found.
    pub(crate) fn trim(&mut self, audio: &[f32]) -> (EngineTrimResult, Vec<f32>) {
        let total_ms = samples_to_ms(audio.len());
        let (probs, detector) = match self.detector() {
            Some(silero) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| silero.probabilities(audio))) {
                Ok(probs) => (probs, VadKind::Silero),
                Err(_) => {
                    log_line("[engine] VAD run failed: the Silero model panicked");
                    (energy_probs(audio), VadKind::Energy)
                }
            },
            None => (energy_probs(audio), VadKind::Energy),
        };
        let segments = segments_from_probs(&probs, total_ms, &DEFAULT_SEGMENT_OPTIONS);
        if segments.is_empty() {
            return (EngineTrimResult { has_speech: false, speech_ms: 0.0, detector }, Vec::new());
        }
        let trimmed = extract_segments(audio, &segments);
        (EngineTrimResult { has_speech: true, speech_ms: samples_to_ms(trimmed.len()), detector }, trimmed)
    }
}

enum Work {
    Probe,
    Load(EngineModelRef),
    Transcribe { model: EngineModelRef, language: String, hints: Vec<String>, audio: Vec<f32> },
    Unload,
}

struct Job {
    id: u64,
    work: Work,
    abort: Arc<AtomicBool>,
}

type Inflight = Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>;

/// Serves frames from `input` until it ends; returns the exit code.
pub(crate) fn serve<S: Speech>(speech: S, mut input: impl Read, output: Box<dyn Write + Send>) -> i32 {
    let replier = Replier(Arc::new(Mutex::new(output)));
    let vad = Arc::new(Mutex::new(Vad::default()));
    let inflight: Inflight = Arc::default();
    let stopping = Arc::new(AtomicBool::new(false));

    let (jobs, job_rx) = mpsc::channel::<Job>();
    let worker = {
        let (replier, vad, inflight, stopping) = (replier.clone(), Arc::clone(&vad), Arc::clone(&inflight), Arc::clone(&stopping));
        thread::Builder::new()
            .name("engine-model".into())
            .stack_size(WORKER_STACK)
            .spawn(move || Worker { speech, loaded: None, vad }.run(job_rx, &replier, &inflight, &stopping))
    };
    let (trims, trim_rx) = mpsc::channel::<(u64, Vec<f32>)>();
    let vad_thread = {
        let (replier, vad) = (replier.clone(), Arc::clone(&vad));
        thread::Builder::new().name("engine-vad".into()).spawn(move || {
            for (id, audio) in trim_rx {
                let (result, pcm) = vad.lock().trim(&audio);
                replier.send(&EngineResponse::Ok { id, result: EngineResult::Trim(result) }, &pcm);
            }
        })
    };
    let (worker, vad_thread) = match (worker, vad_thread) {
        (Ok(w), Ok(v)) => (w, v),
        (Err(err), _) | (_, Err(err)) => {
            log_line(&format!("[engine] could not start: {err}"));
            return 1;
        }
    };

    replier.send(&EngineResponse::Hello { hello: true, pid: std::process::id() }, &[]);

    let code = loop {
        let (json, audio) = match read_raw_frame(&mut input) {
            Ok(Some(frame)) => frame,
            Ok(None) => break 0,
            Err(err) => {
                log_line(&format!("[engine] request stream broken: {err}"));
                break 1;
            }
        };
        let request: EngineRequest = match serde_json::from_slice(&json) {
            Ok(request) => request,
            Err(err) => {
                log_line(&format!("[engine] unreadable request: {err}"));
                let id = serde_json::from_slice::<serde_json::Value>(&json).ok().and_then(|v| v.get("id")?.as_u64());
                if let Some(id) = id {
                    replier.send(&EngineResponse::Err { id, error: format!("Unknown request: {err}"), aborted: false }, &[]);
                }
                continue;
            }
        };
        let (id, work) = match request {
            EngineRequest::Cancel { target, .. } => {
                if let Some(flag) = inflight.lock().get(&target) {
                    flag.store(true, Ordering::Relaxed);
                }
                continue;
            }
            EngineRequest::Trim { id } => {
                let _ = trims.send((id, audio));
                continue;
            }
            EngineRequest::Probe { id } => (id, Work::Probe),
            EngineRequest::Load { id, model } => (id, Work::Load(model)),
            EngineRequest::Transcribe { id, model, language, hints } => {
                (id, Work::Transcribe { model, language, hints, audio })
            }
            EngineRequest::Unload { id } => (id, Work::Unload),
        };
        let abort = Arc::new(AtomicBool::new(false));
        inflight.lock().insert(id, Arc::clone(&abort));
        let _ = jobs.send(Job { id, work, abort });
    };

    // The main process is gone (or broke the stream): abort what is running,
    // skip what is queued, and free the model before exiting.
    stopping.store(true, Ordering::Relaxed);
    for flag in inflight.lock().values() {
        flag.store(true, Ordering::Relaxed);
    }
    drop(jobs);
    drop(trims);
    let _ = worker.join();
    let _ = vad_thread.join();
    code
}

/// One frame as raw JSON and PCM; `Ok(None)` at a clean end of stream.
fn read_raw_frame(input: &mut impl Read) -> io::Result<Option<(Vec<u8>, Vec<f32>)>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let json_len = u32::from_le_bytes(len);
    if json_len > MAX_HEADER_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "engine frame header too large"));
    }
    let mut json = vec![0u8; json_len as usize];
    input.read_exact(&mut json)?;
    input.read_exact(&mut len)?;
    let samples = u32::from_le_bytes(len);
    if samples > MAX_PCM_SAMPLES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "engine frame audio too long"));
    }
    let mut raw = vec![0u8; samples as usize * 4];
    input.read_exact(&mut raw)?;
    let pcm = raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    Ok(Some((json, pcm)))
}

struct Worker<S: Speech> {
    speech: S,
    loaded: Option<(EngineModelRef, S::Model)>,
    vad: Arc<Mutex<Vad>>,
}

impl<S: Speech> Worker<S> {
    fn run(mut self, jobs: mpsc::Receiver<Job>, replier: &Replier, inflight: &Inflight, stopping: &AtomicBool) {
        for Job { id, work, abort } in jobs {
            if stopping.load(Ordering::Relaxed) {
                break;
            }
            let result = match work {
                Work::Probe => self.probe().map(|r| (EngineResult::Probe(r), Vec::new())),
                Work::Load(model) => self.load(model).map(|r| (EngineResult::Load(r), Vec::new())),
                Work::Transcribe { model, language, hints, audio } => self
                    .transcribe(model, &language, hints, &audio, &abort)
                    .map(|r| (EngineResult::Transcribe(r), Vec::new())),
                Work::Unload => {
                    self.loaded = None;
                    Ok((EngineResult::Unload, Vec::new()))
                }
            };
            replier.reply(id, result, abort.load(Ordering::Relaxed));
            inflight.lock().remove(&id);
        }
        // Dropping `self` frees the model.
    }

    fn probe(&mut self) -> Result<EngineProbeResult, TcError> {
        let vad = self.vad.lock().kind();
        let version = self.speech.version()?;
        let devices = self.speech.devices()?.iter().map(to_backend_device).collect();
        Ok(EngineProbeResult { version, devices, vad })
    }

    fn load(&mut self, model: EngineModelRef) -> Result<EngineLoadResult, TcError> {
        let load_ms = self.ensure_loaded(&model)?;
        let backend = self.loaded.as_ref().map(|(_, m)| m.backend()).unwrap_or_default();
        Ok(EngineLoadResult { model_id: model.model_id, backend, load_ms })
    }

    /// Loads `wanted` unless it is already loaded; returns the milliseconds spent.
    fn ensure_loaded(&mut self, wanted: &EngineModelRef) -> Result<f64, TcError> {
        if self.loaded.as_ref().is_some_and(|(r, _)| r == wanted) {
            return Ok(0.0);
        }
        // Free the previous model before loading the next.
        self.loaded = None;
        let started = Instant::now();
        let devices = self.speech.devices()?;
        let (primary, fallback) = pick_device(&devices, wanted.device);
        let Some(primary) = primary else {
            return Err(TcError::plain(if wanted.device == EngineDevice::Gpu {
                "No GPU is available for local models"
            } else {
                "No compute device found"
            }));
        };
        let model = match self.speech.load(&wanted.path, primary) {
            Ok(model) => model,
            Err(err) => {
                let Some(fallback) = fallback else { return Err(err) };
                log_line(&format!("[engine] {} failed ({}); falling back to CPU", primary.name, err.to_js_string()));
                self.speech.load(&wanted.path, fallback)?
            }
        };
        self.loaded = Some((wanted.clone(), model));
        Ok(elapsed_ms(started))
    }

    fn transcribe(
        &mut self,
        wanted: EngineModelRef,
        language: &str,
        hints: Vec<String>,
        audio: &[f32],
        abort: &AtomicBool,
    ) -> Result<EngineTranscribeResult, TcError> {
        let load_ms = self.ensure_loaded(&wanted)?;
        if abort.load(Ordering::Relaxed) {
            return Err(TcError::aborted());
        }
        let Some((_, model)) = self.loaded.as_ref() else { return Err(TcError::plain("No model is loaded")) };
        let hints: Vec<String> = hints.into_iter().filter(|h| !h.is_empty()).collect();
        let joined = hints.join(", ");
        let mut options = TranscribeOptions { language: Some(language), no_timestamps: true, ..Default::default() };
        if !hints.is_empty() {
            if model.supports(Feature::Vocabulary) {
                options.vocabulary = Some(&hints);
            } else if model.supports(Feature::InitialPrompt) {
                options.whisper_initial_prompt = Some(&joined);
            } else if model.supports(Feature::ContextPrompt) {
                options.prompt = Some(&joined);
            }
        }
        let started = Instant::now();
        let result = model.transcribe(audio, &options, abort)?;
        if result.aborted {
            return Err(TcError::aborted());
        }
        Ok(EngineTranscribeResult {
            text: js_trim(&result.text).to_owned(),
            backend: model.backend(),
            load_ms,
            infer_ms: elapsed_ms(started),
        })
    }
}

fn elapsed_ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// `String.prototype.trim`: whitespace, line terminators and the BOM.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

fn to_backend_device(d: &BackendInfo) -> BackendDevice {
    BackendDevice {
        name: d.name.clone(),
        kind: d.kind.clone(),
        device_type: d.device_type.as_str().to_owned(),
        description: d.description.trim().to_owned(),
        memory_total: d.memory_total,
    }
}

/// Picks the compute device. `auto` uses a discrete GPU when one is present
/// (CUDA before Vulkan) and the CPU otherwise; integrated GPUs are skipped
/// because they are usually slower than the CPU path for these models.
pub(crate) fn pick_device(devices: &[BackendInfo], device: EngineDevice) -> (Option<&BackendInfo>, Option<&BackendInfo>) {
    let cpu = devices.iter().find(|d| d.device_type == DeviceType::Cpu);
    let rank = |kind: &str| match kind {
        "cuda" => 0,
        "vulkan" => 1,
        _ => 2,
    };
    let mut gpus: Vec<&BackendInfo> = devices.iter().filter(|d| d.device_type == DeviceType::Gpu).collect();
    gpus.sort_by_key(|d| rank(&d.kind));
    let first_gpu = gpus.first().copied();
    let any_gpu = first_gpu.or_else(|| devices.iter().find(|d| d.device_type == DeviceType::Igpu));
    match device {
        EngineDevice::Cpu => (cpu, None),
        EngineDevice::Gpu => (any_gpu, None),
        EngineDevice::Auto => (first_gpu.or(cpu), first_gpu.and(cpu)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tcpp::{DeviceHandle, ErrorKind};
    use flow_core::protocol::engine::read_frame;
    use std::sync::mpsc::{Receiver, Sender};
    use std::time::Duration;

    fn device(name: &str, kind: &str, device_type: DeviceType) -> BackendInfo {
        BackendInfo {
            name: name.into(),
            description: format!(" {name} device "),
            kind: kind.into(),
            device_type,
            device_id: None,
            memory_total: 1 << 30,
            memory_free: 1 << 29,
            index: None,
            handle: DeviceHandle::NONE,
        }
    }

    fn names<'a>(pick: (Option<&'a BackendInfo>, Option<&'a BackendInfo>)) -> (Option<&'a str>, Option<&'a str>) {
        (pick.0.map(|d| d.name.as_str()), pick.1.map(|d| d.name.as_str()))
    }

    #[test]
    fn auto_prefers_cuda_then_vulkan_with_the_cpu_as_fallback() {
        let devices = [
            device("CPU", "cpu", DeviceType::Cpu),
            device("Vulkan0", "vulkan", DeviceType::Gpu),
            device("CUDA0", "cuda", DeviceType::Gpu),
        ];
        assert_eq!(names(pick_device(&devices, EngineDevice::Auto)), (Some("CUDA0"), Some("CPU")));
        assert_eq!(names(pick_device(&devices[..2], EngineDevice::Auto)), (Some("Vulkan0"), Some("CPU")));
    }

    #[test]
    fn auto_skips_integrated_gpus() {
        let devices = [device("Vulkan0", "vulkan", DeviceType::Igpu), device("CPU", "cpu", DeviceType::Cpu)];
        assert_eq!(names(pick_device(&devices, EngineDevice::Auto)), (Some("CPU"), None));
    }

    #[test]
    fn gpu_takes_an_integrated_gpu_when_there_is_no_other_and_never_falls_back() {
        let devices = [device("CPU", "cpu", DeviceType::Cpu), device("Vulkan0", "vulkan", DeviceType::Igpu)];
        assert_eq!(names(pick_device(&devices, EngineDevice::Gpu)), (Some("Vulkan0"), None));
        assert_eq!(names(pick_device(&devices[..1], EngineDevice::Gpu)), (None, None));
        assert_eq!(names(pick_device(&devices, EngineDevice::Cpu)), (Some("CPU"), None));
    }

    // A fake speech library recording what the engine asked of it.

    #[derive(Clone, Default)]
    struct Script {
        devices: Vec<BackendInfo>,
        /// Device names whose load fails.
        failing: Vec<String>,
        features: Vec<Feature>,
        /// Transcribe waits (abortably) this long.
        transcribe_ms: u64,
        calls: Arc<Mutex<Vec<String>>>,
    }

    struct FakeSpeech(Script);
    struct FakeModel(Script, String);

    impl Speech for FakeSpeech {
        type Model = FakeModel;
        fn version(&self) -> Result<String, TcError> {
            Ok("0.3.0".into())
        }
        fn devices(&self) -> Result<Vec<BackendInfo>, TcError> {
            Ok(self.0.devices.clone())
        }
        fn load(&self, path: &str, device: &BackendInfo) -> Result<FakeModel, TcError> {
            self.0.calls.lock().push(format!("load {path} on {}", device.name));
            if self.0.failing.contains(&device.name) {
                return Err(TcError::new(ErrorKind::Backend, "device lost"));
            }
            Ok(FakeModel(self.0.clone(), device.name.clone()))
        }
    }

    impl SpeechModel for FakeModel {
        fn backend(&self) -> String {
            self.1.clone()
        }
        fn supports(&self, feature: Feature) -> bool {
            self.0.features.contains(&feature)
        }
        fn transcribe(&self, pcm: &[f32], opts: &TranscribeOptions<'_>, abort: &AtomicBool) -> Result<TranscriptionResult, TcError> {
            self.0.calls.lock().push(format!(
                "transcribe {} samples, language {:?}, vocabulary {:?}, initial prompt {:?}, prompt {:?}",
                pcm.len(),
                opts.language,
                opts.vocabulary,
                opts.whisper_initial_prompt,
                opts.prompt
            ));
            let deadline = Instant::now() + Duration::from_millis(self.0.transcribe_ms);
            while Instant::now() < deadline {
                if abort.load(Ordering::Relaxed) {
                    return Err(TcError::new(ErrorKind::Aborted, "run aborted"));
                }
                thread::sleep(Duration::from_millis(2));
            }
            Ok(TranscriptionResult { text: "  hello world \n".into(), aborted: false, truncated: false })
        }
    }

    /// A pipe: the test writes requests, the engine reads them.
    struct PipeReader(Receiver<Vec<u8>>, Vec<u8>, usize);

    impl Read for PipeReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            while self.2 >= self.1.len() {
                match self.0.recv() {
                    Ok(chunk) => (self.1, self.2) = (chunk, 0),
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len() - self.2);
            buf[..n].copy_from_slice(&self.1[self.2..self.2 + n]);
            self.2 += n;
            Ok(n)
        }
    }

    /// Collects the engine's output.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Harness {
        requests: Option<Sender<Vec<u8>>>,
        sink: Sink,
        engine: Option<thread::JoinHandle<i32>>,
        read: usize,
    }

    impl Harness {
        fn start(script: Script) -> Self {
            let (tx, rx) = mpsc::channel();
            let sink = Sink::default();
            let out = sink.clone();
            let engine = thread::spawn(move || serve(FakeSpeech(script), PipeReader(rx, Vec::new(), 0), Box::new(out)));
            Self { requests: Some(tx), sink, engine: Some(engine), read: 0 }
        }

        fn send(&self, request: EngineRequest, pcm: &[f32]) {
            let mut buf = Vec::new();
            write_frame(&mut buf, &request, pcm).unwrap();
            self.requests.as_ref().unwrap().send(buf).unwrap();
        }

        fn send_raw(&self, bytes: Vec<u8>) {
            self.requests.as_ref().unwrap().send(bytes).unwrap();
        }

        /// The next response, waiting up to five seconds.
        fn next(&mut self) -> (EngineResponse, Vec<f32>) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                {
                    let data = self.sink.0.lock();
                    let mut cursor = io::Cursor::new(&data[self.read..]);
                    if let Ok(Some(frame)) = read_frame::<_, EngineResponse>(&mut cursor) {
                        self.read += cursor.position() as usize;
                        return frame;
                    }
                }
                assert!(Instant::now() < deadline, "no response from the engine");
                thread::sleep(Duration::from_millis(2));
            }
        }

        fn close(mut self) -> i32 {
            self.requests = None;
            self.engine.take().unwrap().join().unwrap()
        }
    }

    fn model(id: &str, device: EngineDevice) -> EngineModelRef {
        EngineModelRef { model_id: id.into(), path: format!("C:\\models\\{id}.gguf"), device }
    }

    fn script() -> Script {
        Script {
            devices: vec![device("CPU", "cpu", DeviceType::Cpu), device("Vulkan0", "vulkan", DeviceType::Gpu)],
            ..Default::default()
        }
    }

    #[test]
    fn says_hello_first_and_exits_cleanly_at_end_of_input() {
        let mut h = Harness::start(script());
        let (hello, _) = h.next();
        assert_eq!(hello, EngineResponse::Hello { hello: true, pid: std::process::id() });
        assert_eq!(h.close(), 0);
    }

    #[test]
    fn probes_version_devices_and_detector() {
        let mut h = Harness::start(script());
        h.next();
        h.send(EngineRequest::Probe { id: 1 }, &[]);
        let (response, _) = h.next();
        let EngineResponse::Ok { id: 1, result: EngineResult::Probe(probe) } = response else { panic!("{response:?}") };
        assert_eq!(probe.version, "0.3.0");
        assert_eq!(probe.vad, VadKind::Silero);
        assert_eq!(probe.devices[1].name, "Vulkan0");
        assert_eq!(probe.devices[1].device_type, "gpu");
        // Descriptions are trimmed.
        assert_eq!(probe.devices[1].description, "Vulkan0 device");
        h.close();
    }

    #[test]
    fn loads_once_and_reuses_the_loaded_model() {
        let s = script();
        let calls = Arc::clone(&s.calls);
        let mut h = Harness::start(s);
        h.next();
        h.send(EngineRequest::Load { id: 1, model: model("a", EngineDevice::Auto) }, &[]);
        let (r, _) = h.next();
        let EngineResponse::Ok { result: EngineResult::Load(load), .. } = r else { panic!("{r:?}") };
        assert_eq!((load.model_id.as_str(), load.backend.as_str()), ("a", "Vulkan0"));
        h.send(
            EngineRequest::Transcribe { id: 2, model: model("a", EngineDevice::Auto), language: "en".into(), hints: vec![] },
            &[0.0; 1600],
        );
        let (r, _) = h.next();
        let EngineResponse::Ok { result: EngineResult::Transcribe(t), .. } = r else { panic!("{r:?}") };
        assert_eq!(t.text, "hello world");
        assert_eq!(t.load_ms, 0.0);
        assert_eq!(t.backend, "Vulkan0");
        // A different device reloads.
        h.send(EngineRequest::Load { id: 3, model: model("a", EngineDevice::Cpu) }, &[]);
        h.next();
        assert_eq!(calls.lock().iter().filter(|c| c.starts_with("load")).count(), 2);
        h.close();
    }

    #[test]
    fn falls_back_to_the_cpu_when_the_gpu_fails_to_load() {
        let mut s = script();
        s.failing = vec!["Vulkan0".into()];
        let calls = Arc::clone(&s.calls);
        let mut h = Harness::start(s);
        h.next();
        h.send(EngineRequest::Load { id: 1, model: model("a", EngineDevice::Auto) }, &[]);
        let (r, _) = h.next();
        let EngineResponse::Ok { result: EngineResult::Load(load), .. } = r else { panic!("{r:?}") };
        assert_eq!(load.backend, "CPU");
        assert_eq!(*calls.lock(), ["load C:\\models\\a.gguf on Vulkan0", "load C:\\models\\a.gguf on CPU"]);
        // An explicit GPU request does not fall back.
        h.send(EngineRequest::Load { id: 2, model: model("a", EngineDevice::Gpu) }, &[]);
        assert_eq!(h.next().0, EngineResponse::Err { id: 2, error: "device lost".into(), aborted: false });
        h.close();
    }

    #[test]
    fn reports_a_missing_device() {
        let mut s = script();
        s.devices.truncate(1);
        let mut h = Harness::start(s);
        h.next();
        h.send(EngineRequest::Load { id: 1, model: model("a", EngineDevice::Gpu) }, &[]);
        assert_eq!(
            h.next().0,
            EngineResponse::Err { id: 1, error: "No GPU is available for local models".into(), aborted: false }
        );
        h.close();
    }

    #[test]
    fn passes_hints_the_way_the_model_supports_them() {
        for (features, expected) in [
            (vec![Feature::Vocabulary, Feature::InitialPrompt], "vocabulary Some([\"Tauri\", \"Flow\"]), initial prompt None, prompt None"),
            (vec![Feature::InitialPrompt], "vocabulary None, initial prompt Some(\"Tauri, Flow\"), prompt None"),
            (vec![Feature::ContextPrompt], "vocabulary None, initial prompt None, prompt Some(\"Tauri, Flow\")"),
            (vec![], "vocabulary None, initial prompt None, prompt None"),
        ] {
            let mut s = script();
            s.features = features;
            let calls = Arc::clone(&s.calls);
            let mut h = Harness::start(s);
            h.next();
            let hints = vec!["Tauri".into(), String::new(), "Flow".into()];
            h.send(EngineRequest::Transcribe { id: 1, model: model("a", EngineDevice::Cpu), language: "fr".into(), hints }, &[0.1; 320]);
            h.next();
            let call = calls.lock().last().cloned().unwrap();
            assert_eq!(call, format!("transcribe 320 samples, language Some(\"fr\"), {expected}"));
            h.close();
        }
    }

    #[test]
    fn cancel_aborts_its_target_and_the_error_says_so() {
        let mut s = script();
        s.transcribe_ms = 5_000;
        let mut h = Harness::start(s);
        h.next();
        let started = Instant::now();
        h.send(EngineRequest::Transcribe { id: 7, model: model("a", EngineDevice::Cpu), language: "en".into(), hints: vec![] }, &[0.0; 16]);
        thread::sleep(Duration::from_millis(100));
        // Cancelling something else does nothing.
        h.send(EngineRequest::Cancel { id: 8, target: 99 }, &[]);
        h.send(EngineRequest::Cancel { id: 9, target: 7 }, &[]);
        assert_eq!(h.next().0, EngineResponse::Err { id: 7, error: "run aborted".into(), aborted: true });
        assert!(started.elapsed() < Duration::from_secs(2));
        h.close();
    }

    #[test]
    fn trim_runs_alongside_model_work() {
        let mut s = script();
        s.transcribe_ms = 1_500;
        let mut h = Harness::start(s);
        h.next();
        h.send(EngineRequest::Transcribe { id: 1, model: model("a", EngineDevice::Cpu), language: "en".into(), hints: vec![] }, &[0.0; 16]);
        h.send(EngineRequest::Trim { id: 2 }, &vec![0.0; 16_000]);
        // The trim answers first even though it was sent second.
        let (first, pcm) = h.next();
        assert_eq!(
            first,
            EngineResponse::Ok {
                id: 2,
                result: EngineResult::Trim(EngineTrimResult { has_speech: false, speech_ms: 0.0, detector: VadKind::Silero })
            }
        );
        assert!(pcm.is_empty());
        let (second, _) = h.next();
        assert!(matches!(second, EngineResponse::Ok { id: 1, .. }), "{second:?}");
        h.close();
    }

    #[test]
    fn model_work_is_serialised_in_order() {
        let mut s = script();
        s.transcribe_ms = 200;
        let mut h = Harness::start(s);
        h.next();
        h.send(EngineRequest::Transcribe { id: 1, model: model("a", EngineDevice::Cpu), language: "en".into(), hints: vec![] }, &[0.0; 16]);
        h.send(EngineRequest::Unload { id: 2 }, &[]);
        h.send(EngineRequest::Probe { id: 3 }, &[]);
        let ids: Vec<u64> = (0..3)
            .map(|_| match h.next().0 {
                EngineResponse::Ok { id, .. } => id,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(ids, [1, 2, 3]);
        h.close();
    }

    #[test]
    fn answers_an_unknown_request_with_an_error() {
        let mut h = Harness::start(script());
        h.next();
        let mut buf = Vec::new();
        write_frame(&mut buf, &serde_json::json!({ "type": "dance", "id": 5 }), &[]).unwrap();
        h.send_raw(buf);
        let (r, _) = h.next();
        let EngineResponse::Err { id: 5, error, aborted: false } = r else { panic!("{r:?}") };
        assert!(error.starts_with("Unknown request"));
        // The engine keeps serving.
        h.send(EngineRequest::Unload { id: 6 }, &[]);
        assert_eq!(h.next().0, EngineResponse::Ok { id: 6, result: EngineResult::Unload });
        h.close();
    }

    #[test]
    fn exits_with_an_error_on_a_corrupt_stream() {
        let mut h = Harness::start(script());
        h.next();
        h.send_raw(u32::MAX.to_le_bytes().to_vec());
        assert_eq!(h.close(), 1);
    }

    #[test]
    fn trims_silence_around_a_tone_with_the_energy_detector() {
        let mut audio = vec![0.0_f32; 16_000 * 3];
        for (i, s) in audio[16_000..32_000].iter_mut().enumerate() {
            *s = (i as f32 / 5.0).sin() * 0.3;
        }
        let probs = energy_probs(&audio);
        let segments = segments_from_probs(&probs, 3000.0, &DEFAULT_SEGMENT_OPTIONS);
        assert_eq!(segments.len(), 1);
        assert!(extract_segments(&audio, &segments).len() < audio.len());
    }

    #[test]
    fn trims_like_javascript() {
        assert_eq!(js_trim("\u{feff} hi \n"), "hi");
    }
}
