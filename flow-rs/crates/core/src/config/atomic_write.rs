//! PORT: src/main/config/atomic-write.ts

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Errors worth retrying: on Windows the rename fails while an editor, indexer
/// or virus scanner has the target open (`EPERM`, `EBUSY`, `EACCES` in Node).
fn retryable(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::PermissionDenied | io::ErrorKind::ResourceBusy)
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        || matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// `<path>.tmp`
fn tmp_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".tmp");
    PathBuf::from(name)
}

/// Writes `data` to `<path>.tmp` and renames it over `path`, so a reader never
/// sees a half-written file and a crash never leaves a truncated one.
pub fn write_file_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_path(path);
    std::fs::write(&tmp, data)?;
    // Those handles are short-lived, so retry briefly.
    let mut attempt: u64 = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt >= 5 || !retryable(&error) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(error);
                }
                std::thread::sleep(Duration::from_millis(20 * (attempt + 1)));
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_replaces_without_leaving_a_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested").join("config.toml");
        write_file_atomic(&file, b"one").unwrap();
        write_file_atomic(&file, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert!(!tmp_path(&file).exists());
    }
}
