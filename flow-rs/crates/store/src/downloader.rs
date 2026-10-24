//! Resumable, checksum-verified file download. The HTTP client is injected.
//! PORT: src/main/models/downloader.ts

use std::error::Error as _;
use std::io::{self, Read as _};
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use flow_core::catalog::is_https_url;
use reqwest::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use reqwest::{Response, StatusCode};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncWriteExt as _, BufWriter};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::fs_util::{file_size, remove_file_force, with_suffix};

/// Give up when no data arrives for this long, unless the caller says otherwise.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Why a download (or another model-manager operation) failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// The download was cancelled (the TS `AbortError`). The partial file is kept,
    /// so the next download resumes it.
    #[error("Download cancelled")]
    Cancelled,
    /// The finished file did not match the expected SHA-256. The partial file has been deleted.
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    /// The request failed below HTTP (DNS, refused, reset, TLS): what `fetch` reports as
    /// `TypeError: fetch failed`. Holds the Node-style code (`ENOTFOUND`, `ECONNRESET`)
    /// when one applies, else the underlying cause's message.
    #[error("Network error: {0}")]
    Network(String),
    /// Anything else, with its message.
    #[error("{0}")]
    Failed(String),
}

impl ModelError {
    /// True for the error a cancelled download fails with (`isAbortError`).
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

pub(crate) fn io_error(error: io::Error) -> ModelError {
    ModelError::Failed(error.to_string())
}

/// What to download, where to, and how to report it.
pub struct DownloadOptions<'a> {
    /// Must be `https:`.
    pub url: &'a str,
    pub dest_path: &'a Path,
    /// Expected SHA-256 of the complete file, hex.
    pub sha256: &'a str,
    /// Expected size of the complete file.
    pub size_bytes: u64,
    pub http: &'a reqwest::Client,
    pub cancel: Option<&'a CancellationToken>,
    /// Called as bytes arrive with `(received, total)`. `received == total` means
    /// verification has started.
    pub on_progress: Option<&'a (dyn Fn(u64, u64) + Send + Sync)>,
    /// Give up when no data arrives for this long. The partial file is kept.
    /// Default [`DEFAULT_STALL_TIMEOUT`].
    pub stall_timeout: Option<Duration>,
}

fn is_cancelled(cancel: Option<&CancellationToken>) -> bool {
    cancel.is_some_and(CancellationToken::is_cancelled)
}

/// Streams a file through SHA-256 and returns the lowercase hex digest. Runs on a
/// blocking thread; checks `cancel` between 1 MiB reads.
pub async fn sha256_file(path: &Path, cancel: Option<&CancellationToken>) -> Result<String, ModelError> {
    let path = path.to_path_buf();
    let cancel = cancel.cloned();
    let task = tokio::task::spawn_blocking(move || -> Result<String, ModelError> {
        let mut file = std::fs::File::open(&path).map_err(io_error)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let read = match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(io_error(error)),
            };
            if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
                return Err(ModelError::Cancelled);
            }
            hash.update(&buffer[..read]);
        }
        Ok(hex(&hash.finalize()))
    });
    task.await.map_err(|error| ModelError::Failed(error.to_string()))?
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// The start offset and total length from a `Content-Range: bytes 100-999/1000` header.
fn parse_content_range(header: Option<&str>) -> Option<(f64, Option<f64>)> {
    static PATTERN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)^bytes\s+(\d+)-\d+/(\d+|\*)$").expect("valid pattern"));
    let captures = PATTERN.captures(header.unwrap_or("").trim())?;
    let start = js_number(&captures[1]);
    let total = (&captures[2] != "*").then(|| js_number(&captures[2]));
    Some((start, total))
}

/// `Number(text)` for the header values above (digits, or garbage → NaN).
fn js_number(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// A number as JavaScript prints it in a template string (integers without `.0`).
fn js_display(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.fract() == 0.0 && value.abs() < 1e21 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

/// Downloads to `<dest_path>.part`, resuming with a Range request if a partial file exists,
/// verifies SHA-256 (streamed), then renames into place. A checksum mismatch deletes the
/// partial file and fails with [`ModelError::Checksum`]. Cancelling keeps the partial file
/// and fails with [`ModelError::Cancelled`].
pub async fn download_file(options: DownloadOptions<'_>) -> Result<(), ModelError> {
    let options = &options;
    if !is_https_url(options.url) {
        return Err(ModelError::Failed("Refusing to download from a URL that is not https".into()));
    }
    if is_cancelled(options.cancel) {
        return Err(ModelError::Cancelled);
    }

    let part_path = with_suffix(options.dest_path, ".part");
    if let Some(parent) = options.dest_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
    }

    let size_bytes = options.size_bytes;
    let mut have = file_size(&part_path).await;
    if have > size_bytes {
        // Left over from a different file; it cannot be a prefix of this one.
        remove_file_force(&part_path).await.map_err(io_error)?;
        have = 0;
    }
    if have < size_bytes {
        fetch_remainder(options, &part_path, have).await?;
    }

    if is_cancelled(options.cancel) {
        return Err(ModelError::Cancelled);
    }
    if let Some(on_progress) = options.on_progress {
        on_progress(size_bytes, size_bytes);
    }
    let expected = options.sha256.to_lowercase();
    let actual = sha256_file(&part_path, options.cancel).await?;
    if actual != expected {
        remove_file_force(&part_path).await.map_err(io_error)?;
        return Err(ModelError::Checksum { expected, actual });
    }
    tokio::fs::rename(&part_path, options.dest_path).await.map_err(io_error)
}

/// Why a guarded wait ended early.
enum Stop {
    Cancelled,
    Stalled,
}

enum Failure {
    Stop(Stop),
    Error(ModelError),
}

impl From<Stop> for Failure {
    fn from(stop: Stop) -> Self {
        Self::Stop(stop)
    }
}

impl From<ModelError> for Failure {
    fn from(error: ModelError) -> Self {
        Self::Error(error)
    }
}

/// One timer serves both the caller's cancel and the stall timeout.
struct Watch<'a> {
    cancel: Option<&'a CancellationToken>,
    stall: Duration,
    deadline: Instant,
}

impl Watch<'_> {
    fn arm(&mut self) {
        self.deadline = Instant::now() + self.stall;
    }

    async fn run<F: Future>(&self, future: F) -> Result<F::Output, Stop> {
        let cancelled = async {
            match self.cancel {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            () = cancelled => Err(Stop::Cancelled),
            () = tokio::time::sleep_until(self.deadline) => Err(Stop::Stalled),
            output = future => Ok(output),
        }
    }
}

/// Fetches bytes `have..size_bytes` into the partial file, restarting from zero if the server insists.
async fn fetch_remainder(options: &DownloadOptions<'_>, part_path: &Path, have: u64) -> Result<(), ModelError> {
    let stall = options.stall_timeout.unwrap_or(DEFAULT_STALL_TIMEOUT);
    let mut watch = Watch { cancel: options.cancel, stall, deadline: Instant::now() + stall };
    match transfer(options, part_path, have, &mut watch).await {
        Ok(()) => Ok(()),
        Err(_) if is_cancelled(options.cancel) => Err(ModelError::Cancelled),
        Err(Failure::Stop(Stop::Stalled)) => Err(ModelError::Failed(format!(
            "Download stalled: no data for {} s. Try again to resume.",
            js_display(stall.as_secs_f64().round())
        ))),
        Err(Failure::Stop(Stop::Cancelled)) => Err(ModelError::Cancelled),
        Err(Failure::Error(error)) => Err(error),
    }
}

async fn transfer(
    options: &DownloadOptions<'_>,
    part_path: &Path,
    mut have: u64,
    watch: &mut Watch<'_>,
) -> Result<(), Failure> {
    let size_bytes = options.size_bytes;
    let request = |from: u64| {
        let mut builder = options.http.get(options.url);
        if from > 0 {
            builder = builder.header(RANGE, format!("bytes={from}-"));
        }
        builder.send()
    };

    watch.arm();
    let mut response = watch.run(request(have)).await?.map_err(|e| network_error(&e))?;
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
        // The server has nothing at that offset, so the partial file is not a prefix of its file.
        drop(response);
        have = 0;
        watch.arm();
        response = watch.run(request(0)).await?.map_err(|e| network_error(&e))?;
    }

    let status = response.status();
    let offset: u64;
    let mut declared: Option<f64> = None;
    if status == StatusCode::PARTIAL_CONTENT {
        let range = parse_content_range(header(&response, CONTENT_RANGE));
        match range {
            Some((start, total)) if start == have as f64 => {
                offset = have;
                declared = total;
            }
            _ => {
                return Err(ModelError::Failed(
                    "Download failed: the server sent a different range than requested".into(),
                )
                .into());
            }
        }
    } else if status == StatusCode::OK {
        // No Range support (or a fresh request): the body is the whole file.
        offset = 0;
        if let Some(length) = header(&response, CONTENT_LENGTH)
            && header(&response, CONTENT_ENCODING).is_none()
        {
            declared = Some(js_number(length));
        }
    } else {
        drop(response);
        let text = format!("Download failed: HTTP {} {}", status.as_u16(), status.canonical_reason().unwrap_or(""));
        return Err(ModelError::Failed(text.trim().to_string()).into());
    }

    if let Some(declared) = declared
        && declared != size_bytes as f64
    {
        drop(response);
        remove_file_force(part_path).await.map_err(io_error)?;
        return Err(ModelError::Failed(format!(
            "Download failed: the server's file is {} bytes, expected {size_bytes}",
            js_display(declared)
        ))
        .into());
    }

    let file = if offset > 0 {
        tokio::fs::OpenOptions::new().append(true).create(true).open(part_path).await
    } else {
        tokio::fs::File::create(part_path).await
    }
    .map_err(io_error)?;
    let mut writer = BufWriter::with_capacity(256 * 1024, file);
    let mut received = offset;
    let mut overflow = false;
    let outcome: Result<(), Failure> = async {
        if let Some(on_progress) = options.on_progress {
            on_progress(received, size_bytes);
        }
        loop {
            let chunk = match watch.run(response.chunk()).await? {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                // A reset is a network error; a body cut short is left to the length check below.
                Err(error) => match network_code(&error) {
                    Some(code) => return Err(ModelError::Network(code).into()),
                    None => break,
                },
            };
            watch.arm();
            received += chunk.len() as u64;
            if received > size_bytes {
                overflow = true;
                break;
            }
            writer.write_all(&chunk).await.map_err(io_error)?;
            // The final callback is left to the caller, which uses it to mark verification.
            if received < size_bytes
                && let Some(on_progress) = options.on_progress
            {
                on_progress(received, size_bytes);
            }
        }
        Ok(())
    }
    .await;
    // Whatever happened, what arrived is on disk before anyone looks at the partial file.
    let flushed = writer.shutdown().await;
    drop(writer);
    drop(response);
    outcome?;
    flushed.map_err(io_error)?;

    if overflow {
        remove_file_force(part_path).await.map_err(io_error)?;
        return Err(ModelError::Failed(format!(
            "Download failed: the server sent more than the expected {size_bytes} bytes"
        ))
        .into());
    }
    if received < size_bytes {
        return Err(ModelError::Failed(
            "Download interrupted: the connection closed early. Try again to resume.".into(),
        )
        .into());
    }
    Ok(())
}

fn header(response: &Response, name: reqwest::header::HeaderName) -> Option<&str> {
    response.headers().get(name).and_then(|value| value.to_str().ok())
}

/// The Node-style code for a socket error (`ECONNRESET`, `ENOTFOUND`, ...), if the
/// error chain holds one.
fn network_code(error: &reqwest::Error) -> Option<String> {
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<io::Error>()
            && let Some(code) = io_code(io)
        {
            return Some(code.to_string());
        }
        source = cause.source();
    }
    None
}

fn io_code(error: &io::Error) -> Option<&'static str> {
    // Winsock codes first: DNS failures carry no useful `ErrorKind`.
    let code = match error.raw_os_error() {
        Some(11001 | 11004) => "ENOTFOUND",
        Some(11002) => "EAI_AGAIN",
        Some(10050) => "ENETDOWN",
        Some(10051) => "ENETUNREACH",
        Some(10053) => "ECONNABORTED",
        Some(10054) => "ECONNRESET",
        Some(10060) => "ETIMEDOUT",
        Some(10061) => "ECONNREFUSED",
        Some(10065) => "EHOSTUNREACH",
        _ => match error.kind() {
            io::ErrorKind::ConnectionReset => "ECONNRESET",
            io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
            io::ErrorKind::ConnectionAborted => "ECONNABORTED",
            io::ErrorKind::TimedOut => "ETIMEDOUT",
            io::ErrorKind::HostUnreachable => "EHOSTUNREACH",
            io::ErrorKind::NetworkUnreachable => "ENETUNREACH",
            io::ErrorKind::NetworkDown => "ENETDOWN",
            _ => return None,
        },
    };
    Some(code)
}

/// A failed request as [`ModelError::Network`]: the socket error's code, else the
/// innermost cause's message.
pub(crate) fn network_error(error: &reqwest::Error) -> ModelError {
    if let Some(code) = network_code(error) {
        return ModelError::Network(code);
    }
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message = cause.to_string();
        source = cause.source();
    }
    ModelError::Network(message)
}

#[cfg(test)]
mod tests;
