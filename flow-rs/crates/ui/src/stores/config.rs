//! The config store. `config.toml` is the source of truth: every edit is sent
//! to the main process at once and the window re-renders from the snapshot
//! that comes back, or from `ConfigChanged` when the file is edited elsewhere.
//! Port of `lib/config.tsx`.

use flow_core::config::{Config, ConfigPatch, ConfigPath, ConfigSnapshot};
use flow_core::protocol::settings::{Event, Request};
use serde::Serialize;

use crate::backend::{Link, Pending};
use crate::util::format::error_message;
use crate::util::patch::patched_config;

struct Write {
    id: u64,
    reply: Pending<ConfigSnapshot>,
    /// Tells the caller of [`ConfigStore::patch`] that the write settled.
    done: crossbeam_channel::Sender<Result<(), String>>,
}

pub struct ConfigStore {
    link: Link,
    snapshot: Option<ConfigSnapshot>,
    /// The last load or write error, cleared by the next successful write.
    error: Option<String>,
    // Writes in flight. While any are pending the optimistic state wins, and
    // only the reply to the newest write is applied, so the UI never flickers back.
    pending: usize,
    sequence: u64,
    initial: Option<Pending<ConfigSnapshot>>,
    writes: Vec<Write>,
    refetch: Option<Pending<ConfigSnapshot>>,
}

impl ConfigStore {
    pub(crate) fn new(link: Link) -> Self {
        let initial = Some(link.call(Request::ConfigGet));
        Self { link, snapshot: None, error: None, pending: 0, sequence: 0, initial, writes: Vec::new(), refetch: None }
    }

    /// Applies replies and `ConfigChanged` events. The app calls this once per frame.
    pub(crate) fn pump(&mut self, events: &[Event]) {
        if let Some(reply) = self.initial.as_mut().and_then(Pending::take) {
            self.initial = None;
            match reply {
                Ok(next) if self.pending == 0 => self.snapshot = Some(next),
                Ok(_) => {}
                Err(err) => self.error = Some(error_message(&err)),
            }
        }

        let mut i = 0;
        while i < self.writes.len() {
            let Some(result) = self.writes[i].reply.take() else {
                i += 1;
                continue;
            };
            let write = self.writes.remove(i);
            self.pending -= 1;
            match result {
                Ok(next) => {
                    if write.id == self.sequence {
                        self.snapshot = Some(next);
                    }
                    self.error = None;
                    let _ = write.done.send(Ok(()));
                }
                Err(err) => {
                    self.error = Some(format!("The change was not saved: {}", error_message(&err)));
                    if self.pending == 0 {
                        self.refetch = Some(self.link.call(Request::ConfigGet));
                    }
                    let _ = write.done.send(Err(err));
                }
            }
        }

        if let Some(reply) = self.refetch.as_mut().and_then(Pending::take) {
            self.refetch = None;
            // On failure keep showing the optimistic state; the error banner is already up.
            if let Ok(next) = reply {
                self.snapshot = Some(next);
            }
        }

        for event in events {
            if let Event::ConfigChanged(next) = event
                && self.pending == 0
            {
                self.snapshot = Some(next.clone());
            }
        }
    }

    /// Whether the first snapshot has arrived. Pages only run once it has.
    pub fn loaded(&self) -> bool {
        self.snapshot.is_some()
    }

    /// The snapshot (config, issues, path).
    ///
    /// # Panics
    /// Before the first snapshot arrived; the shell shows the Boot screen until then.
    pub fn snapshot(&self) -> &ConfigSnapshot {
        self.snapshot.as_ref().expect("config not loaded yet")
    }

    pub fn config(&self) -> &Config {
        &self.snapshot().config
    }

    /// Writes one value.
    pub fn set<T: Serialize>(&mut self, path: ConfigPath, value: T) {
        match serde_json::to_value(value) {
            Ok(value) => {
                self.patch(vec![ConfigPatch { path, value: Some(value) }]);
            }
            Err(err) => self.error = Some(format!("The change was not saved: {err}")),
        }
    }

    /// Removes one key so it falls back to its default (`set(path, undefined)` in TS).
    pub fn remove(&mut self, path: ConfigPath) {
        self.patch(vec![ConfigPatch::remove(path)]);
    }

    /// Writes several values in one file update. The returned handle settles
    /// when the main process has answered; it can be dropped.
    pub fn patch(&mut self, patches: Vec<ConfigPatch>) -> Pending<()> {
        let (done, rx) = crossbeam_channel::bounded(1);
        if patches.is_empty() {
            let _ = done.send(Ok(()));
            return Pending::from_receiver(rx);
        }
        self.sequence += 1;
        let id = self.sequence;
        self.pending += 1;
        // Optimistic: patch the current config and re-validate it, so defaults are
        // filled in exactly as the main process will fill them.
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.config = patched_config(&snapshot.config, &patches);
        }
        let reply = self.link.call(Request::ConfigSet(patches));
        self.writes.push(Write { id, reply, done });
        Pending::from_receiver(rx)
    }

    /// The last write error, or the load error before the first snapshot.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn dismiss_error(&mut self) {
        self.error = None;
    }

    /// Writes in flight.
    pub fn pending(&self) -> usize {
        self.pending
    }
}
