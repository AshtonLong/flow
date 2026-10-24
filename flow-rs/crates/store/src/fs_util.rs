//! Small file helpers shared by the stores.
//! PORT: src/main/config/atomic-write.ts (`write_file_atomic`), plus the Node
//! `fs` behaviours the TS relies on (`stat().size`, `mtimeMs`, `rm({ force })`,
//! `path.resolve`).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// `<path><suffix>`, e.g. `model.gguf` → `model.gguf.part`.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = OsString::from(path.as_os_str());
    text.push(suffix);
    PathBuf::from(text)
}

/// Windows errors that Node reports as `EPERM`, `EBUSY` or `EACCES`: a handle held
/// briefly by an editor, an indexer or a virus scanner.
fn is_retryable(error: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
    matches!(error.raw_os_error(), Some(5 | 32 | 33)) || error.kind() == io::ErrorKind::PermissionDenied
}

/// True for the errors Node calls `EBUSY` or `EPERM` (the file is open elsewhere).
pub(crate) fn is_busy(error: &io::Error) -> bool {
    is_retryable(error)
}

/// Writes `data` to `<path>.tmp` and renames it over `path`, so a reader never
/// sees a half-written file and a crash never leaves a truncated one.
pub(crate) fn write_file_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let tmp = with_suffix(path, ".tmp");
    fs::write(&tmp, data)?;
    // On Windows the rename fails while an editor, indexer or virus scanner has
    // the target open. Those handles are short-lived, so retry briefly.
    let mut attempt: u64 = 0;
    loop {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt >= 5 || !is_retryable(&error) {
                    let _ = fs::remove_file(&tmp);
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(20 * (attempt + 1)));
                attempt += 1;
            }
        }
    }
}

/// Size of a file in bytes, or 0 if it does not exist.
pub(crate) async fn file_size(path: &Path) -> u64 {
    tokio::fs::metadata(path).await.map(|m| m.len()).unwrap_or(0)
}

/// Deletes a file; a missing file is not an error (`rm(path, { force: true })`).
pub(crate) async fn remove_file_force(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Modification time in epoch milliseconds, computed exactly as Node computes
/// `stats.mtimeMs` (`sec * 1e3 + nsec / 1e6` in doubles), so records written by
/// the Electron build compare equal.
pub(crate) fn mtime_ms(metadata: &fs::Metadata) -> f64 {
    match metadata.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
        Some(since) => since.as_secs() as f64 * 1e3 + f64::from(since.subsec_nanos()) / 1e6,
        None => 0.0,
    }
}

/// `path.resolve(base, path)`: an absolute `path` stands alone, a relative one is
/// joined to `base`, and `.` and `..` segments are folded away.
pub(crate) fn resolve_path(base: &Path, path: &str) -> PathBuf {
    let joined = base.join(path);
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => out.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                // `pop` refuses to remove the root, as `path.resolve` does.
                out.pop();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_like_node() {
        let base = Path::new(r"C:\Users\me\models");
        assert_eq!(resolve_path(base, "a.gguf"), PathBuf::from(r"C:\Users\me\models\a.gguf"));
        assert_eq!(resolve_path(base, r"C:\x\mine.gguf"), PathBuf::from(r"C:\x\mine.gguf"));
        assert_eq!(resolve_path(base, "../b/./c.gguf"), PathBuf::from(r"C:\Users\me\b\c.gguf"));
        assert_eq!(resolve_path(base, "sub/d.gguf"), PathBuf::from(r"C:\Users\me\models\sub\d.gguf"));
    }

    #[test]
    fn writes_atomically_and_creates_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("file.json");
        write_file_atomic(&path, b"one").unwrap();
        write_file_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!with_suffix(&path, ".tmp").exists());
    }
}
