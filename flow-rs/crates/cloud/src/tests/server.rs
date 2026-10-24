//! A tiny HTTP/1.1 server for the adapter tests, standing in for the injected
//! `fetch` of the TS tests. It runs on its own threads (blocking I/O), records
//! every request, answers with scripted replies, and can hang until the client
//! gives up, so cancels and timeouts are observable as closed connections.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long a test waits for the server to see something before failing.
const WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub enum Reply {
    Respond { status: u16, body: String, content_type: &'static str },
    /// Never answers; waits until the client closes the connection.
    Hang,
    /// Closes the connection without answering, like a failed `fetch`.
    Fail,
}

impl Reply {
    pub fn json(json: Value) -> Self {
        Self::status_json(200, json)
    }
    pub fn status_json(status: u16, json: Value) -> Self {
        Self::Respond { status, body: json.to_string(), content_type: "application/json" }
    }
    /// A status with an empty JSON object, like `{ status: 403 }` in the TS tests.
    pub fn status(status: u16) -> Self {
        Self::status_json(status, json!({}))
    }
    pub fn text(text: &str) -> Self {
        Self::status_text(200, text)
    }
    pub fn status_text(status: u16, text: &str) -> Self {
        Self::Respond { status, body: text.to_string(), content_type: "text/plain;charset=UTF-8" }
    }
}

/// One request as the server received it.
#[derive(Debug, Clone)]
pub struct Call {
    pub method: String,
    /// Path and query, e.g. `/deepgram/v1/listen?model=nova-3`.
    pub target: String,
    /// Names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Headers the transport adds by itself; the TS tests only see what the adapter set.
const TRANSPORT_HEADERS: [&str; 7] =
    ["host", "content-length", "accept", "accept-encoding", "user-agent", "connection", "transfer-encoding"];

impl Call {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }

    /// The headers the adapter set (what `init.headers` holds in the TS tests): no
    /// transport headers, and no multipart content type (`fetch` adds that for a FormData).
    pub fn sent_headers(&self) -> BTreeMap<String, String> {
        self.headers
            .iter()
            .filter(|(name, value)| {
                !TRANSPORT_HEADERS.contains(&name.as_str())
                    && !(name == "content-type" && value.starts_with("multipart/form-data"))
            })
            .cloned()
            .collect()
    }

    pub fn query(&self) -> Vec<(String, String)> {
        let query = self.target.split_once('?').map(|(_, q)| q).unwrap_or_default();
        url::form_urlencoded::parse(query.as_bytes()).into_owned().collect()
    }

    pub fn query_all(&self, key: &str) -> Vec<String> {
        self.query().into_iter().filter(|(k, _)| k == key).map(|(_, v)| v).collect()
    }

    pub fn query_get(&self, key: &str) -> Option<String> {
        self.query_all(key).into_iter().next()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }

    pub fn form(&self) -> Form {
        let content_type = self.header("content-type").expect("a content type");
        assert!(content_type.starts_with("multipart/form-data"), "not a form: {content_type}");
        let boundary = content_type.split("boundary=").nth(1).expect("a boundary").trim_matches('"');
        Form::parse(&self.body, boundary)
    }
}

/// Expected headers, names lowercased.
pub fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(n, v)| (n.to_ascii_lowercase(), v.to_string())).collect()
}

#[derive(Debug, Clone)]
pub struct FormPart {
    pub name: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
}

/// A parsed `multipart/form-data` body, read like a `FormData`.
#[derive(Debug, Clone)]
pub struct Form {
    pub parts: Vec<FormPart>,
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

fn disposition_param(value: &str, key: &str) -> Option<String> {
    value.split(';').filter_map(|p| p.trim().split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| {
        v.trim().trim_matches('"').to_string()
    })
}

impl Form {
    fn parse(body: &[u8], boundary: &str) -> Self {
        let delimiter = format!("--{boundary}").into_bytes();
        let next_delimiter = [b"\r\n".as_slice(), &delimiter].concat();
        let mut parts = Vec::new();
        let mut pos = find(body, &delimiter, 0).expect("a first boundary") + delimiter.len();
        while !body[pos..].starts_with(b"--") {
            pos += 2; // the CRLF after the boundary
            let end = find(body, &next_delimiter, pos).expect("a closing boundary");
            let raw = &body[pos..end];
            let split = find(raw, b"\r\n\r\n", 0).expect("part headers");
            let head = String::from_utf8_lossy(&raw[..split]);
            let mut part =
                FormPart { name: String::new(), file_name: None, content_type: None, data: raw[split + 4..].to_vec() };
            for line in head.split("\r\n") {
                let (key, value) = line.split_once(':').expect("a part header");
                match key.trim().to_ascii_lowercase().as_str() {
                    "content-disposition" => {
                        part.name = disposition_param(value, "name").expect("a field name");
                        part.file_name = disposition_param(value, "filename");
                    }
                    "content-type" => part.content_type = Some(value.trim().to_string()),
                    _ => {}
                }
            }
            parts.push(part);
            pos = end + next_delimiter.len();
        }
        Self { parts }
    }

    pub fn part(&self, name: &str) -> Option<&FormPart> {
        self.parts.iter().find(|p| p.name == name)
    }

    pub fn has(&self, name: &str) -> bool {
        self.part(name).is_some()
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.part(name).map(|p| String::from_utf8_lossy(&p.data).into_owned())
    }

    pub fn get_all(&self, name: &str) -> Vec<String> {
        self.parts
            .iter()
            .filter(|p| p.name == name)
            .map(|p| String::from_utf8_lossy(&p.data).into_owned())
            .collect()
    }
}

#[derive(Default)]
struct State {
    calls: Vec<Call>,
    /// Per call: the client closed a hanging connection.
    closed: Vec<bool>,
}

struct Shared {
    replies: Vec<Reply>,
    state: Mutex<State>,
    changed: Condvar,
    stop: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Blocks until `done` holds, or [`WAIT`] passes.
    fn wait(&self, done: impl Fn(&State) -> bool) -> bool {
        let deadline = Instant::now() + WAIT;
        let mut state = self.lock();
        while !done(&state) {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = match self.changed.wait_timeout(state, deadline - now) {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        true
    }
}

pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Server {
    /// Starts a server; call `n` gets `replies[n]`, or the last reply once they run out.
    pub fn start(replies: Vec<Reply>) -> Self {
        assert!(!replies.is_empty());
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a test server");
        let addr = listener.local_addr().expect("a local address");
        let shared = Arc::new(Shared {
            replies,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let accept = Arc::clone(&shared);
        thread::spawn(move || {
            for stream in listener.incoming() {
                if accept.stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let shared = Arc::clone(&accept);
                thread::spawn(move || serve(stream, &shared));
            }
        });
        Self { addr, shared }
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn calls(&self) -> Vec<Call> {
        self.shared.lock().calls.clone()
    }

    pub fn call(&self, index: usize) -> Call {
        self.calls().get(index).cloned().unwrap_or_else(|| panic!("no call {index}"))
    }

    /// The client closed hanging call `index` (its request was aborted).
    pub fn is_closed(&self, index: usize) -> bool {
        self.shared.lock().closed.get(index).copied().unwrap_or(false)
    }

    /// Waits off the runtime, which also keeps a paused test clock from auto-advancing meanwhile.
    async fn wait(&self, done: impl Fn(&State) -> bool + Send + 'static) -> bool {
        let shared = Arc::clone(&self.shared);
        tokio::task::spawn_blocking(move || shared.wait(done)).await.unwrap_or(false)
    }

    /// Waits until at least `count` requests have arrived.
    pub async fn wait_for_calls(&self, count: usize) -> bool {
        self.wait(move |s| s.calls.len() >= count).await
    }

    /// Waits until the client closes hanging call `index`.
    pub async fn wait_closed(&self, index: usize) -> bool {
        self.wait(move |s| s.closed.get(index).copied().unwrap_or(false)).await
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Call> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut words = line.split_whitespace();
    let method = words.next()?.to_string();
    let target = words.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    let mut body = Vec::new();
    if let Some(length) = header("content-length") {
        body.resize(length.parse().ok()?, 0);
        reader.read_exact(&mut body).ok()?;
    } else if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        loop {
            line.clear();
            reader.read_line(&mut line).ok()?;
            let size = usize::from_str_radix(line.trim().split(';').next()?, 16).ok()?;
            if size == 0 {
                // Trailers, then the blank line.
                loop {
                    line.clear();
                    if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                break;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..]).ok()?;
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).ok()?;
        }
    }
    Some(Call { method, target, headers, body })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn serve(stream: TcpStream, shared: &Shared) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_half);
    let Some(call) = read_request(&mut reader) else { return };
    let index = {
        let mut state = shared.lock();
        state.calls.push(call);
        state.closed.push(false);
        shared.changed.notify_all();
        state.calls.len() - 1
    };
    let reply = &shared.replies[index.min(shared.replies.len() - 1)];
    let mut stream = stream;
    match reply {
        Reply::Respond { status, body, content_type } => {
            let head = format!(
                "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reason(*status),
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
        Reply::Hang => {
            let mut buf = [0u8; 1024];
            while matches!(reader.read(&mut buf), Ok(n) if n > 0) {}
            shared.lock().closed[index] = true;
            shared.changed.notify_all();
        }
        Reply::Fail => {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}
