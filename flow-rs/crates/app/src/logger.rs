//! File logging. Logs never contain transcript text or audio unless the user
//! turns on debug logging; callers pass such content only through `log::debug!`.
//! Mirrors `src/main/log.ts`: `<ISO time> LEVEL [scope] message` in
//! `logs\flow.log`, rotated to `flow.log.1` past 1 MB.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

const MAX_BYTES: u64 = 1_000_000;

struct Sink {
    file: Option<File>,
    path: PathBuf,
    written: u64,
}

pub struct FlowLogger {
    sink: Mutex<Sink>,
    debug: AtomicBool,
    stdout: bool,
}

static LOGGER: std::sync::OnceLock<FlowLogger> = std::sync::OnceLock::new();

fn open(path: &Path) -> (Option<File>, u64) {
    let written = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let file = OpenOptions::new().create(true).append(true).open(path).ok();
    (file, written)
}

/// Opens `flow.log` in `dir` and installs the logger. Safe to call once per process.
pub fn init(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    let path = dir.join("flow.log");
    let (file, written) = open(&path);
    let logger = LOGGER.get_or_init(|| FlowLogger {
        sink: Mutex::new(Sink { file, path, written }),
        debug: AtomicBool::new(false),
        stdout: std::env::var("FLOW_LOG_STDOUT").as_deref() == Ok("1"),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

/// A logger for child processes: stderr only, which the main process collects.
pub fn init_stderr() {
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Info && !metadata.target().contains("::")
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                eprintln!("{}", flow_core::protocol::child_log_line(record.level(), record.target(), record.args()));
            }
        }
        fn flush(&self) {}
    }
    static STDERR: Stderr = Stderr;
    if log::set_logger(&STDERR).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

pub fn set_debug(enabled: bool) {
    if let Some(logger) = LOGGER.get() {
        logger.debug.store(enabled, Ordering::Relaxed);
    }
}

impl log::Log for FlowLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Flow's own scopes are single words; other crates log under module paths.
        if metadata.target().contains("::") {
            return metadata.level() <= log::Level::Warn && metadata.target().starts_with("flow");
        }
        metadata.level() <= log::Level::Info || self.debug.load(Ordering::Relaxed)
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => "ERROR",
            log::Level::Warn => "WARN ",
            log::Level::Info => "INFO ",
            log::Level::Debug | log::Level::Trace => "DEBUG",
        };
        let time = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let line = format!("{time} {level} [{}] {}\n", record.target(), record.args());
        if self.stdout {
            print!("{line}");
        }
        let mut sink = self.sink.lock();
        let Some(file) = sink.file.as_mut() else { return };
        if file.write_all(line.as_bytes()).is_err() {
            sink.file = None;
            return;
        }
        sink.written += line.len() as u64;
        if sink.written > MAX_BYTES {
            sink.file = None;
            let rotated = sink.path.with_extension("log.1");
            // Keep appending if the old file cannot be moved.
            let _ = fs::rename(&sink.path, rotated);
            let (file, written) = open(&sink.path.clone());
            sink.file = file;
            sink.written = written;
        }
    }

    fn flush(&self) {
        if let Some(file) = self.sink.lock().file.as_mut() {
            let _ = file.flush();
        }
    }
}
