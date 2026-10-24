//! The model catalog store: one snapshot, live download progress, shared by
//! every page. Port of `lib/catalog.tsx`.

use std::collections::HashMap;

use flow_core::catalog::{CatalogSnapshot, DownloadProgress, DownloadState};
use flow_core::protocol::settings::{Event, Request};

use crate::backend::{Link, Pending};
use crate::util::format::error_message;

pub struct CatalogStore {
    link: Link,
    /// `None` until the first `ModelsList` resolves.
    snapshot: Option<CatalogSnapshot>,
    /// Latest progress per model id, including finished `error` states until retried.
    progress: HashMap<String, DownloadProgress>,
    error: Option<String>,
    loading: Option<Pending<CatalogSnapshot>>,
}

impl CatalogStore {
    pub(crate) fn new(link: Link) -> Self {
        let mut store = Self { link, snapshot: None, progress: HashMap::new(), error: None, loading: None };
        store.reload();
        store
    }

    /// Re-reads the catalog, e.g. after a key was stored.
    pub fn reload(&mut self) {
        self.loading = Some(self.link.call(Request::ModelsList));
    }

    /// Whether a reload is in flight.
    pub fn reloading(&self) -> bool {
        self.loading.is_some()
    }

    pub(crate) fn pump(&mut self, events: &[Event]) {
        if let Some(result) = self.loading.as_mut().and_then(Pending::take) {
            self.loading = None;
            match result {
                Ok(next) => {
                    self.snapshot = Some(next);
                    self.error = None;
                }
                Err(err) => self.error = Some(error_message(&err)),
            }
        }
        for event in events {
            match event {
                Event::ModelsChanged(next) => {
                    // A finished download shows up as `ready`; drop its progress entry.
                    self.progress.retain(|id, p| {
                        p.state == DownloadState::Error && !next.status.get(id).is_some_and(|s| s.ready)
                    });
                    self.snapshot = Some(next.clone());
                }
                Event::ModelsProgress(p) => {
                    if matches!(p.state, DownloadState::Done | DownloadState::Cancelled) {
                        self.progress.remove(&p.id);
                    } else {
                        self.progress.insert(p.id.clone(), p.clone());
                    }
                }
                _ => {}
            }
        }
    }

    pub fn snapshot(&self) -> Option<&CatalogSnapshot> {
        self.snapshot.as_ref()
    }

    pub fn progress(&self) -> &HashMap<String, DownloadProgress> {
        &self.progress
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn clear_progress(&mut self, id: &str) {
        self.progress.remove(id);
    }

    /// The download in flight for a model, from live progress or the snapshot.
    pub fn download_of(&self, id: &str) -> Option<&DownloadProgress> {
        if let Some(live) = self.progress.get(id) {
            return Some(live);
        }
        self.snapshot
            .as_ref()?
            .status
            .get(id)?
            .download
            .as_ref()
            .filter(|d| matches!(d.state, DownloadState::Downloading | DownloadState::Verifying))
    }
}
