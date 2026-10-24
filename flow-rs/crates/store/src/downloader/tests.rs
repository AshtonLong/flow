//! PORT: tests/unit/models/downloader.test.ts

use super::*;
use crate::test_support::{FakeServer, Request, ServedFile, random_bytes, sha256};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;

struct Setup {
    _dir: tempfile::TempDir,
    dest: PathBuf,
    part: PathBuf,
    server: Arc<FakeServer>,
    client: reqwest::Client,
    url: String,
    bytes: Vec<u8>,
    sha256: String,
}

async fn setup() -> Setup {
    let dir = tempfile::Builder::new().prefix("flow-models-").tempdir().unwrap();
    let dest = dir.path().join("nested").join("a.gguf");
    let part = with_suffix(&dest, ".part");
    let server = FakeServer::start().await;
    let client = server.client();
    let url = server.url("models.test", "/a.gguf");
    let bytes = random_bytes(50_000, 1);
    let sha256 = sha256(&bytes);
    Setup { _dir: dir, dest, part, server, client, url, bytes, sha256 }
}

impl Setup {
    fn options(&self) -> DownloadOptions<'_> {
        DownloadOptions {
            url: &self.url,
            dest_path: &self.dest,
            sha256: &self.sha256,
            size_bytes: self.bytes.len() as u64,
            http: &self.client,
            cancel: None,
            on_progress: None,
            stall_timeout: None,
        }
    }

    fn serve(&self, file: ServedFile) {
        self.server.serve(&self.url, file);
    }

    /// Leaves a partial download on disk, as an interrupted earlier attempt would.
    fn seed_part(&self, data: &[u8]) {
        std::fs::create_dir_all(self.part.parent().unwrap()).unwrap();
        std::fs::write(&self.part, data).unwrap();
    }

    fn request(&self, range: Option<&str>) -> Request {
        Request { url: self.url.clone(), range: range.map(str::to_string) }
    }
}

async fn hash_of(path: &Path) -> String {
    sha256_file(path, None).await.unwrap()
}

#[tokio::test]
async fn downloads_a_fresh_file_verifies_it_and_moves_it_into_place() {
    let s = setup().await;
    s.serve(ServedFile::new(s.bytes.clone()));
    let progress = Mutex::new(Vec::new());
    let total = s.bytes.len() as u64;
    let on_progress = |received: u64, of: u64| {
        assert_eq!(of, total);
        progress.lock().push(received);
    };
    download_file(DownloadOptions { on_progress: Some(&on_progress), ..s.options() }).await.unwrap();

    assert_eq!(std::fs::read(&s.dest).unwrap(), s.bytes);
    assert!(!s.part.exists());
    assert_eq!(s.server.requests(), [s.request(None)]);
    let progress = progress.into_inner();
    assert_eq!(progress[0], 0);
    assert_eq!(*progress.last().unwrap(), total);
    assert!(progress.windows(2).all(|w| w[0] <= w[1]));
    // Completion is reported exactly once, as the hand-off to verification.
    assert_eq!(progress.iter().filter(|&&p| p == total).count(), 1);
}

#[tokio::test]
async fn resumes_a_partial_file_with_a_range_request() {
    let s = setup().await;
    s.serve(ServedFile::new(s.bytes.clone()));
    s.seed_part(&s.bytes[..20_000]);

    let progress = Mutex::new(Vec::new());
    let on_progress = |received: u64, _: u64| progress.lock().push(received);
    download_file(DownloadOptions { on_progress: Some(&on_progress), ..s.options() }).await.unwrap();

    assert_eq!(s.server.requests(), [s.request(Some("bytes=20000-"))]);
    assert_eq!(progress.lock()[0], 20_000);
    assert_eq!(hash_of(&s.dest).await, s.sha256);
    assert!(!s.part.exists());
}

#[tokio::test]
async fn verifies_without_a_request_when_the_partial_file_is_already_complete() {
    let s = setup().await;
    s.serve(ServedFile::new(s.bytes.clone()));
    s.seed_part(&s.bytes);

    download_file(s.options()).await.unwrap();
    assert!(s.server.requests().is_empty());
    assert_eq!(hash_of(&s.dest).await, s.sha256);
}

#[tokio::test]
async fn starts_over_when_the_server_ignores_the_range_header() {
    let s = setup().await;
    s.serve(ServedFile { ignore_range: true, ..ServedFile::new(s.bytes.clone()) });
    s.seed_part(&s.bytes[..20_000]);

    download_file(s.options()).await.unwrap();
    assert_eq!(hash_of(&s.dest).await, s.sha256);
}

#[tokio::test]
async fn starts_over_when_the_server_answers_416() {
    let s = setup().await;
    s.serve(ServedFile { reject_range: true, ..ServedFile::new(s.bytes.clone()) });
    s.seed_part(&s.bytes[..20_000]);

    download_file(s.options()).await.unwrap();
    let ranges: Vec<Option<String>> = s.server.requests().into_iter().map(|r| r.range).collect();
    assert_eq!(ranges, [Some("bytes=20000-".to_string()), None]);
    assert_eq!(hash_of(&s.dest).await, s.sha256);
}

#[tokio::test]
async fn deletes_the_partial_file_and_throws_on_a_checksum_mismatch() {
    let s = setup().await;
    s.serve(ServedFile::new(random_bytes(s.bytes.len(), 99)));

    let error = download_file(s.options()).await.unwrap_err();
    assert!(matches!(error, ModelError::Checksum { .. }), "{error}");
    assert!(!s.part.exists());
    assert!(!s.dest.exists());
}

#[tokio::test]
async fn recovers_from_a_corrupt_partial_file_on_the_next_attempt() {
    let s = setup().await;
    s.serve(ServedFile::new(s.bytes.clone()));
    s.seed_part(&random_bytes(20_000, 7));

    let error = download_file(s.options()).await.unwrap_err();
    assert!(matches!(error, ModelError::Checksum { .. }), "{error}");
    assert!(!s.part.exists());

    download_file(s.options()).await.unwrap();
    assert_eq!(hash_of(&s.dest).await, s.sha256);
}

#[tokio::test]
async fn keeps_the_partial_file_when_cancelled_then_resumes_from_it() {
    let s = setup().await;
    s.serve(ServedFile::new(s.bytes.clone()));
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    s.server.on_chunk(Some(Arc::new(move |sent| {
        if sent >= 10_000 {
            token.cancel();
        }
    })));

    let error = download_file(DownloadOptions { cancel: Some(&cancel), ..s.options() }).await.unwrap_err();
    assert!(error.is_cancelled(), "{error}");
    assert!(!s.dest.exists());
    let kept = std::fs::metadata(&s.part).unwrap().len();
    assert!(kept > 0);
    assert!(kept < s.bytes.len() as u64);

    s.server.on_chunk(None);
    download_file(s.options()).await.unwrap();
    let range = format!("bytes={kept}-");
    assert_eq!(s.server.requests().last(), Some(&s.request(Some(&range))));
    assert_eq!(hash_of(&s.dest).await, s.sha256);
    assert!(!s.part.exists());
}

#[tokio::test]
async fn rejects_immediately_when_the_signal_is_already_aborted() {
    let s = setup().await;
    s.serve(ServedFile::new(s.bytes.clone()));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = download_file(DownloadOptions { cancel: Some(&cancel), ..s.options() }).await.unwrap_err();
    assert!(error.is_cancelled());
    assert!(s.server.requests().is_empty());
}

#[tokio::test]
async fn throws_on_a_non_200_response_and_leaves_nothing_behind() {
    let s = setup().await;
    let error = download_file(s.options()).await.unwrap_err();
    assert!(error.to_string().contains("HTTP 404"), "{error}");
    s.serve(ServedFile { status: Some(503), ..ServedFile::new(s.bytes.clone()) });
    let error = download_file(s.options()).await.unwrap_err();
    assert!(error.to_string().contains("HTTP 503"), "{error}");
    assert!(!s.part.exists());
    assert!(!s.dest.exists());
}

#[tokio::test]
async fn keeps_the_partial_file_when_the_connection_drops_then_resumes() {
    let s = setup().await;
    s.serve(ServedFile { drop_after: Some(12_000), ..ServedFile::new(s.bytes.clone()) });
    let error = download_file(s.options()).await.unwrap_err();
    assert!(error.to_string().contains("interrupted"), "{error}");
    assert_eq!(std::fs::metadata(&s.part).unwrap().len(), 12_000);

    s.serve(ServedFile::new(s.bytes.clone()));
    download_file(s.options()).await.unwrap();
    assert_eq!(s.server.requests().last().unwrap().range.as_deref(), Some("bytes=12000-"));
    assert_eq!(hash_of(&s.dest).await, s.sha256);
}

#[tokio::test]
async fn refuses_a_file_whose_size_differs_from_the_manifest() {
    let s = setup().await;
    s.serve(ServedFile::new(random_bytes(60_000, 1)));
    let error = download_file(s.options()).await.unwrap_err();
    assert_eq!(error.to_string(), "Download failed: the server's file is 60000 bytes, expected 50000");
    assert!(!s.part.exists());
}

#[tokio::test]
async fn gives_up_when_the_server_stops_sending_data() {
    let s = setup().await;
    s.serve(ServedFile { hang: true, ..ServedFile::new(s.bytes.clone()) });
    let error = download_file(DownloadOptions { stall_timeout: Some(Duration::from_millis(40)), ..s.options() })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Download stalled: no data for 0 s. Try again to resume.");
}

#[tokio::test]
async fn refuses_urls_that_are_not_https() {
    let s = setup().await;
    for url in ["http://models.test/a.gguf", "file:///c:/a.gguf", "not a url"] {
        let error = download_file(DownloadOptions { url, ..s.options() }).await.unwrap_err();
        assert!(error.to_string().contains("not https"), "{url}: {error}");
    }
    assert!(s.server.requests().is_empty());
}

#[tokio::test]
async fn reports_network_errors_with_a_code() {
    let s = setup().await;
    let client = crate::test_support::offline_client();
    let error = download_file(DownloadOptions { http: &client, ..s.options() }).await.unwrap_err();
    assert!(matches!(error, ModelError::Network(_)), "{error}");
    assert!(error.to_string().starts_with("Network error: "), "{error}");
}

#[test]
fn parses_content_range_headers() {
    assert_eq!(parse_content_range(Some("bytes 100-999/1000")), Some((100.0, Some(1000.0))));
    assert_eq!(parse_content_range(Some(" Bytes 0-9/* ")), Some((0.0, None)));
    assert_eq!(parse_content_range(Some("bytes 100-999")), None);
    assert_eq!(parse_content_range(None), None);
}
