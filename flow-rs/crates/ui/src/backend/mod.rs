//! The settings window's link to the main process: the [`Backend`] trait, its
//! two implementations (the stdin/stdout pipe and the in-memory mock), and the
//! [`Link`] the UI thread uses to call it.
//!
//! Replies and events arrive on backend threads. They are handed to the UI
//! thread through channels that the window drains every frame, and each
//! arrival calls `request_repaint`, so the window sleeps while nothing happens.

mod mock;
mod pipe;

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use flow_core::protocol::settings::{Event, Reply, Request};
use serde::de::DeserializeOwned;

pub use mock::{MockOptions, mock_backend, mock_backend_with};
pub use pipe::pipe_backend;

/// The settings window's link to the main process (or to the mock).
pub trait Backend: Send + Sync {
    /// Sends one request. `reply` is called exactly once, on any thread.
    fn request(&self, request: Request, reply: Box<dyn FnOnce(Reply) + Send>);
    /// Registers a listener for pushes from the main process. Called once.
    fn subscribe(&self, on_event: Box<dyn Fn(Event) + Send + Sync>);
}

/// A reply that has not arrived yet: the stand-in for a JS promise.
///
/// Poll it from the UI each frame (the arrival repaints the window). Dropping
/// it ignores the reply, like an unmounted React component would.
pub struct Pending<T> {
    rx: Receiver<Result<T, String>>,
    done: Option<Result<T, String>>,
}

/// Clones share the reply: whichever clone polls first receives it. Exists so a
/// pending call can sit in egui memory (component state), which needs `Clone`.
impl<T: Clone> Clone for Pending<T> {
    fn clone(&self) -> Self {
        Self { rx: self.rx.clone(), done: self.done.clone() }
    }
}

impl<T> Pending<T> {
    /// A pending value that is already settled (for code paths that need no call).
    pub fn ready(value: Result<T, String>) -> Self {
        let (_tx, rx) = crossbeam_channel::bounded(1);
        Self { rx, done: Some(value) }
    }

    /// Settles when something is sent on the other end of `rx`.
    pub(crate) fn from_receiver(rx: Receiver<Result<T, String>>) -> Self {
        Self { rx, done: None }
    }

    fn pull(&mut self) {
        if self.done.is_none() {
            match self.rx.try_recv() {
                Ok(value) => self.done = Some(value),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.done = Some(Err("The settings window lost its connection to Flow".into()));
                }
            }
        }
    }

    /// The result, once it has arrived. Stays available until [`Self::take`].
    pub fn poll(&mut self) -> Option<&Result<T, String>> {
        self.pull();
        self.done.as_ref()
    }

    /// Takes the result once it has arrived. Returns `None` while waiting and after taking.
    pub fn take(&mut self) -> Option<Result<T, String>> {
        self.pull();
        self.done.take()
    }

    /// True while the reply has not arrived.
    pub fn is_pending(&mut self) -> bool {
        self.pull();
        self.done.is_none()
    }
}

/// Takes the result of an optional pending call and clears the slot when it settles.
///
/// ```ignore
/// if let Some(result) = take_settled(&mut self.checking) { ... }
/// ```
pub fn take_settled<T>(slot: &mut Option<Pending<T>>) -> Option<Result<T, String>> {
    let result = slot.as_mut()?.take()?;
    *slot = None;
    Some(result)
}

/// What the UI thread holds: the backend plus the context to wake.
#[derive(Clone)]
pub struct Link {
    backend: Arc<dyn Backend>,
    ctx: egui::Context,
}

impl Link {
    pub(crate) fn new(backend: Arc<dyn Backend>, ctx: egui::Context) -> Self {
        Self { backend, ctx }
    }

    /// Calls the main process. Decode the result as the type the [`Request`]
    /// variant documents (`()` for "→ `()`").
    pub fn call<T: DeserializeOwned + Send + 'static>(&self, request: Request) -> Pending<T> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = self.ctx.clone();
        self.backend.request(
            request,
            Box::new(move |reply| {
                let _ = tx.send(reply.decode::<T>());
                ctx.request_repaint();
            }),
        );
        Pending { rx, done: None }
    }

    /// Fire and forget. Failures are logged.
    pub fn send(&self, request: Request) {
        let name = request_name(&request);
        self.backend.request(
            request,
            Box::new(move |reply| {
                if let Reply::Err { error } = reply {
                    log::warn!(target: "settings", "{name} failed: {error}");
                }
            }),
        );
    }

    /// The egui context, e.g. to request a repaint from a callback.
    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }
}

fn request_name(request: &Request) -> String {
    serde_json::to_value(request)
        .ok()
        .and_then(|v| v.get("method").and_then(|m| m.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "request".into())
}

/// Events from the backend, queued for the UI thread.
pub(crate) struct EventQueue {
    rx: Receiver<Event>,
}

impl EventQueue {
    /// Subscribes to the backend; every event wakes the window.
    pub(crate) fn subscribe(backend: &Arc<dyn Backend>, ctx: &egui::Context) -> Self {
        let (tx, rx): (Sender<Event>, Receiver<Event>) = crossbeam_channel::unbounded();
        let ctx = ctx.clone();
        backend.subscribe(Box::new(move |event| {
            let _ = tx.send(event);
            ctx.request_repaint();
        }));
        Self { rx }
    }

    /// Everything that arrived since the last frame.
    pub(crate) fn drain(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_settles_once() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut p: Pending<u32> = Pending { rx, done: None };
        assert!(p.is_pending());
        tx.send(Ok(3)).unwrap();
        assert_eq!(p.poll(), Some(&Ok(3)));
        assert_eq!(p.take(), Some(Ok(3)));
        assert_eq!(p.take(), None);
    }

    #[test]
    fn dropped_sender_is_an_error() {
        let (tx, rx) = crossbeam_channel::bounded::<Result<u32, String>>(1);
        drop(tx);
        let mut p = Pending { rx, done: None };
        assert!(matches!(p.take(), Some(Err(_))));
    }

    #[test]
    fn names_requests() {
        assert_eq!(request_name(&Request::ConfigGet), "ConfigGet");
        assert_eq!(request_name(&Request::AppSetPaused(true)), "AppSetPaused");
    }
}
