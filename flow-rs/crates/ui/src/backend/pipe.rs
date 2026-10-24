//! The real backend: JSON lines over the process's own stdin and stdout.
//!
//! Requests go up as [`RequestEnvelope`] lines on stdout; [`Downstream`] lines
//! (responses and events) come down on stdin and are read by one thread. When
//! stdin closes, the main process is gone: pending requests fail and the
//! window receives [`Event::Quit`].

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use flow_core::protocol::settings::{Downstream, Event, Reply, Request, RequestEnvelope};
use parking_lot::Mutex;

use super::Backend;

type ReplyFn = Box<dyn FnOnce(Reply) + Send>;
type Listener = Box<dyn Fn(Event) + Send + Sync>;

const CLOSED: &str = "Flow is not running";

/// Requests on stdout, responses and events from stdin. EOF on stdin closes the window.
pub fn pipe_backend() -> Arc<dyn Backend> {
    Arc::new(PipeBackend::start(BufReader::new(std::io::stdin()), std::io::stdout()))
}

#[derive(Default)]
struct Shared {
    pending: Mutex<HashMap<u64, ReplyFn>>,
    listeners: Mutex<Vec<Listener>>,
    /// Events that arrived before anyone subscribed.
    early: Mutex<Vec<Event>>,
    closed: AtomicBool,
}

impl Shared {
    fn dispatch(&self, event: Event) {
        let listeners = self.listeners.lock();
        if listeners.is_empty() {
            self.early.lock().push(event);
            return;
        }
        for listener in listeners.iter() {
            listener(event.clone());
        }
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let pending: Vec<ReplyFn> = self.pending.lock().drain().map(|(_, f)| f).collect();
        for reply in pending {
            reply(Reply::err(CLOSED));
        }
        self.dispatch(Event::Quit);
    }
}

pub(crate) struct PipeBackend {
    next_id: AtomicU64,
    out: Mutex<Box<dyn Write + Send>>,
    shared: Arc<Shared>,
}

impl PipeBackend {
    pub(crate) fn start(input: impl BufRead + Send + 'static, output: impl Write + Send + 'static) -> Self {
        let shared = Arc::new(Shared::default());
        let reader = Arc::clone(&shared);
        let spawned = std::thread::Builder::new().name("settings-pipe".into()).spawn(move || read_loop(input, &reader));
        if let Err(err) = spawned {
            log::error!(target: "settings", "could not start the pipe reader: {err}");
            shared.close();
        }
        Self { next_id: AtomicU64::new(1), out: Mutex::new(Box::new(output)), shared }
    }
}

fn read_loop(mut input: impl BufRead, shared: &Shared) {
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                log::warn!(target: "settings", "stdin read failed: {err}");
                break;
            }
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        match serde_json::from_str::<Downstream>(text) {
            Ok(Downstream::Response { id, result }) => {
                let reply = shared.pending.lock().remove(&id);
                match reply {
                    Some(reply) => reply(result),
                    None => log::debug!(target: "settings", "response for unknown request {id}"),
                }
            }
            Ok(Downstream::Event(event)) => shared.dispatch(event),
            Err(err) => log::warn!(target: "settings", "unreadable message from Flow: {err}"),
        }
    }
    shared.close();
}

impl Backend for PipeBackend {
    fn request(&self, request: Request, reply: ReplyFn) {
        if self.shared.closed.load(Ordering::SeqCst) {
            reply(Reply::err(CLOSED));
            return;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let line = match serde_json::to_string(&RequestEnvelope { id, request }) {
            Ok(line) => line,
            Err(err) => {
                reply(Reply::err(err.to_string()));
                return;
            }
        };
        self.shared.pending.lock().insert(id, reply);
        let written = {
            let mut out = self.out.lock();
            out.write_all(line.as_bytes()).and_then(|()| out.write_all(b"\n")).and_then(|()| out.flush())
        };
        if let Err(err) = written {
            log::warn!(target: "settings", "stdout write failed: {err}");
            if let Some(reply) = self.shared.pending.lock().remove(&id) {
                reply(Reply::err(CLOSED));
            }
            self.shared.close();
        }
    }

    fn subscribe(&self, on_event: Box<dyn Fn(Event) + Send + Sync>) {
        let early: Vec<Event> = std::mem::take(&mut *self.shared.early.lock());
        for event in early {
            on_event(event);
        }
        self.shared.listeners.lock().push(on_event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A writer the test can read back.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writes_requests_as_json_lines() {
        // The reader sees EOF at once, so the backend closes; requests then fail fast.
        let sink = Sink::default();
        let (ready_tx, ready_rx) = mpsc::channel();
        let pipe = PipeBackend::start(Cursor::new(Vec::<u8>::new()), sink.clone());
        pipe.subscribe(Box::new(move |event| {
            let _ = ready_tx.send(event);
        }));
        assert_eq!(ready_rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Quit);
        let (tx, rx) = mpsc::channel();
        pipe.request(Request::ConfigGet, Box::new(move |r| tx.send(r).unwrap()));
        assert_eq!(rx.recv().unwrap(), Reply::err(CLOSED));
    }

    #[test]
    fn routes_responses_and_events() {
        let (mut writer, reader) = pipe_pair();
        let sink = Sink::default();
        let pipe = PipeBackend::start(reader, sink.clone());
        let (event_tx, event_rx) = mpsc::channel();
        pipe.subscribe(Box::new(move |event| {
            let _ = event_tx.send(event);
        }));

        let (tx, rx) = mpsc::channel();
        pipe.request(Request::AppSetPaused(true), Box::new(move |r| tx.send(r).unwrap()));
        let written = String::from_utf8(sink.0.lock().clone()).unwrap();
        let envelope: RequestEnvelope = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(envelope.request, Request::AppSetPaused(true));

        let response =
            serde_json::to_string(&Downstream::Response { id: envelope.id, result: Reply::ok(&()) }).unwrap();
        let event = serde_json::to_string(&Downstream::Event(Event::Focus { page: Some("models".into()) })).unwrap();
        writeln!(writer, "{response}\n{event}").unwrap();
        drop(writer);

        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Reply::ok(&()));
        assert_eq!(
            event_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Event::Focus { page: Some("models".into()) }
        );
        // EOF after the writer is dropped.
        assert_eq!(event_rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Quit);
    }

    /// An in-memory pipe: lines written to the first half are read from the second.
    fn pipe_pair() -> (PipeWriter, BufReader<PipeReader>) {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        (PipeWriter(tx), BufReader::new(PipeReader { rx, buf: Vec::new() }))
    }
    struct PipeWriter(mpsc::Sender<Vec<u8>>);
    impl Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.send(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    struct PipeReader {
        rx: mpsc::Receiver<Vec<u8>>,
        buf: Vec<u8>,
    }
    impl std::io::Read for PipeReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.buf.is_empty() {
                match self.rx.recv() {
                    Ok(chunk) => self.buf = chunk,
                    Err(_) => return Ok(0),
                }
            }
            let n = out.len().min(self.buf.len());
            out[..n].copy_from_slice(&self.buf[..n]);
            self.buf.drain(..n);
            Ok(n)
        }
    }
}
