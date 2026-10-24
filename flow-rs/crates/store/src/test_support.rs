//! Test doubles for the model manager: a local HTTPS server with Range support,
//! and catalog builders.
//! PORT: tests/unit/models/helpers.ts
//!
//! The TS tests inject a fake `fetch`; these serve the same files over real HTTPS
//! (a self-signed certificate for `models.test`, `catalog.test` and `evil.test`,
//! all resolved to 127.0.0.1), so the production reqwest path is what is tested.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer as _, SigningKey};
use flow_core::catalog::{Catalog, CloudModelEntry, LocalModelEntry, ProviderApi, ProviderEntry, RunsOn, Tier};
use parking_lot::Mutex;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

pub const HOSTS: [&str; 3] = ["models.test", "catalog.test", "evil.test"];

#[derive(Debug, Clone, Default)]
pub struct ServedFile {
    pub bytes: Vec<u8>,
    /// Respond with this status instead of serving the file.
    pub status: Option<u16>,
    /// Ignore Range headers and always send the whole file with 200.
    pub ignore_range: bool,
    /// Answer every Range request with 416.
    pub reject_range: bool,
    /// Close the connection after this many body bytes of a response.
    pub drop_after: Option<usize>,
    /// Open the response but never send a byte.
    pub hang: bool,
}

impl ServedFile {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self { bytes: bytes.into(), ..Self::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub range: Option<String>,
}

type OnChunk = Arc<dyn Fn(usize) + Send + Sync>;

/// Serves `files` by URL in small chunks, honouring Range, over HTTPS on 127.0.0.1.
pub struct FakeServer {
    addr: SocketAddr,
    files: Mutex<HashMap<String, ServedFile>>,
    /// Every request made, in order.
    requests: Mutex<Vec<Request>>,
    /// Called after each body chunk is handed to the client, with the bytes sent so far.
    on_chunk: Mutex<Option<OnChunk>>,
    chunk_size: usize,
}

fn tls_config() -> rustls::ServerConfig {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let cert = CertificateDer::from_pem_slice(include_bytes!("../testdata/cert.pem")).expect("test certificate");
    let key = PrivateKeyDer::from_pem_slice(include_bytes!("../testdata/key.pem")).expect("test key");
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config")
}

impl FakeServer {
    pub async fn start() -> Arc<Self> {
        Self::start_with_chunk_size(1024).await
    }

    pub async fn start_with_chunk_size(chunk_size: usize) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let server = Arc::new(Self {
            addr: listener.local_addr().expect("address"),
            files: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            on_chunk: Mutex::new(None),
            chunk_size,
        });
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config()));
        let weak = Arc::downgrade(&server);
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let Some(server) = weak.upgrade() else { break };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(stream) = acceptor.accept(socket).await {
                        server.handle(stream).await;
                    }
                });
            }
        });
        server
    }

    /// `https://<host>:<port><path>`.
    pub fn url(&self, host: &str, path: &str) -> String {
        format!("https://{host}:{}{path}", self.addr.port())
    }

    /// A client that reaches this server for the test host names and trusts its certificate.
    pub fn client(&self) -> reqwest::Client {
        let mut builder = reqwest::Client::builder().danger_accept_invalid_certs(true).no_proxy();
        for host in HOSTS {
            builder = builder.resolve(host, self.addr);
        }
        builder.build().expect("client")
    }

    pub fn serve(&self, url: &str, file: ServedFile) {
        self.files.lock().insert(url.to_string(), file);
    }

    pub fn unserve(&self, url: &str) {
        self.files.lock().remove(url);
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().clone()
    }

    pub fn on_chunk(&self, callback: Option<OnChunk>) {
        *self.on_chunk.lock() = callback;
    }

    async fn handle<S>(&self, mut stream: S)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let mut head = Vec::new();
        let mut buffer = [0u8; 4096];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(read) => head.extend_from_slice(&buffer[..read]),
            }
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        let mut lines = head.split("\r\n");
        let path = lines.next().and_then(|line| line.split(' ').nth(1)).unwrap_or("/").to_string();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        let host = headers.get("host").cloned().unwrap_or_default();
        let url = format!("https://{host}{path}");
        let range = headers.get("range").cloned();
        self.requests.lock().push(Request { url: url.clone(), range: range.clone() });

        let Some(file) = self.files.lock().get(&url).cloned() else {
            return respond(&mut stream, "404 Not Found", &[], b"not found").await;
        };
        if let Some(status) = file.status {
            return respond(&mut stream, &status.to_string(), &[], b"nope").await;
        }
        if range.is_some() && file.reject_range {
            return respond(&mut stream, "416 Range Not Satisfiable", &[], b"").await;
        }

        let len = file.bytes.len();
        let mut start = 0;
        let mut status = "200 OK";
        let mut extra = Vec::new();
        let requested = range
            .as_deref()
            .filter(|_| !file.ignore_range)
            .and_then(|r| r.strip_prefix("bytes=")?.strip_suffix('-')?.parse::<usize>().ok());
        if let Some(from) = requested {
            if from >= len {
                return respond(&mut stream, "416 Range Not Satisfiable", &[], b"").await;
            }
            start = from;
            status = "206 Partial Content";
            extra.push(format!("Content-Range: bytes {start}-{}/{len}", len - 1));
        }
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
            len - start,
            extra.iter().map(|h| format!("{h}\r\n")).collect::<String>()
        );
        if stream.write_all(head.as_bytes()).await.is_err() || stream.flush().await.is_err() {
            return;
        }
        if file.hang {
            std::future::pending::<()>().await;
        }

        let end = len.min(start.saturating_add(file.drop_after.unwrap_or(usize::MAX)));
        let mut offset = start;
        while offset < end {
            // Pace the chunks so the client keeps up, as a pulled stream would.
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            let chunk = &file.bytes[offset..end.min(offset + self.chunk_size)];
            if stream.write_all(chunk).await.is_err() || stream.flush().await.is_err() {
                return; // the client went away
            }
            offset += chunk.len();
            let callback = self.on_chunk.lock().clone();
            if let Some(callback) = callback {
                callback(offset - start);
            }
        }
        let _ = stream.shutdown().await;
    }
}

async fn respond<S>(stream: &mut S, status: &str, headers: &[String], body: &[u8])
where
    S: tokio::io::AsyncWrite + Unpin,
{
    let extra: String = headers.iter().map(|h| format!("{h}\r\n")).collect();
    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.shutdown().await;
}

/// A client whose every lookup fails at once, like being offline.
pub fn offline_client() -> reqwest::Client {
    struct Offline;
    impl reqwest::dns::Resolve for Offline {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async { Err("getaddrinfo ENOTFOUND".into()) })
        }
    }
    reqwest::Client::builder().dns_resolver(Offline).no_proxy().build().expect("client")
}

/// Deterministic pseudo-random bytes.
pub fn random_bytes(length: usize, seed: u32) -> Vec<u8> {
    let mut state = if seed == 0 { 1 } else { seed };
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    crate::downloader::hex(&Sha256::digest(bytes))
}

pub struct TestKey {
    pub public_pem: String,
    private_key: SigningKey,
}

/// A fresh key pair; seeds come from a counter, so every call differs.
pub fn test_key() -> TestKey {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let seed: [u8; 32] = Sha256::digest(format!("flow-store test key {n}")).into();
    let private_key = SigningKey::from_bytes(&seed);
    let mut der = vec![0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    der.extend_from_slice(private_key.verifying_key().as_bytes());
    let public_pem = format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", STANDARD.encode(der));
    TestKey { public_pem, private_key }
}

pub fn sign_bytes(bytes: &[u8], key: &TestKey) -> String {
    STANDARD.encode(key.private_key.sign(bytes).to_bytes())
}

/// A catalog entry for `bytes`, downloadable from `base` (`https://models.test:<port>`).
pub fn local_entry(base: &str, id: &str, bytes: &[u8]) -> LocalModelEntry {
    LocalModelEntry {
        id: id.into(),
        name: format!("Model {id}"),
        family: "test".into(),
        description: "A test model".into(),
        url: format!("{base}/{id}.gguf"),
        file: format!("{id}.gguf"),
        sha256: sha256(bytes),
        size_bytes: bytes.len() as u64,
        languages: vec!["en".into()],
        licence: "MIT".into(),
        licence_url: None,
        runs_on: RunsOn::Cpu,
        tier: Tier::Core,
        wer: None,
        gated: false,
        note: None,
    }
}

fn provider(id: &str, name: &str, base_url: &str, keyless: bool) -> ProviderEntry {
    ProviderEntry {
        id: id.into(),
        name: name.into(),
        api: ProviderApi::Openai,
        base_url: base_url.into(),
        key_url: None,
        keyless,
    }
}

fn cloud(id: &str, provider: &str, model: &str, name: &str, description: &str, price: f64) -> CloudModelEntry {
    CloudModelEntry {
        id: id.into(),
        provider: provider.into(),
        model: model.into(),
        name: name.into(),
        description: description.into(),
        price_per_hour: price,
        recommended: false,
        hints: None,
        min_billed_seconds: None,
        docs_url: None,
    }
}

pub fn test_catalog(local: Vec<LocalModelEntry>, as_of: &str) -> Catalog {
    Catalog {
        version: 1,
        as_of: as_of.into(),
        providers: vec![
            provider("groq", "Groq", "https://api.groq.com/openai/v1", false),
            provider("openai", "OpenAI", "https://api.openai.com/v1", false),
            provider("ollama", "Ollama (local)", "http://localhost:11434/v1", true),
        ],
        local,
        cloud: vec![
            cloud(
                "groq/whisper-large-v3-turbo",
                "groq",
                "whisper-large-v3-turbo",
                "Whisper large-v3-turbo",
                "Lowest price",
                0.04,
            ),
            cloud("openai/gpt-4o-transcribe", "openai", "gpt-4o-transcribe", "gpt-4o-transcribe", "OpenAI", 0.36),
            cloud("ollama/whisper", "ollama", "whisper", "Local Whisper", "Keyless", 0.0),
        ],
        cleanup: vec![],
    }
}

/// The manifest as JSON, each entry's `kind` tag first, as the TS writes it.
pub fn catalog_json(catalog: &Catalog) -> Value {
    let mut value = serde_json::to_value(catalog).expect("catalog serialises");
    for list in ["local", "cloud", "cleanup"] {
        if let Some(entries) = value.get_mut(list).and_then(Value::as_array_mut) {
            for entry in entries {
                let mut tagged = serde_json::Map::new();
                tagged.insert("kind".into(), Value::from(list));
                if let Value::Object(fields) = entry.take() {
                    tagged.extend(fields);
                }
                *entry = Value::Object(tagged);
            }
        }
    }
    value
}

/// `JSON.stringify(value, null, 2) + "\n"`.
pub fn encode_value(value: &Value) -> Vec<u8> {
    format!("{}\n", serde_json::to_string_pretty(value).expect("json")).into_bytes()
}

pub fn encode(catalog: &Catalog) -> Vec<u8> {
    encode_value(&catalog_json(catalog))
}
