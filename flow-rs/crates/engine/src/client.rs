//! Main-process client for the engine process. Starts the engine on demand,
//! restarts it after a crash, and exits it after the idle timeout so a loaded
//! model's memory goes back to the system. Port of
//! `src/main/transcribe/engine-client.ts`.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::Duration;

use flow_core::protocol::engine::{
    EngineLoadResult, EngineModelRef, EngineProbeResult, EngineRequest, EngineResponse, EngineResult,
    EngineTranscribeResult, MAX_HEADER_BYTES, MAX_PCM_SAMPLES, VadKind, env, write_frame,
};
use flow_core::types::BackendDevice;
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

/// How long the engine has to say hello after it is spawned.
const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
/// Idle delay when models should unload right after each dictation.
const UNLOAD_SOON: Duration = Duration::from_millis(1500);
/// Default for `model.keep_loaded_minutes` until the app sets it.
const DEFAULT_KEEP_LOADED_MINUTES: f64 = 10.0;

const NOT_STARTED: &str = "The speech engine did not start";
const STOPPED: &str = "The speech engine stopped unexpectedly";
const NOT_RUNNING: &str = "The speech engine is not running";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineOptions {
    /// The executable to start (the app passes its own exe).
    pub exe: PathBuf,
    /// Its arguments (the app passes `--engine`).
    pub args: Vec<String>,
    /// Explicit native library path (`FLOW_TRANSCRIBE_LIBRARY`), e.g. the GPU pack's.
    pub library_path: Option<PathBuf>,
    /// Directory holding the CUDA runtime, when the optional GPU pack is installed.
    pub cuda_runtime_dir: Option<PathBuf>,
    /// Route the native library's diagnostics to the engine's stderr (and so the log).
    pub debug: bool,
}

/// A failed engine request. `aborted` is set when it was cancelled; the
/// message is then "Aborted" (the TS `EngineAbortError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub message: String,
    pub aborted: bool,
}

impl EngineError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), aborted: false }
    }

    pub fn aborted() -> Self {
        Self { message: "Aborted".into(), aborted: true }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

#[derive(Debug, Clone, PartialEq)]
pub struct TrimOutput {
    /// Speech with silence removed, or `None` when no speech was found.
    pub audio: Option<Vec<f32>>,
    pub speech_ms: f64,
    pub detector: VadKind,
}

type Reply = Result<(EngineResult, Vec<f32>), EngineError>;
type Listener = Arc<dyn Fn(&str) + Send + Sync>;

/// One engine process.
struct Conn {
    serial: u64,
    pid: Option<u32>,
    /// Encoded frames for the stdin writer task.
    frames: mpsc::UnboundedSender<Vec<u8>>,
    /// `None` until the engine said hello (or failed to).
    ready: watch::Receiver<Option<Result<(), EngineError>>>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
}

impl Conn {
    fn post(&self, request: &EngineRequest, pcm: &[f32]) {
        let mut buf = Vec::with_capacity(256 + pcm.len() * 4);
        // Serialising a request cannot fail; writing to a Vec cannot either.
        if write_frame(&mut buf, request, pcm).is_ok() {
            let _ = self.frames.send(buf);
        }
    }

    fn kill(&self) {
        if let Some(kill) = self.kill.lock().take() {
            let _ = kill.send(());
        }
    }
}

struct State {
    conn: Option<Arc<Conn>>,
    serial: u64,
    next_id: u64,
    pending: HashMap<u64, oneshot::Sender<Reply>>,
    idle_timer: Option<tokio::task::AbortHandle>,
    idle_serial: u64,
    keep_loaded_minutes: f64,
    loaded_id: Option<String>,
    devices: Vec<BackendDevice>,
    vad: Option<VadKind>,
    runtime: Option<tokio::runtime::Handle>,
}

pub struct EngineClient {
    opts: EngineOptions,
    me: Weak<EngineClient>,
    state: Mutex<State>,
    listeners: Mutex<Vec<Listener>>,
}

impl EngineClient {
    pub fn new(opts: EngineOptions) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            opts,
            me: me.clone(),
            state: Mutex::new(State {
                conn: None,
                serial: 0,
                next_id: 1,
                pending: HashMap::new(),
                idle_timer: None,
                idle_serial: 0,
                keep_loaded_minutes: DEFAULT_KEEP_LOADED_MINUTES,
                loaded_id: None,
                devices: Vec::new(),
                vad: None,
                runtime: tokio::runtime::Handle::try_current().ok(),
            }),
            listeners: Mutex::new(Vec::new()),
        })
    }

    /// Local model currently held in engine memory.
    pub fn loaded_model_id(&self) -> Option<String> {
        self.state.lock().loaded_id.clone()
    }

    pub fn running(&self) -> bool {
        self.state.lock().conn.is_some()
    }

    /// Process id of the running engine.
    pub fn pid(&self) -> Option<u32> {
        self.state.lock().conn.as_ref().and_then(|c| c.pid)
    }

    /// Compute devices seen by the last probe.
    pub fn devices(&self) -> Vec<BackendDevice> {
        self.state.lock().devices.clone()
    }

    pub fn vad(&self) -> Option<VadKind> {
        self.state.lock().vad
    }

    /// `0` unloads right after each dictation, `-1` never unloads.
    pub fn set_keep_loaded_minutes(&self, minutes: f64) {
        self.state.lock().keep_loaded_minutes = minutes;
        self.schedule_idle_exit();
    }

    /// Called when the loaded model changes, with its id (an empty string after
    /// an unload). Not called when the engine exits.
    pub fn on_loaded(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        self.listeners.lock().push(Arc::new(f));
    }

    /// Exits the engine process, releasing the model.
    pub fn shutdown(&self) {
        let mut st = self.state.lock();
        let Some(conn) = st.conn.clone() else { return };
        let pending = forget_engine(&mut st);
        drop(st);
        reject_pending(pending, Some(0));
        conn.kill();
    }

    pub async fn probe(&self) -> Result<EngineProbeResult, EngineError> {
        match self.request(|id| EngineRequest::Probe { id }, &[], None).await? {
            (EngineResult::Probe(result), _) => {
                let mut st = self.state.lock();
                st.devices = result.devices.clone();
                st.vad = Some(result.vad);
                Ok(result)
            }
            (other, _) => Err(unexpected(&other)),
        }
    }

    pub async fn load(&self, model: &EngineModelRef, cancel: Option<CancellationToken>) -> Result<EngineLoadResult, EngineError> {
        let reply = self.request(|id| EngineRequest::Load { id, model: model.clone() }, &[], cancel).await?;
        let EngineResult::Load(result) = reply.0 else { return Err(unexpected(&reply.0)) };
        self.set_loaded(Some(&model.model_id));
        Ok(result)
    }

    pub async fn transcribe(
        &self,
        model: &EngineModelRef,
        audio: &[f32],
        language: &str,
        hints: &[String],
        cancel: Option<CancellationToken>,
    ) -> Result<EngineTranscribeResult, EngineError> {
        let request = |id| EngineRequest::Transcribe {
            id,
            model: model.clone(),
            language: language.to_owned(),
            hints: hints.to_vec(),
        };
        let reply = self.request(request, audio, cancel).await?;
        let EngineResult::Transcribe(result) = reply.0 else { return Err(unexpected(&reply.0)) };
        self.set_loaded(Some(&model.model_id));
        Ok(result)
    }

    pub async fn trim(&self, audio: &[f32]) -> Result<TrimOutput, EngineError> {
        match self.request(|id| EngineRequest::Trim { id }, audio, None).await? {
            (EngineResult::Trim(result), pcm) => Ok(TrimOutput {
                audio: result.has_speech.then_some(pcm),
                speech_ms: result.speech_ms,
                detector: result.detector,
            }),
            (other, _) => Err(unexpected(&other)),
        }
    }

    pub async fn unload(&self) -> Result<(), EngineError> {
        if !self.running() {
            return Ok(());
        }
        self.request(|id| EngineRequest::Unload { id }, &[], None).await?;
        self.set_loaded(None);
        Ok(())
    }

    fn set_loaded(&self, id: Option<&str>) {
        {
            let mut st = self.state.lock();
            if st.loaded_id.as_deref() == id {
                return;
            }
            st.loaded_id = id.map(str::to_owned);
        }
        let listeners = self.listeners.lock().clone();
        for listener in listeners {
            listener(id.unwrap_or(""));
        }
    }

    async fn request(
        &self,
        body: impl FnOnce(u64) -> EngineRequest,
        pcm: &[f32],
        cancel: Option<CancellationToken>,
    ) -> Result<(EngineResult, Vec<f32>), EngineError> {
        if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(EngineError::aborted());
        }
        // The engine can exit (idle timeout) between saying hello and taking the
        // request; JS cannot interleave there, so start it again once.
        let mut attempts = 0;
        let (conn, id, mut rx) = loop {
            let conn = self.start().await?;
            let mut st = self.state.lock();
            if st.conn.as_ref().is_some_and(|c| Arc::ptr_eq(c, &conn)) {
                let id = st.next_id;
                st.next_id += 1;
                cancel_idle_timer(&mut st);
                let (tx, rx) = oneshot::channel();
                st.pending.insert(id, tx);
                conn.post(&body(id), pcm);
                break (conn, id, rx);
            }
            attempts += 1;
            if attempts == 2 {
                return Err(EngineError::new(NOT_RUNNING));
            }
        };
        let Some(cancel) = cancel else {
            return rx.await.unwrap_or_else(|_| Err(EngineError::new(STOPPED)));
        };
        tokio::select! {
            reply = &mut rx => return reply.unwrap_or_else(|_| Err(EngineError::new(STOPPED))),
            () = cancel.cancelled() => {}
        }
        // Ask the engine to abort; its reply (usually the abort) settles the request.
        let cancel_id = {
            let mut st = self.state.lock();
            let cancel_id = st.next_id;
            st.next_id += 1;
            cancel_id
        };
        conn.post(&EngineRequest::Cancel { id: cancel_id, target: id }, &[]);
        rx.await.unwrap_or_else(|_| Err(EngineError::new(STOPPED)))
    }

    /// Starts the engine unless it is running; waits until it says hello.
    async fn start(&self) -> Result<Arc<Conn>, EngineError> {
        let conn = {
            let mut st = self.state.lock();
            st.runtime = tokio::runtime::Handle::try_current().ok();
            match st.conn.clone() {
                Some(conn) => conn,
                None => self.spawn(&mut st)?,
            }
        };
        // Keep a running engine from timing out while this request gets going.
        cancel_idle_timer(&mut self.state.lock());
        let mut ready = conn.ready.clone();
        let outcome = match ready.wait_for(Option::is_some).await {
            Ok(outcome) => outcome.clone().unwrap_or_else(|| Err(EngineError::new(NOT_STARTED))),
            Err(_) => Err(EngineError::new(NOT_STARTED)),
        };
        outcome.map(|()| conn)
    }

    fn spawn(&self, st: &mut State) -> Result<Arc<Conn>, EngineError> {
        let mut cmd = Command::new(&self.opts.exe);
        cmd.args(&self.opts.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        if self.opts.debug {
            cmd.env(env::DEBUG, "1");
        }
        if let Some(path) = &self.opts.library_path {
            cmd.env(env::LIBRARY, path);
        }
        if let Some(dir) = &self.opts.cuda_runtime_dir {
            cmd.env(env::CUDA_RUNTIME_DIR, dir);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|err| {
            log::error!(target: "engine", "could not start {}: {err}", self.opts.exe.display());
            EngineError::new(NOT_STARTED)
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(EngineError::new(NOT_STARTED));
        };

        st.serial += 1;
        let serial = st.serial;
        let (frames, frame_rx) = mpsc::unbounded_channel();
        let (ready_tx, ready) = watch::channel(None);
        let ready_tx = Arc::new(ready_tx);
        let (kill, kill_rx) = oneshot::channel();
        let conn = Arc::new(Conn { serial, pid: child.id(), frames, ready, kill: Mutex::new(Some(kill)) });
        st.conn = Some(Arc::clone(&conn));

        tokio::spawn(write_frames(stdin, frame_rx));
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    log::warn!(target: "engine", "{line}");
                }
            }
        });
        tokio::spawn({
            let (me, ready_tx) = (self.me.clone(), Arc::clone(&ready_tx));
            async move {
                let mut stdout = BufReader::new(stdout);
                loop {
                    match read_response(&mut stdout).await {
                        Ok(Some((EngineResponse::Hello { .. }, _))) => settle(&ready_tx, Ok(())),
                        Ok(Some((response, pcm))) => match me.upgrade() {
                            Some(client) => client.on_response(response, pcm),
                            None => break,
                        },
                        Ok(None) => break,
                        Err(err) => {
                            log::error!(target: "engine", "unreadable response: {err}");
                            break;
                        }
                    }
                }
            }
        });
        tokio::spawn({
            let (me, ready_tx) = (self.me.clone(), Arc::clone(&ready_tx));
            async move {
                let code = wait_or_kill(child, kill_rx).await;
                if let Some(client) = me.upgrade() {
                    client.on_exit(serial, code);
                }
                settle(&ready_tx, Err(EngineError::new(format!("The speech engine exited (code {})", fmt_code(code)))));
            }
        });
        tokio::spawn({
            let (me, mut ready) = (self.me.clone(), conn.ready.clone());
            async move {
                if tokio::time::timeout(HELLO_TIMEOUT, ready.wait_for(Option::is_some)).await.is_ok() {
                    return;
                }
                settle(&ready_tx, Err(EngineError::new(NOT_STARTED)));
                // A failed start must not poison later attempts.
                if let Some(client) = me.upgrade() {
                    client.start_failed(serial);
                }
            }
        });
        Ok(conn)
    }

    fn on_response(&self, response: EngineResponse, pcm: Vec<f32>) {
        let (id, reply) = match response {
            EngineResponse::Hello { .. } => return,
            EngineResponse::Ok { id, result } => (id, Ok((result, pcm))),
            EngineResponse::Err { id, aborted: true, .. } => (id, Err(EngineError::aborted())),
            EngineResponse::Err { id, error, .. } => (id, Err(EngineError::new(error))),
        };
        let Some(pending) = self.state.lock().pending.remove(&id) else { return };
        let _ = pending.send(reply);
        self.schedule_idle_exit();
    }

    /// The process exited on its own (or after a kill).
    fn on_exit(&self, serial: u64, code: Option<i32>) {
        let mut st = self.state.lock();
        if !st.conn.as_ref().is_some_and(|c| c.serial == serial) {
            return;
        }
        let pending = forget_engine(&mut st);
        drop(st);
        reject_pending(pending, code);
    }

    /// The engine never said hello: forget it (and stop it, so it cannot linger).
    fn start_failed(&self, serial: u64) {
        let mut st = self.state.lock();
        let Some(conn) = st.conn.clone().filter(|c| c.serial == serial) else { return };
        let pending = forget_engine(&mut st);
        drop(st);
        reject_pending(pending, None);
        conn.kill();
    }

    fn schedule_idle_exit(&self) {
        let mut st = self.state.lock();
        cancel_idle_timer(&mut st);
        if st.conn.is_none() || !st.pending.is_empty() || st.keep_loaded_minutes < 0.0 {
            return;
        }
        let delay = if st.keep_loaded_minutes == 0.0 {
            UNLOAD_SOON
        } else {
            // A delay too long to represent never fires.
            match Duration::try_from_secs_f64(st.keep_loaded_minutes * 60.0) {
                Ok(delay) => delay,
                Err(_) => return,
            }
        };
        let Some(runtime) = st.runtime.clone() else { return };
        let (me, serial) = (self.me.clone(), st.idle_serial);
        let task = runtime.spawn(async move {
            tokio::time::sleep(delay).await;
            if let Some(client) = me.upgrade() {
                client.idle_elapsed(serial);
            }
        });
        st.idle_timer = Some(task.abort_handle());
    }

    fn idle_elapsed(&self, serial: u64) {
        let mut st = self.state.lock();
        if st.idle_serial != serial || !st.pending.is_empty() {
            return;
        }
        let Some(conn) = st.conn.clone() else { return };
        forget_engine(&mut st);
        drop(st);
        conn.kill();
    }
}

/// Clears the running engine from the state (`handleExit`); returns the
/// requests still waiting on it. The loaded model is forgotten without a
/// `loaded` notification, as in the TS.
fn forget_engine(st: &mut State) -> HashMap<u64, oneshot::Sender<Reply>> {
    st.conn = None;
    st.loaded_id = None;
    cancel_idle_timer(st);
    std::mem::take(&mut st.pending)
}

fn reject_pending(pending: HashMap<u64, oneshot::Sender<Reply>>, code: Option<i32>) {
    let crashed = !pending.is_empty();
    for (_, reply) in pending {
        let _ = reply.send(Err(EngineError::new(STOPPED)));
    }
    if crashed {
        log::error!(target: "engine", "exited with code {} while busy", fmt_code(code));
    }
}

fn cancel_idle_timer(st: &mut State) {
    if let Some(timer) = st.idle_timer.take() {
        timer.abort();
    }
    // A timer that already woke up sees the new serial and does nothing.
    st.idle_serial += 1;
}

impl Drop for EngineClient {
    fn drop(&mut self) {
        if let Some(conn) = self.state.get_mut().conn.take() {
            conn.kill();
        }
    }
}

fn unexpected(result: &EngineResult) -> EngineError {
    EngineError::new(format!("The speech engine sent an unexpected reply: {result:?}"))
}

fn settle(ready: &watch::Sender<Option<Result<(), EngineError>>>, outcome: Result<(), EngineError>) {
    ready.send_if_modified(|current| {
        if current.is_some() {
            return false;
        }
        *current = Some(outcome);
        true
    });
}

/// `${code}` for a JS exit code that may be null.
fn fmt_code(code: Option<i32>) -> String {
    code.map_or_else(|| "null".to_owned(), |c| c.to_string())
}

async fn write_frames(mut stdin: ChildStdin, mut frames: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(frame) = frames.recv().await {
        if stdin.write_all(&frame).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
    // Dropping stdin closes it, which tells the engine to exit.
}

async fn wait_or_kill(mut child: Child, kill: oneshot::Receiver<()>) -> Option<i32> {
    let status = tokio::select! {
        status = child.wait() => status,
        _ = kill => {
            let _ = child.start_kill();
            child.wait().await
        }
    };
    status.ok().and_then(|s| s.code())
}

/// Reads one response frame; `Ok(None)` at a clean end of stream.
async fn read_response<R: AsyncRead + Unpin>(input: &mut R) -> std::io::Result<Option<(EngineResponse, Vec<f32>)>> {
    let json_len = match input.read_u32_le().await {
        Ok(n) => n,
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    };
    if json_len > MAX_HEADER_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "engine frame header too large"));
    }
    let mut json = vec![0u8; json_len as usize];
    input.read_exact(&mut json).await?;
    let samples = input.read_u32_le().await?;
    if samples > MAX_PCM_SAMPLES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "engine frame audio too long"));
    }
    let mut raw = vec![0u8; samples as usize * 4];
    input.read_exact(&mut raw).await?;
    let pcm = raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    let response = serde_json::from_slice(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some((response, pcm)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_frames_written_by_the_protocol() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &EngineResponse::Hello { hello: true, pid: 7 }, &[]).unwrap();
        write_frame(
            &mut buf,
            &EngineResponse::Ok {
                id: 3,
                result: EngineResult::Trim(flow_core::protocol::engine::EngineTrimResult {
                    has_speech: true,
                    speech_ms: 0.125,
                    detector: VadKind::Silero,
                }),
            },
            &[0.5, -1.0],
        )
        .unwrap();
        let mut input = &buf[..];
        let (hello, _) = read_response(&mut input).await.unwrap().unwrap();
        assert_eq!(hello, EngineResponse::Hello { hello: true, pid: 7 });
        let (_, pcm) = read_response(&mut input).await.unwrap().unwrap();
        assert_eq!(pcm, vec![0.5, -1.0]);
        assert!(read_response(&mut input).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rejects_an_oversized_header() {
        let buf = u32::MAX.to_le_bytes();
        assert!(read_response(&mut &buf[..]).await.is_err());
    }

    #[test]
    fn formats_exit_codes_like_javascript() {
        assert_eq!(fmt_code(None), "null");
        assert_eq!(fmt_code(Some(3)), "3");
    }

    #[tokio::test]
    async fn a_missing_executable_does_not_start() {
        let client = EngineClient::new(EngineOptions {
            exe: PathBuf::from("C:\\definitely\\not\\here.exe"),
            args: vec![],
            library_path: None,
            cuda_runtime_dir: None,
            debug: false,
        });
        let err = client.trim(&[0.0; 16]).await.unwrap_err();
        assert_eq!(err, EngineError::new("The speech engine did not start"));
        assert!(!client.running());
    }

    #[tokio::test]
    async fn a_cancelled_token_aborts_before_starting() {
        let client = EngineClient::new(EngineOptions {
            exe: PathBuf::from("C:\\definitely\\not\\here.exe"),
            args: vec![],
            library_path: None,
            cuda_runtime_dir: None,
            debug: false,
        });
        let token = CancellationToken::new();
        token.cancel();
        let model = EngineModelRef { model_id: "m".into(), path: "m.gguf".into(), device: Default::default() };
        assert_eq!(client.load(&model, Some(token)).await.unwrap_err(), EngineError::aborted());
        assert!(!client.running());
    }
}
