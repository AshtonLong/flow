//! transcribe.cpp 0.3.0 through its C ABI: the subset of the npm bindings'
//! public API (`dist/index.js`) that Flow's engine used — `version()`,
//! `getAvailableBackends()`, `TranscribeModel.load(path, { device })`,
//! `supports()`, `backend`, `transcribe()` with cancellation, and `dispose()`
//! (here: `Drop`).

pub mod ffi;
pub mod loader;

use std::ffi::{CString, c_char, c_int, c_void};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use loader::{Native, native, set_log_to_stderr};

use ffi::{Handle, TRANSCRIBE_OK};
use loader::{c_str, status_text};

/// The bindings' error classes, mapped from `transcribe_status` (plus `Plain`
/// for the engine's own `new Error(...)`s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Plain,
    Transcribe,
    InvalidArgument,
    NotImplementedByModel,
    ModelFileNotFound,
    ModelLoad,
    OutOfMemory,
    Backend,
    UnsupportedRequest,
    Abi,
    InputTooLong,
    VersionMismatch,
    Aborted,
    OutputTruncated,
    OutputRepetition,
}

impl ErrorKind {
    /// The JS class name, as `String(err)` would print it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "Error",
            Self::Transcribe => "TranscribeError",
            Self::InvalidArgument => "InvalidArgument",
            Self::NotImplementedByModel => "NotImplementedByModel",
            Self::ModelFileNotFound => "ModelFileNotFound",
            Self::ModelLoad => "ModelLoadError",
            Self::OutOfMemory => "OutOfMemory",
            Self::Backend => "BackendError",
            Self::UnsupportedRequest => "UnsupportedRequest",
            Self::Abi => "AbiError",
            Self::InputTooLong => "InputTooLong",
            Self::VersionMismatch => "VersionMismatch",
            Self::Aborted => "Aborted",
            Self::OutputTruncated => "OutputTruncated",
            Self::OutputRepetition => "OutputRepetition",
        }
    }

    fn for_status(status: c_int) -> Self {
        use ffi::*;
        match status {
            TRANSCRIBE_ERR_INVALID_ARG | TRANSCRIBE_ERR_SAMPLE_RATE => Self::InvalidArgument,
            TRANSCRIBE_ERR_NOT_IMPLEMENTED => Self::NotImplementedByModel,
            TRANSCRIBE_ERR_FILE_NOT_FOUND => Self::ModelFileNotFound,
            TRANSCRIBE_ERR_GGUF | TRANSCRIBE_ERR_UNSUPPORTED_ARCH | TRANSCRIBE_ERR_UNSUPPORTED_VARIANT => {
                Self::ModelLoad
            }
            TRANSCRIBE_ERR_OOM => Self::OutOfMemory,
            TRANSCRIBE_ERR_BACKEND => Self::Backend,
            TRANSCRIBE_ERR_UNSUPPORTED_LANGUAGE
            | TRANSCRIBE_ERR_UNSUPPORTED_TASK
            | TRANSCRIBE_ERR_UNSUPPORTED_TIMESTAMPS
            | TRANSCRIBE_ERR_UNSUPPORTED_PNC
            | TRANSCRIBE_ERR_UNSUPPORTED_ITN => Self::UnsupportedRequest,
            TRANSCRIBE_ERR_ABORTED => Self::Aborted,
            TRANSCRIBE_ERR_BAD_STRUCT_SIZE => Self::Abi,
            TRANSCRIBE_ERR_INPUT_TOO_LONG => Self::InputTooLong,
            TRANSCRIBE_ERR_OUTPUT_TRUNCATED => Self::OutputTruncated,
            TRANSCRIBE_ERR_OUTPUT_REPETITION => Self::OutputRepetition,
            _ => Self::Transcribe,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcError {
    pub kind: ErrorKind,
    pub message: String,
    /// The `transcribe_status` value (0 for binding-side errors).
    pub status: i32,
}

impl TcError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), status: 0 }
    }

    pub fn with_status(kind: ErrorKind, message: impl Into<String>, status: i32) -> Self {
        Self { kind, message: message.into(), status }
    }

    /// A plain `new Error(message)`.
    pub fn plain(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Plain, message)
    }

    /// The `AbortError` the engine throws when a request is cancelled between steps.
    pub fn aborted() -> Self {
        Self::new(ErrorKind::Aborted, "Aborted")
    }

    pub fn is_abort(&self) -> bool {
        self.kind == ErrorKind::Aborted
    }

    /// `String(err)` in JS: `Name: message`.
    pub fn to_js_string(&self) -> String {
        format!("{}: {}", self.kind.name(), self.message)
    }

    /// `exceptionForStatus`: `"<context>: <status string> (status N)"`.
    fn for_status(native: &Native, status: c_int, context: &str) -> Self {
        let text = status_text(&native.api, status);
        Self::with_status(ErrorKind::for_status(status), format!("{context}: {text} (status {status})"), status)
    }
}

impl fmt::Display for TcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TcError {}

fn check(native: &Native, status: c_int, context: &str) -> Result<(), TcError> {
    if status == TRANSCRIBE_OK { Ok(()) } else { Err(TcError::for_status(native, status, context)) }
}

/// A string option as passed to C, which would silently cut it at a NUL.
fn cstr(value: &str, name: &str) -> Result<CString, TcError> {
    CString::new(value)
        .map_err(|_| TcError::new(ErrorKind::InvalidArgument, format!("{name} contains a NUL character")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub version: String,
    pub commit: String,
    /// The header hash this binding expects (not read from the library).
    pub header_hash: &'static str,
}

pub fn version() -> Result<Version, TcError> {
    let n = native()?;
    // SAFETY: plain getters returning static strings.
    let (version, commit) = unsafe { ((n.api.version)(), (n.api.version_commit)()) };
    Ok(Version {
        version: unsafe { c_str(version) }.unwrap_or_default(),
        commit: unsafe { c_str(commit) }.unwrap_or_default(),
        header_hash: ffi::PUBLIC_HEADER_HASH,
    })
}

/// Vendor-agnostic device class, orthogonal to `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Cpu,
    Gpu,
    Igpu,
    Accel,
    Unknown,
}

impl DeviceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Igpu => "igpu",
            Self::Accel => "accel",
            Self::Unknown => "unknown",
        }
    }

    fn from_raw(raw: c_int) -> Self {
        match raw {
            ffi::TRANSCRIBE_DEVICE_TYPE_CPU => Self::Cpu,
            ffi::TRANSCRIBE_DEVICE_TYPE_GPU => Self::Gpu,
            ffi::TRANSCRIBE_DEVICE_TYPE_IGPU => Self::Igpu,
            ffi::TRANSCRIBE_DEVICE_TYPE_ACCEL => Self::Accel,
            _ => Self::Unknown,
        }
    }
}

/// An opaque `transcribe_device_t`: an entry of the process-wide device
/// registry, valid for the life of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceHandle(Handle);

impl DeviceHandle {
    /// A handle that selects nothing; for devices that do not come from the library.
    pub const NONE: Self = Self(std::ptr::null_mut());
}

// SAFETY: registry entries are immutable and live for the whole process.
unsafe impl Send for DeviceHandle {}
unsafe impl Sync for DeviceHandle {}

/// One compute device (`BackendInfo` in the bindings).
#[derive(Debug, Clone, PartialEq)]
pub struct BackendInfo {
    pub name: String,
    pub description: String,
    pub kind: String,
    pub device_type: DeviceType,
    /// Stable hardware id (PCI bus id) when the backend reports one.
    pub device_id: Option<String>,
    /// Reported device memory capacity in bytes, or 0 if unreported.
    pub memory_total: u64,
    /// Available device memory in bytes at query time, or 0 if unreported.
    pub memory_free: u64,
    /// Process-local registry index for display.
    pub index: Option<usize>,
    pub handle: DeviceHandle,
}

fn device_from_raw(n: &Native, handle: Handle, index: Option<usize>, context: &str) -> Result<BackendInfo, TcError> {
    // SAFETY: zeroed is a valid bit pattern for this plain struct; the init
    // call fills struct_size and the defaults before the accessor reads it.
    let mut dev: ffi::DeviceInfo = unsafe { std::mem::zeroed() };
    unsafe { (n.api.device_info_init)(&mut dev) };
    check(n, unsafe { (n.api.device_get_info)(handle, &mut dev) }, context)?;
    Ok(BackendInfo {
        name: unsafe { c_str(dev.name) }.unwrap_or_default(),
        description: unsafe { c_str(dev.description) }.unwrap_or_default(),
        kind: unsafe { c_str(dev.kind) }.unwrap_or_default(),
        device_type: DeviceType::from_raw(dev.device_type),
        device_id: unsafe { c_str(dev.device_id) },
        memory_total: dev.memory_total,
        memory_free: dev.memory_free,
        index,
        handle: DeviceHandle(handle),
    })
}

pub fn get_available_backends() -> Result<Vec<BackendInfo>, TcError> {
    let n = native()?;
    // SAFETY: plain getters on the device registry.
    let count = unsafe { (n.api.device_count)() };
    let mut out = Vec::new();
    for i in 0..count.max(0) {
        let handle = unsafe { (n.api.device_get)(i) };
        if handle.is_null() {
            continue;
        }
        out.push(device_from_raw(&n, handle, Some(i as usize), &format!("reading backend device {i}"))?);
    }
    Ok(out)
}

/// Model features (`model.supports(feature)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    InitialPrompt,
    TemperatureFallback,
    LongForm,
    Cancellation,
    Pnc,
    Itn,
    Diarization,
    Vocabulary,
    ContextPrompt,
    Instruct,
    TranscriptPrefix,
}

impl Feature {
    fn raw(self) -> c_int {
        match self {
            Self::InitialPrompt => ffi::TRANSCRIBE_FEATURE_INITIAL_PROMPT,
            Self::TemperatureFallback => ffi::TRANSCRIBE_FEATURE_TEMPERATURE_FALLBACK,
            Self::LongForm => ffi::TRANSCRIBE_FEATURE_LONG_FORM,
            Self::Cancellation => ffi::TRANSCRIBE_FEATURE_CANCELLATION,
            Self::Pnc => ffi::TRANSCRIBE_FEATURE_PNC,
            Self::Itn => ffi::TRANSCRIBE_FEATURE_ITN,
            Self::Diarization => ffi::TRANSCRIBE_FEATURE_DIARIZATION,
            Self::Vocabulary => ffi::TRANSCRIBE_FEATURE_VOCABULARY,
            Self::ContextPrompt => ffi::TRANSCRIBE_FEATURE_CONTEXT_PROMPT,
            Self::Instruct => ffi::TRANSCRIBE_FEATURE_INSTRUCT,
            Self::TranscriptPrefix => ffi::TRANSCRIBE_FEATURE_TRANSCRIPT_PREFIX,
        }
    }
}

/// The run options Flow uses (a subset of the bindings' `TranscribeOptions`).
#[derive(Debug, Clone, Default)]
pub struct TranscribeOptions<'a> {
    pub language: Option<&'a str>,
    /// `timestamps: 'none'` when true, else the library's `auto`.
    pub no_timestamps: bool,
    /// Custom terms in priority order ("vocabulary" feature).
    pub vocabulary: Option<&'a [String]>,
    /// The whisper run extension's `initialPrompt`.
    pub whisper_initial_prompt: Option<&'a str>,
    /// Context text ("context_prompt" feature).
    pub prompt: Option<&'a str>,
}

/// An offline transcript plus the terminal run flags (only what Flow reads).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionResult {
    pub text: String,
    pub aborted: bool,
    pub truncated: bool,
}

/// A loaded model. Not shared between threads: the engine keeps it on its one worker.
pub struct TranscribeModel {
    native: Arc<Native>,
    handle: Handle,
}

// SAFETY: the model may move between threads; it is only ever used from one
// thread at a time (it is not Sync), which also serialises compute on it.
unsafe impl Send for TranscribeModel {}

/// Reads the abort flag the run was given. Must never unwind.
unsafe extern "C" fn abort_thunk(user_data: *mut c_void) -> bool {
    // SAFETY: the pointer is the `AtomicBool` installed for the run, which
    // outlives it (the callback is removed before the run's borrow ends).
    !user_data.is_null() && unsafe { (*(user_data as *const AtomicBool)).load(Ordering::Relaxed) }
}

impl TranscribeModel {
    /// Loads a GGUF model, on `device` when given (exact selection never falls
    /// back), else by the library's automatic policy.
    pub fn load(path: &str, device: Option<&BackendInfo>) -> Result<Self, TcError> {
        let n = native()?;
        // SAFETY: zeroed is valid for this plain struct; init fills it.
        let mut params: ffi::ModelLoadParams = unsafe { std::mem::zeroed() };
        unsafe { (n.api.model_load_params_init)(&mut params) };
        if let Some(device) = device {
            if device.handle == DeviceHandle::NONE {
                return Err(TcError::new(
                    ErrorKind::Transcribe,
                    "device must be an entry returned by getAvailableBackends() or model.device",
                ));
            }
            params.device = device.handle.0;
        }
        let path_c = cstr(path, "path")?;
        let mut out: Handle = std::ptr::null_mut();
        // SAFETY: valid params and path for the duration of the call.
        let status = unsafe { (n.api.model_load_file)(path_c.as_ptr(), &params, &mut out) };
        check(&n, status, &format!("loading model {path}"))?;
        if out.is_null() {
            return Err(TcError::new(ErrorKind::ModelLoad, format!("model load returned a null handle for {path}")));
        }
        Ok(Self { native: n, handle: out })
    }

    pub fn supports(&self, feature: Feature) -> bool {
        // SAFETY: valid model handle.
        unsafe { (self.native.api.model_supports)(self.handle, feature.raw()) }
    }

    /// The backend the model runs on, e.g. `CPU` or `Vulkan0`.
    pub fn backend(&self) -> String {
        // SAFETY: valid model handle; the string lives as long as the model.
        unsafe { c_str((self.native.api.model_backend)(self.handle)) }.unwrap_or_default()
    }

    pub fn arch(&self) -> String {
        unsafe { c_str((self.native.api.model_arch_string)(self.handle)) }.unwrap_or_default()
    }

    pub fn variant(&self) -> String {
        unsafe { c_str((self.native.api.model_variant_string)(self.handle)) }.unwrap_or_default()
    }

    /// The compute device the model resolved to.
    pub fn device(&self) -> Result<BackendInfo, TcError> {
        let handle = unsafe { (self.native.api.model_device)(self.handle) };
        if handle.is_null() {
            return Err(TcError::new(ErrorKind::Transcribe, "model has no resolved compute device"));
        }
        device_from_raw(&self.native, handle, None, "reading model device")
    }

    /// One session, one run, freed after (the bindings' `model.transcribe`).
    /// `abort` cancels the run cooperatively through the library's abort callback.
    pub fn transcribe(
        &self,
        pcm: &[f32],
        opts: &TranscribeOptions<'_>,
        abort: Option<&AtomicBool>,
    ) -> Result<TranscriptionResult, TcError> {
        let n = &*self.native;
        if pcm.is_empty() {
            return Err(TcError::new(ErrorKind::Transcribe, "PCM is empty"));
        }
        let samples =
            c_int::try_from(pcm.len()).map_err(|_| TcError::new(ErrorKind::InputTooLong, "PCM is too long"))?;
        let params = RunParams::build(self, opts)?;
        let session = Session::open(self)?;

        if let Some(flag) = abort {
            let user_data = flag as *const AtomicBool as *mut c_void;
            // SAFETY: the flag outlives the run; the callback is removed below.
            unsafe { (n.api.set_abort_callback)(session.handle, Some(abort_thunk), user_data) };
        }
        // SAFETY: the session, samples and params (with every string and array
        // they point to) stay alive for the duration of the call.
        let status = unsafe { (n.api.run)(session.handle, pcm.as_ptr(), samples, &params.raw) };
        if abort.is_some() {
            unsafe { (n.api.set_abort_callback)(session.handle, None, std::ptr::null_mut()) };
        }

        match status {
            ffi::TRANSCRIBE_ERR_ABORTED => {
                return Err(TcError::with_status(ErrorKind::Aborted, "run aborted", status));
            }
            ffi::TRANSCRIBE_ERR_OUTPUT_REPETITION => {
                return Err(TcError::with_status(
                    ErrorKind::OutputRepetition,
                    "run stopped: output began repeating",
                    status,
                ));
            }
            ffi::TRANSCRIBE_ERR_OUTPUT_TRUNCATED => {
                return Err(TcError::with_status(ErrorKind::OutputTruncated, "run output truncated", status));
            }
            _ => check(n, status, "transcribe_run")?,
        }
        // SAFETY: valid session; the text is copied before the session is freed.
        let text = unsafe { c_str((n.api.full_text)(session.handle)) }.unwrap_or_default();
        let (aborted, truncated) =
            unsafe { ((n.api.was_aborted)(session.handle), (n.api.was_truncated)(session.handle)) };
        Ok(TranscriptionResult { text, aborted, truncated })
    }
}

impl Drop for TranscribeModel {
    fn drop(&mut self) {
        // SAFETY: sessions are freed at the end of each run, so the model has
        // no live sessions here; the handle is not used again.
        unsafe { (self.native.api.model_free)(self.handle) };
    }
}

struct Session<'m> {
    model: &'m TranscribeModel,
    handle: Handle,
}

impl<'m> Session<'m> {
    fn open(model: &'m TranscribeModel) -> Result<Self, TcError> {
        let n = &*model.native;
        // SAFETY: zeroed is valid for this plain struct; init fills it.
        let mut params: ffi::SessionParams = unsafe { std::mem::zeroed() };
        unsafe { (n.api.session_params_init)(&mut params) };
        let mut out: Handle = std::ptr::null_mut();
        check(n, unsafe { (n.api.session_init)(model.handle, &params, &mut out) }, "opening session")?;
        if out.is_null() {
            return Err(TcError::new(ErrorKind::Transcribe, "session init returned a null handle"));
        }
        Ok(Self { model, handle: out })
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        // SAFETY: the run has returned; the handle is not used again.
        unsafe { (self.model.native.api.session_free)(self.handle) };
    }
}

/// `transcribe_run_params` plus everything its pointers point to.
struct RunParams {
    raw: ffi::RunParams,
    _strings: Vec<CString>,
    _vocabulary: Vec<*const c_char>,
    _whisper: Option<Box<ffi::WhisperRunExt>>,
}

impl RunParams {
    fn build(model: &TranscribeModel, opts: &TranscribeOptions<'_>) -> Result<Self, TcError> {
        let n = &*model.native;
        // SAFETY: zeroed is valid for this plain struct; init fills it.
        let mut raw: ffi::RunParams = unsafe { std::mem::zeroed() };
        unsafe { (n.api.run_params_init)(&mut raw) };
        raw.task = ffi::TRANSCRIBE_TASK_TRANSCRIBE;
        raw.timestamps =
            if opts.no_timestamps { ffi::TRANSCRIBE_TIMESTAMPS_NONE } else { ffi::TRANSCRIBE_TIMESTAMPS_AUTO };
        raw.pnc = ffi::TRANSCRIBE_MODE_DEFAULT;
        raw.itn = ffi::TRANSCRIBE_MODE_DEFAULT;
        raw.diarize = ffi::TRANSCRIBE_MODE_DEFAULT;
        let mut strings = Vec::new();
        // CString's heap buffer does not move when the CString is moved into the Vec.
        let mut keep = |s: CString| {
            let ptr = s.as_ptr();
            strings.push(s);
            ptr
        };
        if let Some(language) = opts.language {
            raw.language = keep(cstr(language, "language")?);
        }
        let mut whisper = None;
        if let Some(prompt) = opts.whisper_initial_prompt {
            // SAFETY: valid model handle.
            let accepted = unsafe {
                (n.api.model_accepts_ext_kind)(
                    model.handle,
                    ffi::TRANSCRIBE_EXT_SLOT_RUN,
                    ffi::TRANSCRIBE_EXT_KIND_WHISPER_RUN,
                )
            };
            if !accepted {
                return Err(TcError::new(
                    ErrorKind::UnsupportedRequest,
                    "this model does not accept the \"whisper\" run extension",
                ));
            }
            // SAFETY: zeroed is valid; init fills the defaults, struct size and kind.
            let mut ext: Box<ffi::WhisperRunExt> = Box::new(unsafe { std::mem::zeroed() });
            unsafe { (n.api.whisper_run_ext_init)(&mut *ext) };
            ext.initial_prompt = keep(cstr(prompt, "initialPrompt")?);
            raw.family = &*ext as *const ffi::WhisperRunExt as *const c_void;
            whisper = Some(ext);
        }
        let mut vocabulary = Vec::new();
        if let Some(terms) = opts.vocabulary.filter(|t| !t.is_empty()) {
            for term in terms {
                vocabulary.push(keep(cstr(term, "vocabulary")?));
            }
            raw.vocabulary = vocabulary.as_ptr();
            raw.n_vocabulary = i32::try_from(vocabulary.len())
                .map_err(|_| TcError::new(ErrorKind::InvalidArgument, "vocabulary is too long"))?;
        }
        if let Some(prompt) = opts.prompt {
            raw.prompt = keep(cstr(prompt, "prompt")?);
        }
        Ok(Self { raw, _strings: strings, _vocabulary: vocabulary, _whisper: whisper })
    }
}
