//! The raw C ABI of transcribe.cpp **0.3.0**, hand-bound from the npm bindings
//! (`transcribe-cpp@0.3.0`: `dist/ffi.js` for the signatures,
//! `dist/_generated.js` for the struct layouts and enums). Only the part Flow
//! uses is bound: identity, backends and devices, logging, model load and
//! introspection, sessions, offline runs with cancellation, and the whisper
//! run extension. Streaming and batch runs are left out.
//!
//! Struct layouts are checked twice, like the bindings do: against the layout
//! captured at generation time (`STRUCT_LAYOUT`, as compile-time asserts
//! below) and against the loaded library (`transcribe_abi_struct_size` /
//! `_align`, in [`Api::verify_layouts`]).

use std::ffi::{c_char, c_int, c_void};
use std::mem::{align_of, offset_of, size_of};
use std::path::Path;

/// The version this binding was written against; the library must match its
/// base version exactly (pre-1.0).
pub const OUR_VERSION: &str = "0.3.0";
/// Digest of the public header ABI the bindings were generated from.
pub const PUBLIC_HEADER_HASH: &str = "59b9a92b47074666";

// transcribe_status
pub const TRANSCRIBE_OK: c_int = 0;
pub const TRANSCRIBE_ERR_INVALID_ARG: c_int = 1;
pub const TRANSCRIBE_ERR_NOT_IMPLEMENTED: c_int = 2;
pub const TRANSCRIBE_ERR_FILE_NOT_FOUND: c_int = 3;
pub const TRANSCRIBE_ERR_GGUF: c_int = 4;
pub const TRANSCRIBE_ERR_UNSUPPORTED_ARCH: c_int = 5;
pub const TRANSCRIBE_ERR_UNSUPPORTED_VARIANT: c_int = 6;
pub const TRANSCRIBE_ERR_OOM: c_int = 7;
pub const TRANSCRIBE_ERR_BACKEND: c_int = 8;
pub const TRANSCRIBE_ERR_SAMPLE_RATE: c_int = 9;
pub const TRANSCRIBE_ERR_UNSUPPORTED_LANGUAGE: c_int = 10;
pub const TRANSCRIBE_ERR_UNSUPPORTED_TASK: c_int = 11;
pub const TRANSCRIBE_ERR_UNSUPPORTED_TIMESTAMPS: c_int = 12;
pub const TRANSCRIBE_ERR_ABORTED: c_int = 13;
pub const TRANSCRIBE_ERR_BAD_STRUCT_SIZE: c_int = 14;
pub const TRANSCRIBE_ERR_UNSUPPORTED_PNC: c_int = 15;
pub const TRANSCRIBE_ERR_UNSUPPORTED_ITN: c_int = 16;
pub const TRANSCRIBE_ERR_INPUT_TOO_LONG: c_int = 17;
pub const TRANSCRIBE_ERR_OUTPUT_TRUNCATED: c_int = 18;
pub const TRANSCRIBE_ERR_OUTPUT_REPETITION: c_int = 19;

// transcribe_abi_struct
pub const TRANSCRIBE_ABI_MODEL_LOAD_PARAMS: c_int = 0;
pub const TRANSCRIBE_ABI_SESSION_PARAMS: c_int = 1;
pub const TRANSCRIBE_ABI_RUN_PARAMS: c_int = 2;
pub const TRANSCRIBE_ABI_TIMINGS: c_int = 5;
pub const TRANSCRIBE_ABI_EXT: c_int = 12;
pub const TRANSCRIBE_ABI_DEVICE_INFO: c_int = 13;

// transcribe_log_level
pub const TRANSCRIBE_LOG_LEVEL_CONT: c_int = 5;

// transcribe_task / transcribe_timestamp_kind
pub const TRANSCRIBE_TASK_TRANSCRIBE: c_int = 0;
pub const TRANSCRIBE_TIMESTAMPS_NONE: c_int = 0;
pub const TRANSCRIBE_TIMESTAMPS_AUTO: c_int = 1;

// transcribe_pnc_mode / transcribe_itn_mode / transcribe_diarize_mode
pub const TRANSCRIBE_MODE_DEFAULT: c_int = 0;

// transcribe_backend_request
pub const TRANSCRIBE_BACKEND_AUTO: c_int = 0;

// transcribe_device_type
pub const TRANSCRIBE_DEVICE_TYPE_CPU: c_int = 0;
pub const TRANSCRIBE_DEVICE_TYPE_GPU: c_int = 1;
pub const TRANSCRIBE_DEVICE_TYPE_IGPU: c_int = 2;
pub const TRANSCRIBE_DEVICE_TYPE_ACCEL: c_int = 3;

// transcribe_feature
pub const TRANSCRIBE_FEATURE_INITIAL_PROMPT: c_int = 0;
pub const TRANSCRIBE_FEATURE_TEMPERATURE_FALLBACK: c_int = 1;
pub const TRANSCRIBE_FEATURE_LONG_FORM: c_int = 2;
pub const TRANSCRIBE_FEATURE_CANCELLATION: c_int = 3;
pub const TRANSCRIBE_FEATURE_PNC: c_int = 4;
pub const TRANSCRIBE_FEATURE_ITN: c_int = 5;
pub const TRANSCRIBE_FEATURE_DIARIZATION: c_int = 6;
pub const TRANSCRIBE_FEATURE_VOCABULARY: c_int = 7;
pub const TRANSCRIBE_FEATURE_CONTEXT_PROMPT: c_int = 8;
pub const TRANSCRIBE_FEATURE_INSTRUCT: c_int = 9;
pub const TRANSCRIBE_FEATURE_TRANSCRIPT_PREFIX: c_int = 10;

// transcribe_ext_slot / family extension kinds
pub const TRANSCRIBE_EXT_SLOT_RUN: c_int = 0;
pub const TRANSCRIBE_EXT_KIND_WHISPER_RUN: u32 = 1_314_015_319;

pub type Handle = *mut c_void;
pub type LogCallback = Option<unsafe extern "C" fn(level: c_int, message: *const c_char, user_data: *mut c_void)>;
pub type AbortCallback = Option<unsafe extern "C" fn(user_data: *mut c_void) -> bool>;

#[repr(C)]
pub struct DeviceInfo {
    pub struct_size: u64,
    pub name: *const c_char,
    pub description: *const c_char,
    pub kind: *const c_char,
    pub device_id: *const c_char,
    pub memory_total: u64,
    pub memory_free: u64,
    pub device_type: c_int,
}

#[repr(C)]
pub struct ModelLoadParams {
    pub struct_size: u64,
    pub backend: c_int,
    pub device: Handle,
}

#[repr(C)]
pub struct SessionParams {
    pub struct_size: u64,
    pub n_threads: c_int,
    pub kv_type: c_int,
    pub n_ctx: i32,
}

#[repr(C)]
pub struct RunParams {
    pub struct_size: u64,
    pub task: c_int,
    pub timestamps: c_int,
    pub pnc: c_int,
    pub itn: c_int,
    pub diarize: c_int,
    pub language: *const c_char,
    pub target_language: *const c_char,
    pub keep_special_tags: bool,
    pub family: *const c_void,
    pub spec_k_drafts: i32,
    pub vocabulary: *const *const c_char,
    pub n_vocabulary: i32,
    pub prompt: *const c_char,
    pub prefix: *const c_char,
}

#[repr(C)]
pub struct Timings {
    pub struct_size: u64,
    pub load_ms: f32,
    pub mel_ms: f32,
    pub encode_ms: f32,
    pub decode_ms: f32,
}

#[repr(C)]
pub struct Ext {
    pub size: u64,
    pub kind: u32,
}

#[repr(C)]
pub struct WhisperRunExt {
    pub ext: Ext,
    pub initial_prompt: *const c_char,
    pub prompt_tokens: *const i32,
    pub n_prompt_tokens: usize,
    pub prompt_condition: c_int,
    pub condition_on_prev_tokens: bool,
    pub max_prev_context_tokens: i32,
    pub temperature: f32,
    pub temperature_inc: f32,
    pub compression_ratio_thold: f32,
    pub logprob_thold: f32,
    pub no_speech_thold: f32,
    pub seed: u32,
    pub max_initial_timestamp: f32,
}

/// Layout self-check: the sizes, alignments and offsets the C compiler captured
/// when the 0.3.0 bindings were generated (`STRUCT_LAYOUT`).
macro_rules! layout {
    ($t:ty, $size:expr, $align:expr, { $($field:ident: $off:expr),* $(,)? }) => {
        const _: () = {
            assert!(size_of::<$t>() == $size);
            assert!(align_of::<$t>() == $align);
            $(assert!(offset_of!($t, $field) == $off);)*
        };
    };
}

layout!(Ext, 16, 8, { size: 0, kind: 8 });
layout!(DeviceInfo, 64, 8, {
    struct_size: 0, name: 8, description: 16, kind: 24, device_id: 32,
    memory_total: 40, memory_free: 48, device_type: 56,
});
layout!(ModelLoadParams, 24, 8, { struct_size: 0, backend: 8, device: 16 });
layout!(SessionParams, 24, 8, { struct_size: 0, n_threads: 8, kv_type: 12, n_ctx: 16 });
layout!(RunParams, 104, 8, {
    struct_size: 0, task: 8, timestamps: 12, pnc: 16, itn: 20, diarize: 24, language: 32,
    target_language: 40, keep_special_tags: 48, family: 56, spec_k_drafts: 64, vocabulary: 72,
    n_vocabulary: 80, prompt: 88, prefix: 96,
});
layout!(Timings, 24, 8, { struct_size: 0, load_ms: 8, mel_ms: 12, encode_ms: 16, decode_ms: 20 });
layout!(WhisperRunExt, 80, 8, {
    ext: 0, initial_prompt: 16, prompt_tokens: 24, n_prompt_tokens: 32, prompt_condition: 40,
    condition_on_prev_tokens: 44, max_prev_context_tokens: 48, temperature: 52, temperature_inc: 56,
    compression_ratio_thold: 60, logprob_thold: 64, no_speech_thold: 68, seed: 72, max_initial_timestamp: 76,
});

/// The structs Flow passes across the boundary, with their `transcribe_abi_struct` ids.
const ABI_STRUCTS: &[(&str, c_int, usize, usize)] = &[
    ("transcribe_ext", TRANSCRIBE_ABI_EXT, size_of::<Ext>(), align_of::<Ext>()),
    ("transcribe_device_info", TRANSCRIBE_ABI_DEVICE_INFO, size_of::<DeviceInfo>(), align_of::<DeviceInfo>()),
    (
        "transcribe_model_load_params",
        TRANSCRIBE_ABI_MODEL_LOAD_PARAMS,
        size_of::<ModelLoadParams>(),
        align_of::<ModelLoadParams>(),
    ),
    ("transcribe_session_params", TRANSCRIBE_ABI_SESSION_PARAMS, size_of::<SessionParams>(), align_of::<SessionParams>()),
    ("transcribe_run_params", TRANSCRIBE_ABI_RUN_PARAMS, size_of::<RunParams>(), align_of::<RunParams>()),
    ("transcribe_timings", TRANSCRIBE_ABI_TIMINGS, size_of::<Timings>(), align_of::<Timings>()),
];

/// The bound functions. Holding the `Library` keeps them valid.
pub struct Api {
    pub version: unsafe extern "C" fn() -> *const c_char,
    pub version_commit: unsafe extern "C" fn() -> *const c_char,
    pub status_string: unsafe extern "C" fn(c_int) -> *const c_char,
    pub abi_struct_size: unsafe extern "C" fn(c_int) -> usize,
    pub abi_struct_align: unsafe extern "C" fn(c_int) -> usize,
    pub init_backends: unsafe extern "C" fn(*const c_char) -> c_int,
    pub init_backends_default: unsafe extern "C" fn() -> c_int,
    pub device_count: unsafe extern "C" fn() -> c_int,
    pub device_get: unsafe extern "C" fn(c_int) -> Handle,
    pub device_info_init: unsafe extern "C" fn(*mut DeviceInfo),
    pub device_get_info: unsafe extern "C" fn(Handle, *mut DeviceInfo) -> c_int,
    pub backend_available: unsafe extern "C" fn(c_int) -> bool,
    pub log_set: unsafe extern "C" fn(LogCallback, *mut c_void),
    pub model_load_params_init: unsafe extern "C" fn(*mut ModelLoadParams),
    pub model_load_file: unsafe extern "C" fn(*const c_char, *const ModelLoadParams, *mut Handle) -> c_int,
    pub model_free: unsafe extern "C" fn(Handle),
    pub model_arch_string: unsafe extern "C" fn(Handle) -> *const c_char,
    pub model_variant_string: unsafe extern "C" fn(Handle) -> *const c_char,
    pub model_backend: unsafe extern "C" fn(Handle) -> *const c_char,
    pub model_device: unsafe extern "C" fn(Handle) -> Handle,
    pub model_supports: unsafe extern "C" fn(Handle, c_int) -> bool,
    pub model_accepts_ext_kind: unsafe extern "C" fn(Handle, c_int, u32) -> bool,
    pub session_params_init: unsafe extern "C" fn(*mut SessionParams),
    pub session_init: unsafe extern "C" fn(Handle, *const SessionParams, *mut Handle) -> c_int,
    pub session_free: unsafe extern "C" fn(Handle),
    pub set_abort_callback: unsafe extern "C" fn(Handle, AbortCallback, *mut c_void),
    pub was_aborted: unsafe extern "C" fn(Handle) -> bool,
    pub was_truncated: unsafe extern "C" fn(Handle) -> bool,
    pub run_params_init: unsafe extern "C" fn(*mut RunParams),
    pub run: unsafe extern "C" fn(Handle, *const f32, c_int, *const RunParams) -> c_int,
    pub full_text: unsafe extern "C" fn(Handle) -> *const c_char,
    pub timings_init: unsafe extern "C" fn(*mut Timings),
    pub get_timings: unsafe extern "C" fn(Handle, *mut Timings) -> c_int,
    pub whisper_run_ext_init: unsafe extern "C" fn(*mut WhisperRunExt),
    _library: libloading::Library,
}

// SAFETY: the table holds plain function pointers into a library that stays
// loaded for the table's lifetime; the C API is safe to call from any thread
// (one compute per model at a time, which the engine's worker guarantees).
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

impl Api {
    /// Loads the library and binds every function, failing on the first missing symbol.
    ///
    /// # Safety
    /// Loading runs the library's initialisers; the file must be a transcribe.cpp build.
    pub unsafe fn load(path: &Path) -> Result<Self, libloading::Error> {
        let library = unsafe { open(path)? };
        macro_rules! bind {
            ($name:literal) => {
                *unsafe { library.get($name)? }
            };
        }
        Ok(Self {
            version: bind!("transcribe_version"),
            version_commit: bind!("transcribe_version_commit"),
            status_string: bind!("transcribe_status_string"),
            abi_struct_size: bind!("transcribe_abi_struct_size"),
            abi_struct_align: bind!("transcribe_abi_struct_align"),
            init_backends: bind!("transcribe_init_backends"),
            init_backends_default: bind!("transcribe_init_backends_default"),
            device_count: bind!("transcribe_device_count"),
            device_get: bind!("transcribe_device_get"),
            device_info_init: bind!("transcribe_device_info_init"),
            device_get_info: bind!("transcribe_device_get_info"),
            backend_available: bind!("transcribe_backend_available"),
            log_set: bind!("transcribe_log_set"),
            model_load_params_init: bind!("transcribe_model_load_params_init"),
            model_load_file: bind!("transcribe_model_load_file"),
            model_free: bind!("transcribe_model_free"),
            model_arch_string: bind!("transcribe_model_arch_string"),
            model_variant_string: bind!("transcribe_model_variant_string"),
            model_backend: bind!("transcribe_model_backend"),
            model_device: bind!("transcribe_model_device"),
            model_supports: bind!("transcribe_model_supports"),
            model_accepts_ext_kind: bind!("transcribe_model_accepts_ext_kind"),
            session_params_init: bind!("transcribe_session_params_init"),
            session_init: bind!("transcribe_session_init"),
            session_free: bind!("transcribe_session_free"),
            set_abort_callback: bind!("transcribe_set_abort_callback"),
            was_aborted: bind!("transcribe_was_aborted"),
            was_truncated: bind!("transcribe_was_truncated"),
            run_params_init: bind!("transcribe_run_params_init"),
            run: bind!("transcribe_run"),
            full_text: bind!("transcribe_full_text"),
            timings_init: bind!("transcribe_timings_init"),
            get_timings: bind!("transcribe_get_timings"),
            whisper_run_ext_init: bind!("transcribe_whisper_run_ext_init"),
            _library: library,
        })
    }

    /// Native agreement: our layouts against the loaded library's. Returns the mismatches.
    pub fn verify_layouts(&self) -> Vec<String> {
        let mut mismatches = Vec::new();
        for &(name, id, size, align) in ABI_STRUCTS {
            // SAFETY: plain value-in, value-out call.
            let (native_size, native_align) = unsafe { ((self.abi_struct_size)(id), (self.abi_struct_align)(id)) };
            if native_size == 0 {
                mismatches.push(format!("{name}: native size 0 (loaded library is older than this binding)"));
                continue;
            }
            if size != native_size {
                mismatches.push(format!("{name}: size {size} != native {native_size}"));
            }
            if align != native_align {
                mismatches.push(format!("{name}: align {align} != native {native_align}"));
            }
        }
        mismatches
    }
}

/// Opens a library so that its own dependencies (ggml.dll, ggml-base.dll) are
/// found next to it, as koffi's loader does.
///
/// # Safety
/// Loading runs the library's initialisers (and unloading its terminators);
/// the file must be a library that is sound to load into this process.
#[cfg(windows)]
pub unsafe fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library};
    unsafe { Library::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH).map(Into::into) }
}

/// Opens a library.
///
/// # Safety
/// Loading runs the library's initialisers (and unloading its terminators);
/// the file must be a library that is sound to load into this process.
#[cfg(not(windows))]
pub unsafe fn open(path: &Path) -> Result<libloading::Library, libloading::Error> {
    unsafe { libloading::Library::new(path) }
}
