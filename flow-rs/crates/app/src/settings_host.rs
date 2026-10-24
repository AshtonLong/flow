//! The settings window. It exists only while open: a child process
//! (`Flow.exe --settings-window`) created on demand and gone when closed, so
//! none of its memory is held while Flow sits in the tray. It talks to this
//! process over its stdin/stdout, one JSON message per line.
//! Mirrors `src/main/windows/settings.ts`; the requests are served by `ipc.rs`.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use flow_core::protocol::settings::{Downstream, Event, Reply, RequestEnvelope};
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

/// Serves one request; the reply goes back to the window.
pub type RequestHandler = Arc<dyn Fn(RequestEnvelope) -> crate::session::BoxFuture<Reply> + Send + Sync>;

struct Child {
    pid: u32,
    /// Lines to write to the window's stdin.
    out: mpsc::UnboundedSender<String>,
    kill: tokio::sync::oneshot::Sender<()>,
}

pub struct SettingsHost {
    exe: PathBuf,
    rt: Handle,
    child: Mutex<Option<Child>>,
    handler: Mutex<Option<RequestHandler>>,
    /// Called when the window process exits (e.g. to cancel a hotkey capture).
    on_closed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

impl SettingsHost {
    pub fn new(exe: PathBuf, rt: Handle) -> Arc<Self> {
        Arc::new(Self { exe, rt, child: Mutex::new(None), handler: Mutex::new(None), on_closed: Mutex::new(None) })
    }

    pub fn set_handler(&self, handler: RequestHandler) {
        *self.handler.lock() = Some(handler);
    }

    pub fn on_closed(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.on_closed.lock() = Some(f);
    }

    /// Opens the window, or brings it to the front (on `page`, if given).
    pub fn open(self: &Arc<Self>, page: Option<&str>) {
        let mut slot = self.child.lock();
        if let Some(child) = slot.as_ref() {
            crate::system::allow_set_foreground(child.pid);
            send_line(&child.out, &Downstream::Event(Event::Focus { page: page.map(str::to_string) }));
            return;
        }
        let mut command = Command::new(&self.exe);
        command.arg("--settings-window");
        if let Some(page) = page {
            command.arg("--page").arg(page);
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        command.creation_flags(CREATE_NO_WINDOW);
        let spawned = {
            let _guard = self.rt.enter();
            command.spawn()
        };
        let mut process = match spawned {
            Ok(process) => process,
            Err(err) => {
                log::error!(target: "settings", "could not open the settings window: {err}");
                return;
            }
        };
        let pid = process.id().unwrap_or(0);
        crate::system::allow_set_foreground(pid);
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
        let stdin = process.stdin.take();
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        *slot = Some(Child { pid, out: out_tx.clone(), kill: kill_tx });
        drop(slot);
        log::info!(target: "settings", "window opened (pid {pid})");

        // Writer: queued lines to the window's stdin.
        if let Some(mut stdin) = stdin {
            self.rt.spawn(async move {
                while let Some(line) = out_rx.recv().await {
                    if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                        break;
                    }
                }
            });
        }
        // Reader: requests from the window.
        if let Some(stdout) = stdout {
            let me = self.clone();
            let out = out_tx.clone();
            self.rt.spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let envelope: RequestEnvelope = match serde_json::from_str(&line) {
                        Ok(envelope) => envelope,
                        Err(err) => {
                            log::warn!(target: "settings", "ignored a malformed request: {err}");
                            continue;
                        }
                    };
                    let Some(handler) = me.handler.lock().clone() else { continue };
                    let out = out.clone();
                    tokio::spawn(async move {
                        let id = envelope.id;
                        let result = handler(envelope).await;
                        send_line(&out, &Downstream::Response { id, result });
                    });
                }
            });
        }
        if let Some(stderr) = stderr {
            self.rt.spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    log::warn!(target: "settings", "{line}");
                }
            });
        }
        // Lifetime: the window closes itself, or is told to.
        let me = self.clone();
        self.rt.spawn(async move {
            tokio::select! {
                status = process.wait() => {
                    log::info!(target: "settings", "window closed ({})", status.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()));
                }
                _ = kill_rx => {
                    // Ask nicely first; the window closes on Quit or stdin EOF.
                    let quit = tokio::time::timeout(std::time::Duration::from_secs(2), process.wait()).await;
                    if quit.is_err() {
                        let _ = process.kill().await;
                    }
                }
            }
            let mut slot = me.child.lock();
            if slot.as_ref().is_some_and(|c| c.pid == pid) {
                *slot = None;
            }
            drop(slot);
            if let Some(f) = me.on_closed.lock().clone() {
                f();
            }
        });
    }

    /// Pushes an event to the window, if it is open.
    pub fn send(&self, event: Event) {
        if let Some(child) = self.child.lock().as_ref() {
            send_line(&child.out, &Downstream::Event(event));
        }
    }

    /// Closes the window.
    pub fn close(&self) {
        if let Some(child) = self.child.lock().take() {
            send_line(&child.out, &Downstream::Event(Event::Quit));
            let _ = child.kill.send(());
        }
    }
}

fn send_line(out: &mpsc::UnboundedSender<String>, message: &Downstream) {
    match serde_json::to_string(message) {
        Ok(mut line) => {
            line.push('\n');
            let _ = out.send(line);
        }
        Err(err) => log::error!(target: "settings", "could not encode a message: {err}"),
    }
}
