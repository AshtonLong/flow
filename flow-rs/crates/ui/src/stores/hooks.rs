//! Small shared state. Port of `lib/hooks.ts`: app info (refreshed when the
//! window regains focus, since `paused` can change from the tray), the
//! microphone list, and a flag that turns itself off ("Copied").

use std::time::{Duration, Instant};

use flow_core::protocol::settings::Request;
use flow_core::types::AppInfo;

use crate::adapters::audio::{self, Microphone};
use crate::backend::{Link, Pending};

/// `app.info()`, refreshed when the window regains focus.
pub struct AppInfoStore {
    link: Link,
    info: Option<AppInfo>,
    loading: Option<Pending<AppInfo>>,
}

impl AppInfoStore {
    pub(crate) fn new(link: Link) -> Self {
        let mut store = Self { link, info: None, loading: None };
        store.refresh();
        store
    }

    pub fn refresh(&mut self) {
        self.loading = Some(self.link.call(Request::AppInfo));
    }

    pub(crate) fn pump(&mut self, focus_gained: bool) {
        if focus_gained {
            self.refresh();
        }
        if let Some(result) = self.loading.as_mut().and_then(Pending::take) {
            self.loading = None;
            // The pages degrade to "unknown" rather than failing.
            if let Ok(info) = result {
                self.info = Some(info);
            }
        }
    }

    /// `None` until the first reply.
    pub fn info(&self) -> Option<&AppInfo> {
        self.info.as_ref()
    }

    pub fn paused(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.paused)
    }
}

/// Connected microphones, refreshed when the window regains focus and on demand.
pub struct Microphones {
    mics: Vec<Microphone>,
}

impl Microphones {
    pub(crate) fn new() -> Self {
        Self { mics: audio::list_microphones() }
    }

    pub fn list(&self) -> &[Microphone] {
        &self.mics
    }

    pub fn refresh(&mut self) {
        self.mics = audio::list_microphones();
    }
}

/// A flag that turns itself off, for "Copied" style confirmations. Keep it in
/// the page struct; [`Flash::on`] schedules the repaint that turns it off.
#[derive(Debug, Clone)]
pub struct Flash {
    duration: Duration,
    until: Option<Instant>,
}

impl Default for Flash {
    fn default() -> Self {
        Self::new(Duration::from_millis(1600))
    }
}

impl Flash {
    pub fn new(duration: Duration) -> Self {
        Self { duration, until: None }
    }

    pub fn flash(&mut self) {
        self.until = Some(Instant::now() + self.duration);
    }

    pub fn on(&mut self, ctx: &egui::Context) -> bool {
        match self.until {
            Some(until) if Instant::now() < until => {
                ctx.request_repaint_after(until - Instant::now());
                true
            }
            Some(_) => {
                self.until = None;
                false
            }
            None => false,
        }
    }
}

/// `useDebounced`: the value, once it has stayed the same for `delay`.
#[derive(Debug, Clone)]
pub struct Debounced<T> {
    delay: Duration,
    current: T,
    latest: T,
    changed_at: Instant,
}

impl<T: Clone + PartialEq> Debounced<T> {
    pub fn new(value: T, delay: Duration) -> Self {
        Self { delay, current: value.clone(), latest: value, changed_at: Instant::now() }
    }

    /// Feeds this frame's value; returns the debounced one.
    pub fn update(&mut self, ctx: &egui::Context, value: &T) -> &T {
        if *value != self.latest {
            self.latest = value.clone();
            self.changed_at = Instant::now();
        }
        if self.latest != self.current {
            let elapsed = self.changed_at.elapsed();
            if elapsed >= self.delay {
                self.current = self.latest.clone();
            } else {
                ctx.request_repaint_after(self.delay - elapsed);
            }
        }
        &self.current
    }
}
