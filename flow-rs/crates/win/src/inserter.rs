//! Places a transcript at the cursor: clipboard paste (default) or simulated
//! typing. Every path that cannot insert leaves the text on the clipboard, so a
//! dictation is never silently lost. Mirrors `src/main/insert/inserter.ts`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use flow_core::config::InsertMethod;
use flow_core::hotkeys::{SIDED_MODIFIERS, normalize_binding};
use flow_core::types::{FocusedApp, InsertFailure, InsertOutcome, InsertResult};
use parking_lot::{Condvar, Mutex};
use tokio_util::sync::CancellationToken;

use crate::clipboard::{Clipboard, ClipboardEntry};
use crate::platform::{Platform, PlatformError};

/// How long to wait for Ctrl/Alt/Shift/Win to be released before inserting. A
/// held modifier would turn the paste into another shortcut (Ctrl+Win+V), and
/// the user may well be holding the hotkey for their next dictation, so this is
/// long: the text arrives late but in order. After it, the clipboard is used.
pub const MODIFIER_WAIT: Duration = Duration::from_millis(30_000);
/// After bringing the target window back to the foreground, before sending keys.
pub const FOCUS_SETTLE: Duration = Duration::from_millis(60);
/// After writing the clipboard, before pasting: clipboard listeners run first.
pub const CLIPBOARD_SETTLE: Duration = Duration::from_millis(30);
/// After the paste shortcut, before restoring: the target app reads the clipboard asynchronously.
pub const PASTE_READ: Duration = Duration::from_millis(150);

const DEFAULT_PASTE_SHORTCUT: &str = "Ctrl+V";

#[derive(Debug, Clone)]
pub struct InsertOptions {
    pub method: InsertMethod,
    /// For example `Ctrl+V`, or `Ctrl+Shift+V` in a terminal.
    pub paste_shortcut: String,
    pub restore_clipboard: bool,
    /// The window that had focus at key-up.
    pub target: Option<FocusedApp>,
    /// Stops an insertion that has not sent its keys yet (or typing, mid-way).
    /// Nothing is put on the clipboard; the result of a cancelled call is meaningless.
    pub cancel: Option<CancellationToken>,
}

/// Sleeps for a duration; injectable so tests run fast.
pub type Sleep = Arc<dyn Fn(Duration) + Send + Sync>;
/// A second way to send the paste shortcut when the platform cannot.
pub type TapShortcut = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone, Default)]
pub struct InserterOptions {
    /// Fallback shortcut sender used when the platform cannot send input.
    pub tap_shortcut: Option<TapShortcut>,
    /// Defaults to `std::thread::sleep`.
    pub sleep: Option<Sleep>,
}

/// The clipboard as it was before the paste, read out in full.
struct Snapshot {
    entries: Vec<ClipboardEntry>,
    /// Nothing was on the clipboard at all.
    empty: bool,
    /// Set when plain text was the only content: restored without touching clipboard history.
    text_only: Option<String>,
}

fn same_text(a: &str, b: &str) -> bool {
    a.replace("\r\n", "\n") == b.replace("\r\n", "\n")
}

/// Hands out turns in call order (a ticket lock), so back-to-back calls insert in order.
#[derive(Default)]
struct Queue {
    next: AtomicU64,
    serving: Mutex<u64>,
    turn: Condvar,
}

struct Turn<'a>(&'a Queue);

impl Queue {
    fn enter(&self) -> Turn<'_> {
        let ticket = self.next.fetch_add(1, Ordering::SeqCst);
        let mut serving = self.serving.lock();
        while *serving != ticket {
            self.turn.wait(&mut serving);
        }
        Turn(self)
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        *self.0.serving.lock() += 1;
        self.0.turn.notify_all();
    }
}

/// Why an insertion attempt stopped early; all of them leave the text on the clipboard.
enum Failure {
    Platform,
    Clipboard,
}

impl From<PlatformError> for Failure {
    fn from(_: PlatformError) -> Self {
        Self::Platform
    }
}

pub struct Inserter {
    platform: Arc<dyn Platform>,
    clipboard: Arc<dyn Clipboard>,
    tap_shortcut: Option<TapShortcut>,
    sleep: Sleep,
    queue: Queue,
}

impl Inserter {
    pub fn new(platform: Arc<dyn Platform>, clipboard: Arc<dyn Clipboard>) -> Self {
        Self::with_options(platform, clipboard, InserterOptions::default())
    }

    pub fn with_options(platform: Arc<dyn Platform>, clipboard: Arc<dyn Clipboard>, options: InserterOptions) -> Self {
        Self {
            platform,
            clipboard,
            tap_shortcut: options.tap_shortcut,
            sleep: options.sleep.unwrap_or_else(|| Arc::new(std::thread::sleep)),
            queue: Queue::default(),
        }
    }

    /// Places text at the cursor. Blocking; calls are serialised so back-to-back
    /// dictations insert in order. Never panics.
    pub fn insert(&self, text: &str, options: &InsertOptions) -> InsertResult {
        let _turn = self.queue.enter();
        self.run(text, options)
    }

    /// Writes text to the clipboard ("copy last transcript"). Never fails or panics.
    pub fn copy(&self, text: &str) {
        self.write_text(text);
    }

    fn cancelled(options: &InsertOptions) -> bool {
        options.cancel.as_ref().is_some_and(CancellationToken::is_cancelled)
    }

    fn run(&self, text: &str, options: &InsertOptions) -> InsertResult {
        if Self::cancelled(options) {
            return InsertResult::clipboard(InsertFailure::Failed);
        }
        if text.is_empty() {
            return InsertResult::ok(match options.method {
                InsertMethod::Type => InsertOutcome::Typed,
                InsertMethod::Paste => InsertOutcome::Pasted,
            });
        }
        match catch_unwind(AssertUnwindSafe(|| self.attempt(text, options))) {
            Ok(Ok(result)) => result,
            Ok(Err(_)) | Err(_) => self.to_clipboard(text, InsertFailure::Failed),
        }
    }

    fn attempt(&self, text: &str, options: &InsertOptions) -> Result<InsertResult, Failure> {
        let platform = &self.platform;
        let cancel = options.cancel.as_ref();
        let aborted = InsertResult::clipboard(InsertFailure::Failed);

        if let Some(blocked) = self.blocked_reason(options.target.as_ref()) {
            return Ok(self.to_clipboard(text, blocked));
        }

        platform.wait_for_modifiers_up(MODIFIER_WAIT, cancel);
        if Self::cancelled(options) {
            return Ok(aborted);
        }
        if SIDED_MODIFIERS.iter().any(|key| platform.is_key_down(key) == Some(true)) {
            return Ok(self.to_clipboard(text, InsertFailure::Failed));
        }
        // Focus may have moved while transcribing, or while waiting just now.
        if !self.focus_target(options.target.as_ref()) {
            return Ok(self.to_clipboard(text, InsertFailure::TargetClosed));
        }
        if Self::cancelled(options) {
            return Ok(aborted);
        }

        if options.method == InsertMethod::Type && platform.available() {
            platform.type_text(text, cancel)?;
            return Ok(InsertResult::ok(InsertOutcome::Typed));
        }
        self.paste(text, options)
    }

    /// Why the text cannot be inserted at all, judged before anything is touched.
    fn blocked_reason(&self, target: Option<&FocusedApp>) -> Option<InsertFailure> {
        let platform = &self.platform;
        // Without the platform nothing is known about windows: paste into whatever has focus.
        if !platform.available() {
            return None;
        }
        let Some(target) = target.filter(|t| platform.is_text_target(Some(t))) else {
            return Some(InsertFailure::NoTarget);
        };
        if !platform.is_window_alive(&target.window_id) {
            return Some(InsertFailure::TargetClosed);
        }
        // Windows drops simulated input aimed at a higher-privileged window.
        if target.elevated && !platform.self_elevated() {
            return Some(InsertFailure::Elevated);
        }
        None
    }

    fn focus_target(&self, target: Option<&FocusedApp>) -> bool {
        let platform = &self.platform;
        let Some(target) = target.filter(|_| platform.available()) else { return true };
        let focused = || platform.get_focused_app().is_some_and(|app| app.window_id == target.window_id);
        if focused() {
            return true;
        }
        if !platform.is_window_alive(&target.window_id) {
            return false;
        }
        if !platform.focus_window(&target.window_id) {
            return false;
        }
        (self.sleep)(FOCUS_SETTLE);
        focused()
    }

    fn paste(&self, text: &str, options: &InsertOptions) -> Result<InsertResult, Failure> {
        let platform = &self.platform;
        // Read out in full before the clipboard is overwritten.
        let saved = if options.restore_clipboard { self.snapshot() } else { None };
        if !platform.write_clipboard_excluded(text) {
            self.clipboard.write_text(text).map_err(|_| Failure::Clipboard)?;
        }
        (self.sleep)(CLIPBOARD_SETTLE);
        if Self::cancelled(options) {
            if let Some(saved) = &saved {
                self.restore(saved, None);
            }
            return Ok(InsertResult::clipboard(InsertFailure::Failed));
        }

        let shortcut = normalize_binding(&options.paste_shortcut).unwrap_or_else(|| DEFAULT_PASTE_SHORTCUT.into());
        let sent = platform.send_shortcut(&shortcut) || self.tap_shortcut.as_ref().is_some_and(|tap| tap(&shortcut));
        // No key was pressed, so the transcript stays on the clipboard for a manual paste.
        if !sent {
            return Ok(InsertResult::clipboard(InsertFailure::Failed));
        }

        (self.sleep)(PASTE_READ);
        if let Some(saved) = &saved {
            self.restore(saved, Some(text));
        }
        Ok(InsertResult::ok(InsertOutcome::Pasted))
    }

    fn to_clipboard(&self, text: &str, reason: InsertFailure) -> InsertResult {
        self.write_text(text);
        InsertResult::clipboard(reason)
    }

    /// A plain write: text the user is meant to find on the clipboard belongs in its history.
    fn write_text(&self, text: &str) {
        // Nothing more can be done if this fails; the caller still has the transcript.
        let _ = catch_unwind(AssertUnwindSafe(|| self.clipboard.write_text(text)));
    }

    /// `None` when the clipboard cannot be read at all; it is then left holding the transcript.
    fn snapshot(&self) -> Option<Snapshot> {
        let read = self.clipboard.read().ok()?;
        let plain = read.entries.iter().find(|e| e.is_text());
        let only_text = plain.is_some()
            && read.entries.iter().all(|e| e.is_text() || e.is_text_companion() || e.is_exclusion_marker());
        let text_only = if only_text { plain.and_then(ClipboardEntry::as_text) } else { None };
        Some(Snapshot { entries: read.entries, empty: read.empty, text_only })
    }

    /// Puts the saved content back. With `ours` given, only if the clipboard
    /// still holds that text: the user may have copied something meanwhile.
    fn restore(&self, saved: &Snapshot, ours: Option<&str>) {
        // The paste already happened; a failed restore is not worth reporting.
        let _ = catch_unwind(AssertUnwindSafe(|| self.try_restore(saved, ours)));
    }

    fn try_restore(&self, saved: &Snapshot, ours: Option<&str>) {
        let clipboard = &self.clipboard;
        if let Some(ours) = ours {
            match clipboard.read_text() {
                Ok(current) if same_text(&current, ours) => {}
                _ => return,
            }
        }
        if saved.empty {
            clipboard.clear();
            return;
        }
        if let Some(text) = &saved.text_only {
            // Already in the clipboard history from when it was first copied; do not add it again.
            if !self.platform.write_clipboard_excluded(text) {
                let _ = clipboard.write_text(text);
            }
            return;
        }
        // Everything first; then without raw OS formats, which cannot all be written back.
        let keep_all = |e: &ClipboardEntry| !e.is_ole();
        let keep_standard = |e: &ClipboardEntry| !e.is_raw();
        for keep in [&keep_all as &dyn Fn(&ClipboardEntry) -> bool, &keep_standard] {
            if self.write_entries(saved, keep) {
                return;
            }
        }
        if let Some(text) = saved.entries.iter().find_map(ClipboardEntry::as_text) {
            let _ = clipboard.write_text(&text);
        }
        // Otherwise nothing could be put back: the transcript stays on the clipboard.
    }

    fn write_entries(&self, saved: &Snapshot, keep: &dyn Fn(&ClipboardEntry) -> bool) -> bool {
        let entries: Vec<ClipboardEntry> = saved.entries.iter().filter(|e| keep(e)).cloned().collect();
        if entries.is_empty() {
            return false;
        }
        self.clipboard.write(&entries).is_ok()
    }

    /// How many calls have taken a turn so far.
    #[cfg(test)]
    fn tickets_issued(&self) -> u64 {
        self.queue.next.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::time::Instant;

    use super::*;
    use crate::clipboard::{CF_LOCALE, CF_UNICODETEXT, ClipboardError, ClipboardSnapshot, EXCLUSION_FORMATS};

    // ── Clipboard formats, as the Win32 clipboard names them ─────────────────

    const TEXT: &str = "text/plain";
    const HTML: &str = "HTML Format";
    const PNG: &str = "PNG";

    /// Registered format ids for the fake clipboard: stable per name.
    fn format_id(name: &str) -> u32 {
        let hash = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
        0xc000 + hash % 0x3000
    }

    /// One format: `TEXT` is `CF_UNICODETEXT`, anything else a registered format of that name.
    /// Strings stand in for any bytes.
    fn entry(kind: &str, content: &str) -> ClipboardEntry {
        if kind == TEXT {
            ClipboardEntry::text(content)
        } else {
            ClipboardEntry::new(format_id(kind), Some(kind.to_string()), content.as_bytes().to_vec())
        }
    }

    fn label(entry: &ClipboardEntry) -> String {
        match entry.format {
            CF_UNICODETEXT => TEXT.to_string(),
            CF_LOCALE => "locale".to_string(),
            _ => entry.name.clone().unwrap_or_else(|| entry.format.to_string()),
        }
    }

    /// The clipboard's content as `(format, content)` pairs.
    type View = Vec<(String, String)>;

    fn view(entries: &[ClipboardEntry]) -> View {
        entries
            .iter()
            .map(|e| (label(e), e.as_text().unwrap_or_else(|| String::from_utf8_lossy(&e.data).into_owned())))
            .collect()
    }

    fn data(pairs: &[(&str, &str)]) -> View {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    type FormatFilter = Box<dyn Fn(&str) -> bool + Send>;
    type SleepHook = Arc<Mutex<Option<Box<dyn Fn(u64) + Send>>>>;

    #[derive(Default)]
    struct ClipState {
        entries: Vec<ClipboardEntry>,
        log: Vec<String>,
        /// Formats `read` cannot copy out (a GDI handle, a format that went away).
        unreadable: HashSet<String>,
        /// `write` fails when given one of these formats.
        unwritable: Option<FormatFilter>,
        fail_read: bool,
        fail_read_text: bool,
        fail_write: bool,
        fail_write_text: bool,
    }

    /// Mimics the Win32 clipboard: one set of formats, replaced as a whole.
    #[derive(Default)]
    struct FakeClipboard {
        state: Mutex<ClipState>,
    }

    impl FakeClipboard {
        fn set(&self, pairs: &[(&str, &str)]) {
            self.state.lock().entries = pairs.iter().map(|(k, v)| entry(k, v)).collect();
        }
        fn items(&self) -> View {
            view(&self.state.lock().entries)
        }
        fn text(&self) -> String {
            self.state.lock().entries.iter().find_map(ClipboardEntry::as_text).unwrap_or_default()
        }
        fn log(&self) -> Vec<String> {
            self.state.lock().log.clone()
        }
    }

    impl Clipboard for FakeClipboard {
        fn read(&self) -> Result<ClipboardSnapshot, ClipboardError> {
            let state = self.state.lock();
            if state.fail_read {
                return Err(ClipboardError::Busy);
            }
            let entries = state.entries.iter().filter(|e| !state.unreadable.contains(&label(e))).cloned().collect();
            Ok(ClipboardSnapshot { entries, empty: state.entries.is_empty() })
        }
        fn read_text(&self) -> Result<String, ClipboardError> {
            if self.state.lock().fail_read_text {
                return Err(ClipboardError::Busy);
            }
            Ok(self.text())
        }
        fn write(&self, entries: &[ClipboardEntry]) -> Result<(), ClipboardError> {
            let mut state = self.state.lock();
            if state.fail_write {
                return Err(ClipboardError::Busy);
            }
            if let Some(unwritable) = &state.unwritable
                && let Some(bad) = entries.iter().find(|e| unwritable(&label(e)))
            {
                return Err(ClipboardError::Failed(format!("cannot write {}", label(bad))));
            }
            state.log.push(format!("write:{}", entries.iter().map(label).collect::<Vec<_>>().join(",")));
            state.entries = entries.to_vec();
            Ok(())
        }
        fn write_text(&self, text: &str) -> Result<(), ClipboardError> {
            let mut state = self.state.lock();
            if state.fail_write_text {
                return Err(ClipboardError::Busy);
            }
            state.log.push(format!("writeText:{text}"));
            state.entries = vec![ClipboardEntry::text(text)];
            Ok(())
        }
        fn clear(&self) {
            let mut state = self.state.lock();
            state.log.push("clear".into());
            state.entries.clear();
        }
    }

    // ── The platform ─────────────────────────────────────────────────────────

    fn app(window_id: &str, process_name: &str, title: &str, pid: u32) -> FocusedApp {
        FocusedApp {
            window_id: window_id.into(),
            process_name: process_name.into(),
            title: title.into(),
            pid,
            elevated: false,
        }
    }

    fn notepad() -> FocusedApp {
        app("100", "notepad.exe", "Untitled - Notepad", 1)
    }

    fn other() -> FocusedApp {
        app("200", "chrome.exe", "Tab", 2)
    }

    /// Everything that happened, shared by the fakes.
    #[derive(Default)]
    struct Log {
        /// What the simulated target app received.
        received: Mutex<Vec<String>>,
        /// Everything that happened, in order.
        trace: Mutex<Vec<String>>,
        sleeps: Mutex<Vec<u64>>,
    }

    struct PlatState {
        available: bool,
        self_elevated: bool,
        windows: HashMap<String, FocusedApp>,
        foreground: Option<String>,
        shell_windows: HashSet<String>,
        can_focus: bool,
        can_exclude: bool,
        can_send: bool,
        held_modifiers: HashSet<String>,
        release_during_wait: bool,
        type_error: bool,
        send_panics: bool,
        /// The cancel token `type_text` was given.
        type_cancel: Option<CancellationToken>,
    }

    type Hook = Box<dyn Fn(&FakePlatform) + Send + Sync>;

    struct FakePlatform {
        log: Arc<Log>,
        clipboard: Arc<FakeClipboard>,
        state: Mutex<PlatState>,
        /// Runs at the end of `wait_for_modifiers_up`.
        after_wait: Mutex<Option<Hook>>,
    }

    impl FakePlatform {
        fn new(log: Arc<Log>, clipboard: Arc<FakeClipboard>) -> Self {
            let windows = [notepad(), other()].into_iter().map(|a| (a.window_id.clone(), a)).collect();
            Self {
                log,
                clipboard,
                state: Mutex::new(PlatState {
                    available: true,
                    self_elevated: false,
                    windows,
                    foreground: Some(notepad().window_id),
                    shell_windows: HashSet::new(),
                    can_focus: true,
                    can_exclude: true,
                    can_send: true,
                    held_modifiers: HashSet::new(),
                    release_during_wait: true,
                    type_error: false,
                    send_panics: false,
                    type_cancel: None,
                }),
                after_wait: Mutex::new(None),
            }
        }
        fn trace(&self, line: String) {
            self.log.trace.lock().push(line);
        }
        fn foreground(&self) -> String {
            self.state.lock().foreground.clone().unwrap_or_else(|| "null".into())
        }
    }

    impl Platform for FakePlatform {
        fn available(&self) -> bool {
            self.state.lock().available
        }
        fn self_elevated(&self) -> bool {
            self.state.lock().self_elevated
        }
        fn get_focused_app(&self) -> Option<FocusedApp> {
            let state = self.state.lock();
            state.foreground.as_ref().and_then(|id| state.windows.get(id).cloned())
        }
        fn is_window_alive(&self, window_id: &str) -> bool {
            self.state.lock().windows.contains_key(window_id)
        }
        fn focus_window(&self, window_id: &str) -> bool {
            self.trace(format!("focus:{window_id}"));
            let mut state = self.state.lock();
            if !state.can_focus || !state.windows.contains_key(window_id) {
                return false;
            }
            state.foreground = Some(window_id.to_string());
            true
        }
        fn is_text_target(&self, app: Option<&FocusedApp>) -> bool {
            app.is_some_and(|app| !self.state.lock().shell_windows.contains(&app.window_id))
        }
        fn type_text(&self, text: &str, cancel: Option<&CancellationToken>) -> Result<(), PlatformError> {
            self.trace(format!("type:{text}"));
            self.state.lock().type_cancel = cancel.cloned();
            if self.state.lock().type_error {
                return Err(PlatformError::Blocked);
            }
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Ok(());
            }
            let foreground = self.foreground();
            self.log.received.lock().push(format!("{foreground}:{text}"));
            Ok(())
        }
        fn send_shortcut(&self, binding: &str) -> bool {
            self.trace(format!("shortcut:{binding}"));
            if self.state.lock().send_panics {
                panic!("SendInput exploded");
            }
            if !self.state.lock().can_send {
                return false;
            }
            // The target app reads whatever is on the clipboard at this moment.
            let line = format!("{}:{}", self.foreground(), self.clipboard.text());
            self.log.received.lock().push(line);
            true
        }
        fn wait_for_modifiers_up(&self, timeout: Duration, _cancel: Option<&CancellationToken>) {
            self.trace(format!("wait:{}", timeout.as_millis()));
            {
                let mut state = self.state.lock();
                if state.release_during_wait {
                    state.held_modifiers.clear();
                }
            }
            if let Some(hook) = self.after_wait.lock().as_ref() {
                hook(self);
            }
        }
        fn is_key_down(&self, key: &str) -> Option<bool> {
            Some(self.state.lock().held_modifiers.contains(key))
        }
        fn mask_start_menu(&self) {}
        /// Like the real one: plain text plus the three history-exclusion marker formats.
        fn write_clipboard_excluded(&self, text: &str) -> bool {
            if !self.state.lock().can_exclude {
                return false;
            }
            let mut state = self.clipboard.state.lock();
            state.log.push(format!("excluded:{text}"));
            let mut entries = vec![ClipboardEntry::text(text)];
            entries.extend(EXCLUSION_FORMATS.iter().map(|name| entry(name, "\0\0\0\0")));
            state.entries = entries;
            true
        }
    }

    // ── The world ────────────────────────────────────────────────────────────

    struct World {
        log: Arc<Log>,
        clipboard: Arc<FakeClipboard>,
        platform: Arc<FakePlatform>,
        inserter: Inserter,
        /// Runs at the start of every sleep, with the duration in milliseconds.
        on_sleep: SleepHook,
    }

    impl World {
        fn received(&self) -> Vec<String> {
            self.log.received.lock().clone()
        }
        fn trace(&self) -> Vec<String> {
            self.log.trace.lock().clone()
        }
        fn sleeps(&self) -> Vec<u64> {
            self.log.sleeps.lock().clone()
        }
        fn plat(&self) -> parking_lot::MutexGuard<'_, PlatState> {
            self.platform.state.lock()
        }
        fn clip(&self) -> parking_lot::MutexGuard<'_, ClipState> {
            self.clipboard.state.lock()
        }
        fn on_sleep(&self, hook: impl Fn(u64) + Send + 'static) {
            *self.on_sleep.lock() = Some(Box::new(hook));
        }
        fn insert(&self, text: &str, options: InsertOptions) -> InsertResult {
            self.inserter.insert(text, &options)
        }
    }

    enum Tap {
        /// Records `tap:<binding>` and pastes like a real key press.
        Default,
        Fails,
        Missing,
    }

    fn world(tap: Tap, real_sleep: bool) -> World {
        let log = Arc::new(Log::default());
        let clipboard = Arc::new(FakeClipboard::default());
        let platform = Arc::new(FakePlatform::new(log.clone(), clipboard.clone()));
        let on_sleep: SleepHook = Arc::new(Mutex::new(None));
        let tap_shortcut: Option<TapShortcut> = match tap {
            Tap::Default => {
                let (log, clipboard, platform) = (log.clone(), clipboard.clone(), platform.clone());
                Some(Arc::new(move |binding: &str| {
                    log.trace.lock().push(format!("tap:{binding}"));
                    log.received.lock().push(format!("{}:{}", platform.foreground(), clipboard.text()));
                    true
                }))
            }
            Tap::Fails => Some(Arc::new(|_: &str| false)),
            Tap::Missing => None,
        };
        let sleep: Sleep = if real_sleep {
            Arc::new(|d: Duration| std::thread::sleep(d / 10))
        } else {
            let (log, on_sleep) = (log.clone(), on_sleep.clone());
            Arc::new(move |d: Duration| {
                let ms = d.as_millis() as u64;
                log.sleeps.lock().push(ms);
                if let Some(hook) = on_sleep.lock().as_ref() {
                    hook(ms);
                }
            })
        };
        let inserter = Inserter::with_options(
            platform.clone(),
            clipboard.clone(),
            InserterOptions { tap_shortcut, sleep: Some(sleep) },
        );
        World { log, clipboard, platform, inserter, on_sleep }
    }

    fn setup() -> World {
        world(Tap::Default, false)
    }

    fn paste() -> InsertOptions {
        InsertOptions {
            method: InsertMethod::Paste,
            paste_shortcut: "Ctrl+V".into(),
            restore_clipboard: true,
            target: Some(notepad()),
            cancel: None,
        }
    }

    fn type_() -> InsertOptions {
        InsertOptions { method: InsertMethod::Type, ..paste() }
    }

    fn pasted() -> InsertResult {
        InsertResult::ok(InsertOutcome::Pasted)
    }

    fn typed() -> InsertResult {
        InsertResult::ok(InsertOutcome::Typed)
    }

    fn to_clipboard(reason: InsertFailure) -> InsertResult {
        InsertResult::clipboard(reason)
    }

    fn wait_trace() -> String {
        format!("wait:{}", MODIFIER_WAIT.as_millis())
    }

    fn ms(d: Duration) -> u64 {
        d.as_millis() as u64
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    mod paste {
        use super::*;

        #[test]
        fn pastes_through_the_clipboard_and_restores_what_was_there() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello world", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.received(), strings(&["100:hello world"]));
            assert_eq!(w.clipboard.text(), "previous");
            assert_eq!(w.trace(), vec![wait_trace(), "shortcut:Ctrl+V".into()]);
            assert_eq!(w.sleeps(), vec![ms(CLIPBOARD_SETTLE), ms(PASTE_READ)]);
        }

        #[test]
        fn keeps_the_transcript_and_the_restored_text_out_of_clipboard_history() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous")]);
            w.insert("hello", paste());
            assert_eq!(w.clipboard.log(), strings(&["excluded:hello", "excluded:previous"]));
        }

        #[test]
        fn treats_text_that_was_itself_written_excluded_as_text_only() {
            let w = setup();
            w.platform.write_clipboard_excluded("previous");
            w.clip().log.clear();
            w.insert("hello", paste());
            assert_eq!(w.clipboard.log(), strings(&["excluded:hello", "excluded:previous"]));
            assert_eq!(w.clipboard.text(), "previous");
        }

        #[test]
        fn treats_text_with_its_locale_as_text_only() {
            let w = setup();
            let locale = ClipboardEntry::new(CF_LOCALE, None, vec![9, 4, 0, 0]);
            w.clip().entries = vec![ClipboardEntry::text("previous"), locale];
            w.insert("hello", paste());
            assert_eq!(w.clipboard.log(), strings(&["excluded:hello", "excluded:previous"]));
        }

        #[test]
        fn falls_back_to_a_plain_clipboard_write_when_exclusion_is_unavailable() {
            let w = setup();
            w.plat().can_exclude = false;
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.received(), strings(&["100:hello"]));
            assert_eq!(w.clipboard.log(), strings(&["writeText:hello", "writeText:previous"]));
        }

        #[test]
        fn restores_rich_content_with_every_format_it_had() {
            let w = setup();
            let original = [(TEXT, "plain"), (HTML, "<b>plain</b>"), (PNG, "PNG-BYTES")];
            w.clipboard.set(&original);
            w.insert("hello", paste());
            assert_eq!(w.received(), strings(&["100:hello"]));
            assert_eq!(w.clipboard.items(), data(&original));
            assert_eq!(w.clipboard.log().last().cloned(), Some(format!("write:{TEXT},{HTML},{PNG}")));
        }

        #[test]
        fn restores_an_image_only_clipboard() {
            let w = setup();
            w.clipboard.set(&[(PNG, "PNG-BYTES")]);
            w.insert("hello", paste());
            assert_eq!(w.received(), strings(&["100:hello"]));
            assert_eq!(w.clipboard.items(), data(&[(PNG, "PNG-BYTES")]));
        }

        #[test]
        fn restores_an_html_only_clipboard() {
            let w = setup();
            w.clipboard.set(&[(HTML, "<i>only html</i>")]);
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), data(&[(HTML, "<i>only html</i>")]));
        }

        #[test]
        fn restores_raw_os_formats_along_with_the_content() {
            let w = setup();
            let original = [(HTML, "<b>x</b>"), ("Custom App Format", "app-data")];
            w.clipboard.set(&original);
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), data(&original));
        }

        #[test]
        fn reads_the_clipboard_out_in_full_before_overwriting_it() {
            let w = setup();
            // A snapshot that kept references instead of the data would restore "hello".
            w.clipboard.set(&[(TEXT, "previous"), (HTML, "<b>previous</b>")]);
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), data(&[(TEXT, "previous"), (HTML, "<b>previous</b>")]));
        }

        #[test]
        fn skips_a_format_that_cannot_be_read_and_restores_the_rest() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "plain"), (HTML, "<b>plain</b>"), (PNG, "PNG-BYTES")]);
            w.clip().unreadable.insert(PNG.into());
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.clipboard.items(), data(&[(TEXT, "plain"), (HTML, "<b>plain</b>")]));
        }

        #[test]
        fn skips_formats_that_cannot_be_copied_gdi_handles() {
            let w = setup();
            let bitmap = "bitmap handle";
            w.clipboard.set(&[(TEXT, "https://example.com"), (bitmap, "x")]);
            w.clip().unreadable.insert(bitmap.into());
            w.insert("hello", paste());
            // Only plain text was saved, so it is restored as text.
            assert_eq!(w.clipboard.text(), "https://example.com");
            assert_eq!(w.clipboard.log().last().cloned(), Some("excluded:https://example.com".into()));
        }

        #[test]
        fn never_writes_ole_bookkeeping_formats_back() {
            let w = setup();
            w.clipboard.set(&[(PNG, "PNG-BYTES"), ("DataObject", "hwnd"), ("Ole Private Data", "ptr")]);
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), data(&[(PNG, "PNG-BYTES")]));
        }

        #[test]
        fn retries_without_raw_os_formats_when_the_full_restore_is_rejected() {
            let w = setup();
            w.clipboard.set(&[(HTML, "<b>x</b>"), (PNG, "PNG-BYTES"), ("Weird", "data")]);
            w.clip().unwritable = Some(Box::new(|kind| kind == "Weird"));
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), data(&[(HTML, "<b>x</b>"), (PNG, "PNG-BYTES")]));
        }

        #[test]
        fn falls_back_to_the_plain_text_when_no_format_can_be_written() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "plain"), (HTML, "<b>plain</b>")]);
            w.clip().unwritable = Some(Box::new(|_| true));
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.clipboard.items(), data(&[(TEXT, "plain")]));
        }

        #[test]
        fn leaves_the_transcript_on_the_clipboard_when_nothing_can_be_restored() {
            let w = setup();
            w.clipboard.set(&[(PNG, "PNG-BYTES")]);
            w.clip().unreadable.insert(PNG.into());
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            // The original was not empty, so the clipboard is not cleared either.
            assert_eq!(w.clipboard.text(), "hello");
            assert!(!w.clipboard.log().contains(&"clear".to_string()));
        }

        #[test]
        fn leaves_the_transcript_on_the_clipboard_when_the_clipboard_cannot_be_read() {
            let w = setup();
            w.clipboard.set(&[(PNG, "PNG-BYTES")]);
            w.clip().fail_read = true;
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.received(), strings(&["100:hello"]));
            assert_eq!(w.clipboard.text(), "hello");
        }

        #[test]
        fn clears_the_clipboard_when_it_was_empty_before() {
            let w = setup();
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), View::new());
            assert_eq!(w.clipboard.log().last().cloned(), Some("clear".into()));
        }

        #[test]
        fn leaves_the_transcript_on_the_clipboard_when_restore_is_off() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello", InsertOptions { restore_clipboard: false, ..paste() });
            assert_eq!(result, pasted());
            assert_eq!(w.clipboard.text(), "hello");
        }

        #[test]
        fn does_not_restore_when_the_user_copied_something_else_meanwhile() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous"), (HTML, "<b>previous</b>")]);
            let clipboard = w.clipboard.clone();
            w.on_sleep(move |ms| {
                if ms == PASTE_READ.as_millis() as u64 {
                    clipboard.set(&[(TEXT, "copied by the user")]);
                }
            });
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), data(&[(TEXT, "copied by the user")]));
        }

        #[test]
        fn does_not_restore_when_the_user_copied_an_image_meanwhile() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous")]);
            let clipboard = w.clipboard.clone();
            w.on_sleep(move |ms| {
                if ms == PASTE_READ.as_millis() as u64 {
                    clipboard.set(&[(PNG, "screenshot")]);
                }
            });
            w.insert("hello", paste());
            assert_eq!(w.clipboard.items(), data(&[(PNG, "screenshot")]));
        }

        #[test]
        fn still_restores_when_the_clipboard_reports_the_text_with_crlf_line_endings() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous")]);
            let clipboard = w.clipboard.clone();
            w.on_sleep(move |ms| {
                if ms == PASTE_READ.as_millis() as u64 {
                    clipboard.set(&[(TEXT, "line one\r\nline two")]);
                }
            });
            w.insert("line one\nline two", paste());
            assert_eq!(w.clipboard.text(), "previous");
        }

        #[test]
        fn uses_the_configured_paste_shortcut_normalised() {
            let w = setup();
            w.insert("ls -la", InsertOptions { paste_shortcut: "shift+ctrl+v".into(), ..paste() });
            assert!(w.trace().contains(&"shortcut:Ctrl+Shift+V".to_string()));
        }

        #[test]
        fn falls_back_to_ctrl_v_when_the_configured_shortcut_does_not_parse() {
            let w = setup();
            let result = w.insert("hello", InsertOptions { paste_shortcut: "Ctrl+Nope".into(), ..paste() });
            assert_eq!(result, pasted());
            assert!(w.trace().contains(&"shortcut:Ctrl+V".to_string()));
        }

        #[test]
        fn falls_back_to_tap_shortcut_when_the_platform_cannot_send_input() {
            let w = setup();
            w.plat().can_send = false;
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.trace(), vec![wait_trace(), "shortcut:Ctrl+V".into(), "tap:Ctrl+V".into()]);
            assert_eq!(w.received(), strings(&["100:hello"]));
            assert_eq!(w.clipboard.text(), "previous");
        }

        #[test]
        fn leaves_the_text_on_the_clipboard_when_no_shortcut_could_be_sent() {
            let w = world(Tap::Fails, false);
            w.plat().can_send = false;
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::Failed));
            assert_eq!(w.clipboard.text(), "hello");
            assert_eq!(w.received(), Vec::<String>::new());
        }

        #[test]
        fn works_without_a_tap_shortcut_fallback() {
            let w = world(Tap::Missing, false);
            w.plat().can_send = false;
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::Failed));
            assert_eq!(w.clipboard.text(), "hello");
        }

        #[test]
        fn waits_for_held_modifiers_to_be_released_before_pasting() {
            let w = setup();
            w.plat().held_modifiers = ["LAlt", "LShift"].into_iter().map(String::from).collect();
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.trace()[0], wait_trace());
        }

        #[test]
        fn uses_the_clipboard_when_a_modifier_is_still_held_after_the_wait() {
            let w = setup();
            w.plat().held_modifiers = ["LWin"].into_iter().map(String::from).collect();
            w.plat().release_during_wait = false;
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello", paste());
            // Ctrl+V with Win held would be Ctrl+Win+V: never send it.
            assert_eq!(result, to_clipboard(InsertFailure::Failed));
            assert_eq!(w.received(), Vec::<String>::new());
            assert_eq!(w.trace(), vec![wait_trace()]);
            assert_eq!(w.clipboard.text(), "hello");
        }

        #[test]
        fn does_nothing_for_empty_text() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous")]);
            assert_eq!(w.insert("", paste()), pasted());
            assert_eq!(w.trace(), Vec::<String>::new());
            assert_eq!(w.clipboard.log(), Vec::<String>::new());
        }
    }

    mod target_checks {
        use super::*;

        #[test]
        fn uses_the_clipboard_when_there_is_no_target() {
            let w = setup();
            let result = w.insert("hello", InsertOptions { target: None, ..paste() });
            assert_eq!(result, to_clipboard(InsertFailure::NoTarget));
            // Already on the clipboard by the time the result is returned.
            assert_eq!(w.clipboard.text(), "hello");
            assert_eq!(w.clipboard.log(), strings(&["writeText:hello"]));
            assert_eq!(w.trace(), Vec::<String>::new());
        }

        #[test]
        fn uses_the_clipboard_when_the_target_is_the_desktop_or_taskbar() {
            let w = setup();
            w.plat().shell_windows.insert(notepad().window_id);
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::NoTarget));
            assert_eq!(w.received(), Vec::<String>::new());
        }

        #[test]
        fn uses_the_clipboard_when_the_target_window_was_closed() {
            let w = setup();
            w.plat().windows.remove(&notepad().window_id);
            w.plat().foreground = Some(other().window_id);
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::TargetClosed));
            assert_eq!(w.clipboard.text(), "hello");
            assert_eq!(w.trace(), Vec::<String>::new());
        }

        #[test]
        fn uses_the_clipboard_for_an_elevated_target() {
            let w = setup();
            let admin = FocusedApp { elevated: true, ..notepad() };
            let result = w.insert("hello", InsertOptions { target: Some(admin), ..paste() });
            assert_eq!(result, to_clipboard(InsertFailure::Elevated));
            assert_eq!(w.received(), Vec::<String>::new());
            assert_eq!(w.trace(), Vec::<String>::new());
        }

        #[test]
        fn inserts_into_an_elevated_target_when_flow_is_elevated_too() {
            let w = setup();
            w.plat().self_elevated = true;
            let admin = FocusedApp { elevated: true, ..notepad() };
            let result = w.insert("hello", InsertOptions { target: Some(admin), ..paste() });
            assert_eq!(result, pasted());
        }

        #[test]
        fn refocuses_the_target_when_focus_moved_during_transcription() {
            let w = setup();
            w.plat().foreground = Some(other().window_id);
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.received(), strings(&["100:hello"]));
            assert_eq!(w.trace(), vec![wait_trace(), "focus:100".into(), "shortcut:Ctrl+V".into()]);
            assert_eq!(w.sleeps()[0], ms(FOCUS_SETTLE));
        }

        #[test]
        fn uses_the_clipboard_when_the_target_cannot_be_refocused() {
            let w = setup();
            w.plat().foreground = Some(other().window_id);
            w.plat().can_focus = false;
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::TargetClosed));
            assert_eq!(w.received(), Vec::<String>::new());
            assert_eq!(w.clipboard.text(), "hello");
        }

        #[test]
        fn uses_the_clipboard_when_focus_does_not_stick() {
            let w = setup();
            w.plat().foreground = Some(other().window_id);
            let platform = w.platform.clone();
            w.on_sleep(move |ms| {
                if ms == FOCUS_SETTLE.as_millis() as u64 {
                    platform.state.lock().foreground = Some(other().window_id);
                }
            });
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::TargetClosed));
            assert_eq!(w.received(), Vec::<String>::new());
        }

        #[test]
        fn uses_the_clipboard_when_the_target_closes_while_waiting_for_modifiers() {
            let w = setup();
            *w.platform.after_wait.lock() = Some(Box::new(|platform: &FakePlatform| {
                let mut state = platform.state.lock();
                state.windows.remove(&notepad().window_id);
                state.foreground = Some(other().window_id);
            }));
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::TargetClosed));
            assert_eq!(w.received(), Vec::<String>::new());
        }

        #[test]
        fn refocuses_when_nothing_has_the_foreground() {
            let w = setup();
            w.plat().foreground = None;
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.received(), strings(&["100:hello"]));
        }
    }

    mod type_ {
        use super::*;

        #[test]
        fn types_the_text_and_leaves_the_clipboard_alone() {
            let w = setup();
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello", type_());
            assert_eq!(result, typed());
            assert_eq!(w.received(), strings(&["100:hello"]));
            assert_eq!(w.trace(), vec![wait_trace(), "type:hello".into()]);
            assert_eq!(w.clipboard.log(), Vec::<String>::new());
        }

        #[test]
        fn applies_the_same_target_checks() {
            let w = setup();
            assert_eq!(w.insert("a", InsertOptions { target: None, ..type_() }), to_clipboard(InsertFailure::NoTarget));
            let admin = FocusedApp { elevated: true, ..notepad() };
            assert_eq!(
                w.insert("b", InsertOptions { target: Some(admin), ..type_() }),
                to_clipboard(InsertFailure::Elevated)
            );
            w.plat().foreground = Some(other().window_id);
            assert_eq!(w.insert("c", type_()), typed());
            assert_eq!(w.received(), strings(&["100:c"]));
        }

        #[test]
        fn puts_the_text_on_the_clipboard_when_typing_fails() {
            let w = setup();
            w.plat().type_error = true;
            let result = w.insert("hello", type_());
            assert_eq!(result, to_clipboard(InsertFailure::Failed));
            assert_eq!(w.clipboard.text(), "hello");
        }

        #[test]
        fn falls_back_to_paste_when_the_platform_is_unavailable() {
            let w = setup();
            w.plat().available = false;
            w.plat().can_send = false;
            let result = w.insert("hello", type_());
            assert_eq!(result, pasted());
            assert!(w.trace().contains(&"tap:Ctrl+V".to_string()));
            assert!(!w.trace().contains(&"type:hello".to_string()));
        }
    }

    mod degraded_platform {
        use super::*;

        #[test]
        fn pastes_blind_without_a_target_through_tap_shortcut() {
            let w = setup();
            w.plat().available = false;
            w.plat().can_send = false;
            w.plat().can_exclude = false;
            w.clipboard.set(&[(TEXT, "previous")]);
            let result = w.insert("hello", InsertOptions { target: None, ..paste() });
            assert_eq!(result, pasted());
            assert_eq!(w.trace(), vec![wait_trace(), "shortcut:Ctrl+V".into(), "tap:Ctrl+V".into()]);
            assert_eq!(w.clipboard.log(), strings(&["writeText:hello", "writeText:previous"]));
        }
    }

    mod failures_and_ordering {
        use super::*;

        #[test]
        fn never_panics_any_error_puts_the_text_on_the_clipboard() {
            let w = setup();
            w.plat().send_panics = true;
            let result = w.insert("hello", paste());
            assert_eq!(result, to_clipboard(InsertFailure::Failed));
            assert_eq!(w.clipboard.text(), "hello");
        }

        #[test]
        fn survives_a_clipboard_that_cannot_be_written() {
            let w = setup();
            w.plat().can_exclude = false;
            w.clip().fail_write_text = true;
            assert_eq!(w.insert("hello", paste()), to_clipboard(InsertFailure::Failed));
            let no_target = InsertOptions { target: None, ..paste() };
            assert_eq!(w.insert("hello", no_target), to_clipboard(InsertFailure::NoTarget));
            assert_eq!(w.received(), Vec::<String>::new());
        }

        #[test]
        fn a_failed_restore_does_not_turn_a_paste_into_a_failure() {
            let w = setup();
            w.clipboard.set(&[(HTML, "<i>previous</i>")]);
            w.clip().fail_write = true;
            w.clip().fail_read_text = true;
            let result = w.insert("hello", paste());
            assert_eq!(result, pasted());
            assert_eq!(w.received(), strings(&["100:hello"]));
        }

        #[test]
        fn serialises_back_to_back_insertions_in_call_order_clipboard_work_included() {
            // Real (short) sleeps, so overlapping calls would interleave if not queued.
            let w = Arc::new(world(Tap::Default, true));
            w.clipboard.set(&[(TEXT, "previous"), (HTML, "<b>previous</b>")]);
            let calls = [("one ", paste()), ("two ", type_()), ("three", paste())];
            let mut handles = Vec::new();
            for (index, (text, options)) in calls.into_iter().enumerate() {
                let worker = w.clone();
                handles.push(std::thread::spawn(move || worker.insert(text, options)));
                // The next call is made only once this one holds its place in line.
                let deadline = Instant::now() + Duration::from_secs(5);
                while w.inserter.tickets_issued() <= index as u64 && Instant::now() < deadline {
                    std::thread::yield_now();
                }
            }
            let outcomes: Vec<InsertOutcome> =
                handles.into_iter().map(|h| h.join().expect("insert never panics").outcome).collect();
            assert_eq!(outcomes, vec![InsertOutcome::Pasted, InsertOutcome::Typed, InsertOutcome::Pasted]);
            assert_eq!(w.received(), strings(&["100:one ", "100:two ", "100:three"]));
            // Each paste saw the original clipboard, never the previous transcript.
            assert_eq!(w.clipboard.items(), data(&[(TEXT, "previous"), (HTML, "<b>previous</b>")]));
        }

        #[test]
        fn a_failed_insertion_does_not_block_the_queue() {
            let w = setup();
            w.plat().type_error = true;
            assert_eq!(w.insert("one", type_()), to_clipboard(InsertFailure::Failed));
            assert_eq!(w.insert("two", paste()), pasted());
            assert_eq!(w.received(), strings(&["100:two"]));
        }

        #[test]
        fn a_panicking_insertion_does_not_block_the_queue() {
            let w = setup();
            w.plat().send_panics = true;
            assert_eq!(w.insert("one", paste()), to_clipboard(InsertFailure::Failed));
            w.plat().send_panics = false;
            assert_eq!(w.insert("two", paste()), pasted());
            assert_eq!(w.received(), strings(&["100:two"]));
        }

        #[test]
        fn copy_writes_plain_text_to_the_clipboard_without_failing() {
            let w = setup();
            w.inserter.copy("last transcript");
            assert_eq!(w.clipboard.text(), "last transcript");
            assert_eq!(w.clipboard.log(), strings(&["writeText:last transcript"]));

            w.clip().fail_write_text = true;
            w.inserter.copy("again");
            assert_eq!(w.clipboard.text(), "last transcript");
        }
    }

    mod abort {
        use super::*;

        #[test]
        fn an_already_cancelled_call_inserts_nothing_and_leaves_the_clipboard_alone() {
            let w = setup();
            let cancel = CancellationToken::new();
            cancel.cancel();
            w.clipboard.set(&[(TEXT, "previous")]);
            w.insert("hello", InsertOptions { cancel: Some(cancel), ..paste() });
            assert_eq!(w.received(), Vec::<String>::new());
            assert_eq!(w.clipboard.log(), Vec::<String>::new());
            assert_eq!(w.clipboard.text(), "previous");
        }

        #[test]
        fn a_cancel_after_the_clipboard_was_written_restores_it_and_sends_nothing() {
            let w = setup();
            let cancel = CancellationToken::new();
            w.clipboard.set(&[(PNG, "PNG-BYTES")]);
            let trigger = cancel.clone();
            w.on_sleep(move |ms| {
                if ms == CLIPBOARD_SETTLE.as_millis() as u64 {
                    trigger.cancel();
                }
            });
            w.insert("hello", InsertOptions { cancel: Some(cancel), ..paste() });
            assert_eq!(w.received(), Vec::<String>::new());
            assert_eq!(w.clipboard.items(), data(&[(PNG, "PNG-BYTES")]));
        }

        #[test]
        fn a_cancel_while_waiting_for_modifiers_inserts_nothing() {
            let w = setup();
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            *w.platform.after_wait.lock() = Some(Box::new(move |_: &FakePlatform| trigger.cancel()));
            w.insert("hello", InsertOptions { cancel: Some(cancel), ..type_() });
            assert_eq!(w.received(), Vec::<String>::new());
            assert_eq!(w.clipboard.log(), Vec::<String>::new());
        }

        #[test]
        fn passes_the_cancel_token_to_type_text() {
            let w = setup();
            let cancel = CancellationToken::new();
            w.insert("hello", InsertOptions { cancel: Some(cancel.clone()), ..type_() });
            let seen = w.plat().type_cancel.clone().expect("type_text got a token");
            assert!(!seen.is_cancelled());
            cancel.cancel();
            assert!(seen.is_cancelled());
        }
    }
}
