//! Native bootstrap: resolve → load → bind → verify ABI → version-gate → init
//! backends. Port of the bindings' `loader.js`, `cuda.js`, `abi.js` and
//! `native.js`, with Flow's environment variables in place of
//! `TRANSCRIBE_LIBRARY` / `TRANSCRIBE_CUDA_RUNTIME_DIR`. Lazy and memoised:
//! nothing loads until the first call, and a failed load is retried next time.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use flow_core::protocol::engine::env;
use parking_lot::Mutex;

use super::ffi::{self, Api, OUR_VERSION, TRANSCRIBE_LOG_LEVEL_CONT, TRANSCRIBE_OK};
use super::{ErrorKind, TcError};

#[cfg(windows)]
pub const LIB_NAME: &str = "transcribe.dll";
#[cfg(not(windows))]
pub const LIB_NAME: &str = "libtranscribe.so";

/// Where the library lives when `FLOW_TRANSCRIBE_LIBRARY` is not set: an
/// `engine` folder next to the executable.
pub const ENGINE_DIR: &str = "engine";

/// The loaded library: bound, verified and with its backends registered.
pub struct Native {
    pub api: Api,
    pub library_path: PathBuf,
    /// CUDA runtime libraries pinned for the process lifetime.
    _cuda: Vec<libloading::Library>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub library_path: PathBuf,
    /// The directory holding the library and its sibling ggml backend modules.
    pub artifact_dir: PathBuf,
}

/// `FLOW_TRANSCRIBE_LIBRARY` (an explicit path), else `<exe dir>\engine\transcribe.dll`.
pub fn resolve_library() -> Result<Resolved, TcError> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    resolve_from(std::env::var_os(env::LIBRARY).map(PathBuf::from), exe_dir)
}

fn resolve_from(over: Option<PathBuf>, exe_dir: Option<PathBuf>) -> Result<Resolved, TcError> {
    if let Some(path) = over.filter(|p| !p.as_os_str().is_empty()) {
        if !path.exists() {
            return Err(TcError::new(
                ErrorKind::Transcribe,
                format!("{} points at a missing file: {}", env::LIBRARY, path.display()),
            ));
        }
        let artifact_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        return Ok(Resolved { library_path: path, artifact_dir });
    }
    let dir = exe_dir.unwrap_or_default().join(ENGINE_DIR);
    let path = dir.join(LIB_NAME);
    if !path.exists() {
        return Err(TcError::new(
            ErrorKind::Backend,
            format!(
                "No transcribe.cpp native library found at {}. Set {} or place {LIB_NAME} and its ggml libraries there.",
                path.display(),
                env::LIBRARY
            ),
        ));
    }
    Ok(Resolved { library_path: path, artifact_dir: dir })
}

static NATIVE: Mutex<Option<Arc<Native>>> = Mutex::new(None);
static LOG_TO_STDERR: AtomicBool = AtomicBool::new(false);

/// Routes the native library's (and ggml's) diagnostics to stderr, or drops them.
/// Like the bindings' `setLogHandler`, this only swaps the target: the native
/// callback is installed once, during bootstrap.
pub fn set_log_to_stderr(enabled: bool) {
    LOG_TO_STDERR.store(enabled, Ordering::Relaxed);
}

/// The native callback. May fire from ggml worker threads; must never unwind.
unsafe extern "C" fn log_thunk(level: c_int, message: *const c_char, _user_data: *mut c_void) {
    if !LOG_TO_STDERR.load(Ordering::Relaxed) || message.is_null() {
        return;
    }
    // SAFETY: the library passes a NUL-terminated string valid for the call.
    let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    let text = text.trim_end();
    if text.is_empty() || (level == TRANSCRIBE_LOG_LEVEL_CONT && text == ".") {
        return;
    }
    let _ = writeln!(std::io::stderr(), "[transcribe] {text}");
}

/// Copies a C string the library owns; `None` for a null pointer.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated string that stays valid for
/// the call (the library's strings live as long as the object they describe).
pub unsafe fn c_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: guaranteed by the caller.
    Some(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

/// Leading dotted-numeric release segment, suffix stripped: "0.0.1.post3" → "0.0.1".
pub fn base_version(v: &str) -> &str {
    let v = v.trim();
    let mut end = 0;
    let mut expect_digit = true;
    for (i, ch) in v.char_indices() {
        if ch.is_ascii_digit() {
            expect_digit = false;
            end = i + 1;
        } else if ch == '.' && !expect_digit {
            expect_digit = true;
        } else {
            break;
        }
    }
    if end == 0 { v } else { &v[..end] }
}

/// The loaded library, bootstrapping it on first use.
pub fn native() -> Result<Arc<Native>, TcError> {
    let mut cached = NATIVE.lock();
    if let Some(native) = cached.as_ref() {
        return Ok(Arc::clone(native));
    }
    let resolved = resolve_library()?;
    // Bring-your-own CUDA runtime (the GPU pack): make cudart/cublas resolvable
    // BEFORE any load, so the ggml-cuda module's imports resolve when the
    // backends are registered below. A no-op without the variable.
    let cuda = std::env::var_os(env::CUDA_RUNTIME_DIR)
        .filter(|d| !d.is_empty())
        .map(|d| preload_cuda_runtime(Path::new(&d)))
        .unwrap_or_default();

    // SAFETY: loading the configured transcribe.cpp build.
    let api = unsafe { Api::load(&resolved.library_path) }.map_err(|e| {
        TcError::new(ErrorKind::Backend, format!("could not load {}: {e}", resolved.library_path.display()))
    })?;
    let mismatches = api.verify_layouts();
    if !mismatches.is_empty() {
        return Err(TcError::with_status(
            ErrorKind::Abi,
            format!(
                "transcribe_cpp ABI layout check failed — this is a version/build skew between the binding and the native library:\n  {}",
                mismatches.join("\n  ")
            ),
            ffi::TRANSCRIBE_ERR_BAD_STRUCT_SIZE,
        ));
    }
    // SAFETY: plain getter returning a static string.
    let native_version = unsafe { c_str((api.version)()) }.unwrap_or_default();
    if base_version(&native_version) != OUR_VERSION {
        return Err(TcError::new(
            ErrorKind::VersionMismatch,
            format!(
                "native library is version {native_version} but this binding is {OUR_VERSION} (pre-1.0 requires an exact base-version match)"
            ),
        ));
    }
    // transcribe_log_set is only supported as a startup-time install, before
    // backend init can create worker threads.
    // SAFETY: the thunk is a plain function that never unwinds.
    unsafe { (api.log_set)(Some(log_thunk), std::ptr::null_mut()) };

    // Register the backend modules next to the library (ggml-cpu-<isa>.dll,
    // ggml-vulkan.dll, ggml-cuda.dll): ggml scans the directory and picks the
    // best CPU variant for this processor.
    let dir = resolved.artifact_dir.to_string_lossy().into_owned();
    let dir_c = CString::new(dir.clone())
        .map_err(|_| TcError::new(ErrorKind::InvalidArgument, "library path contains a NUL character"))?;
    // SAFETY: a NUL-terminated path that outlives the call.
    let mut status = unsafe { (api.init_backends)(dir_c.as_ptr()) };
    if status != TRANSCRIBE_OK {
        // SAFETY: no arguments.
        status = unsafe { (api.init_backends_default)() };
    }
    if status != TRANSCRIBE_OK {
        let text = status_text(&api, status);
        return Err(TcError::with_status(
            ErrorKind::Backend,
            format!("transcribe_init_backends found no usable compute device in {dir}: {text} (status {status})"),
            status,
        ));
    }
    let native = Arc::new(Native { api, library_path: resolved.library_path, _cuda: cuda });
    *cached = Some(Arc::clone(&native));
    Ok(native)
}

pub fn status_text(api: &Api, status: c_int) -> String {
    // SAFETY: returns a static string for any status value.
    unsafe { c_str((api.status_string)(status)) }.unwrap_or_default()
}

/// Runtime libraries the ggml-cuda module links, in dependency order (cublasLt
/// before cublas): `[package, subdir, file]`. The package/subdir mirror the
/// `nvidia-*-cu12` wheel layout, so pointing the variable at a
/// `site-packages\nvidia` tree also works; a flat directory is tried first.
#[cfg(windows)]
const RUNTIME_LIBS: [[&str; 3]; 3] = [
    ["cuda_runtime", "bin", "cudart64_12.dll"],
    ["cublas", "bin", "cublasLt64_12.dll"],
    ["cublas", "bin", "cublas64_12.dll"],
];
#[cfg(not(windows))]
const RUNTIME_LIBS: [[&str; 3]; 3] = [
    ["cuda_runtime", "lib", "libcudart.so.12"],
    ["cublas", "lib", "libcublasLt.so.12"],
    ["cublas", "lib", "libcublas.so.12"],
];

/// Adds `dir` to the DLL search path and preloads the CUDA runtime from it, so
/// the ggml-cuda module's imports are satisfied by the already-loaded set.
/// Best-effort and silent: a missing library degrades like a missing driver,
/// and the Vulkan/CPU modules keep working.
fn preload_cuda_runtime(dir: &Path) -> Vec<libloading::Library> {
    add_dll_directory(dir);
    let mut pinned = Vec::new();
    for [package, sub, file] in RUNTIME_LIBS {
        let candidates = [dir.join(file), dir.join(package).join(sub).join(file)];
        let Some(found) = candidates.iter().find(|p| p.exists()) else { continue };
        // SAFETY: loading a vendor runtime library by absolute path.
        if let Ok(library) = unsafe { ffi::open(found) } {
            pinned.push(library);
        }
    }
    pinned
}

#[cfg(windows)]
fn add_dll_directory(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetDllDirectoryW(path: *const u16) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: a NUL-terminated wide string that outlives the call.
    unsafe { SetDllDirectoryW(wide.as_ptr()) };
}

#[cfg(not(windows))]
fn add_dll_directory(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_version_suffixes() {
        assert_eq!(base_version("0.3.0"), "0.3.0");
        assert_eq!(base_version(" 0.3.0.post3 "), "0.3.0");
        assert_eq!(base_version("0.3.0-dev+abc"), "0.3.0");
        assert_eq!(base_version("v1"), "v1");
    }

    #[test]
    fn prefers_the_explicit_library_path() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("transcribe.dll");
        std::fs::write(&lib, b"").unwrap();
        let resolved = resolve_from(Some(lib.clone()), Some(PathBuf::from("C:\\nowhere"))).unwrap();
        assert_eq!(resolved, Resolved { library_path: lib, artifact_dir: dir.path().to_path_buf() });
    }

    #[test]
    fn rejects_an_explicit_path_to_a_missing_file() {
        let err = resolve_from(Some(PathBuf::from("C:\\nowhere\\transcribe.dll")), None).unwrap_err();
        assert_eq!(err.message, "FLOW_TRANSCRIBE_LIBRARY points at a missing file: C:\\nowhere\\transcribe.dll");
    }

    #[test]
    fn falls_back_to_the_engine_folder_next_to_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let engine = dir.path().join(ENGINE_DIR);
        std::fs::create_dir(&engine).unwrap();
        std::fs::write(engine.join(LIB_NAME), b"").unwrap();
        // An empty variable counts as unset.
        let resolved = resolve_from(Some(PathBuf::new()), Some(dir.path().to_path_buf())).unwrap();
        assert_eq!(resolved, Resolved { library_path: engine.join(LIB_NAME), artifact_dir: engine });
        let missing = resolve_from(None, Some(dir.path().join("elsewhere"))).unwrap_err();
        assert_eq!(missing.kind, ErrorKind::Backend);
        assert!(missing.message.starts_with("No transcribe.cpp native library found at "));
    }
}
