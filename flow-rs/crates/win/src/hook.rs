//! The global input hook: `WH_KEYBOARD_LL` and `WH_MOUSE_LL` events, mapped to
//! key ids and fed to the hotkey matcher. Events are compared against the
//! bindings and discarded; nothing is stored or logged. Mirrors
//! `src/main/input/hook.ts`.
//!
//! Two threads. The hook thread installs the hooks and runs their message loop;
//! its hook procedures only map the event, push it on a channel and pass it on
//! (`CallNextHookEx`), so no key is ever swallowed or delayed. The processing
//! thread owns everything else: the matcher, the "verify held keys" timer
//! (which runs only while keys are held), binding capture for the settings
//! recorder, suspension and the Start-menu mask.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use flow_core::config::{HotkeyAction, HotkeysConfig};
use flow_core::hotkeys::{KeyId, binding_from_keys, format_binding, is_modifier_token, parse_binding};
use flow_core::matcher::{HotkeyEvent, HotkeyMatcher, KeyProbe, MatcherOptions};
use parking_lot::{Mutex, ReentrantMutex};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT,
};
use windows::core::PCWSTR;

pub use crate::keymap::InputEvent;
use crate::keymap::{map_keyboard_event, map_mouse_event};

/// While keys are held, how often they are checked against the OS for missed releases.
pub const VERIFY_INTERVAL: Duration = Duration::from_millis(250);

/// Where a hook backend delivers mapped events. Must not block.
pub type InputSink = Arc<dyn Fn(InputEvent) + Send + Sync>;

/// The source of raw input: the real low-level hooks, or a fake in tests.
pub trait HookBackend: Send + Sync {
    /// Installs the hooks; mapped events go to `sink` until [`HookBackend::stop`].
    /// `Err` carries the reason, for the "degraded" list.
    fn start(&self, sink: InputSink) -> Result<(), String>;
    fn stop(&self);
}

#[derive(Clone, Default)]
pub struct InputHookDeps {
    /// Called when a binding containing Win or Alt is engaged, while the modifier is still down.
    pub mask_start_menu: Option<Arc<dyn Fn() + Send + Sync>>,
    /// `Platform::is_key_down`: lets the hook notice key releases it was never sent.
    pub is_key_down: Option<KeyProbe>,
}

fn action_of(event: HotkeyEvent) -> Option<HotkeyAction> {
    match event {
        HotkeyEvent::HoldStart | HotkeyEvent::Lock => Some(HotkeyAction::HoldToTalk),
        HotkeyEvent::HoldEnd { .. } => None,
        HotkeyEvent::Toggle => Some(HotkeyAction::Toggle),
        HotkeyEvent::Cancel => Some(HotkeyAction::Cancel),
        HotkeyEvent::PasteLast => Some(HotkeyAction::PasteLast),
    }
}

/// Releasing Win alone opens the Start menu, and Alt alone focuses the menu bar,
/// unless another key was pressed meanwhile.
fn needs_mask(binding: &str) -> bool {
    parse_binding(binding)
        .is_some_and(|b| b.tokens.iter().any(|t| is_modifier_token(t) && (t.ends_with("Win") || t.ends_with("Alt"))))
}

/// A lone modifier is recorded sided (`RCtrl`): a generic lone `Ctrl` would fire on every Ctrl+C.
fn chord_text(keys: &[KeyId]) -> String {
    format_binding(&binding_from_keys(keys, keys.len() > 1))
}

type Progress = Arc<dyn Fn(String) + Send + Sync>;

struct Capture {
    held: Vec<KeyId>,
    best: Vec<KeyId>,
    on_progress: Progress,
    reply: Sender<Option<String>>,
}

/// What a call into the core asks to happen once its lock is released.
#[derive(Default)]
struct Effects {
    /// Each event, and whether the Start menu must be masked first.
    events: Vec<(HotkeyEvent, bool)>,
    progress: Option<(Progress, String)>,
}

/// The processing side's state. Every method runs under the hook's lock.
struct Core {
    matcher: HotkeyMatcher,
    bindings: HotkeysConfig,
    started: bool,
    suspended: bool,
    capture: Option<Capture>,
    /// The verify timer is running.
    verifying: bool,
    can_verify: bool,
}

impl Core {
    fn events(&self, events: Vec<HotkeyEvent>) -> Effects {
        let events = events
            .into_iter()
            .map(|event| (event, action_of(event).is_some_and(|action| needs_mask(self.bindings.binding(action)))))
            .collect();
        Effects { events, progress: None }
    }

    fn handle(&mut self, input: InputEvent) -> Effects {
        if self.capture.is_some() {
            return self.capture_key(input);
        }
        if self.suspended {
            return Effects::default();
        }
        if input.down {
            let events = self.matcher.key_down(input.key);
            // Runs only while keys are held, so an idle keyboard costs nothing.
            if self.can_verify {
                self.verifying = true;
            }
            self.events(events)
        } else {
            let events = self.matcher.key_up(input.key);
            self.events(events)
        }
    }

    /// Forgets all held keys; a hold in progress ends as aborted.
    fn reset(&mut self) -> Effects {
        let events = self.matcher.reset();
        self.events(events)
    }

    fn verify_tick(&mut self) -> Effects {
        let events = self.matcher.verify_held();
        if self.matcher.held_count() == 0 {
            self.verifying = false;
        }
        self.events(events)
    }

    fn capture_key(&mut self, input: InputEvent) -> Effects {
        let Some(capture) = self.capture.as_mut() else { return Effects::default() };
        let key = input.key.to_string();
        if input.down {
            if capture.held.contains(&key) {
                return Effects::default();
            }
            capture.held.push(key);
            if capture.held.len() >= capture.best.len() {
                capture.best = capture.held.clone();
            }
            let progress = chord_text(&capture.held);
            return Effects { events: Vec::new(), progress: Some((capture.on_progress.clone(), progress)) };
        }
        // Ignores the release of a key pressed before recording began (the Enter that clicked "Record").
        let Some(index) = capture.held.iter().position(|k| *k == key) else { return Effects::default() };
        capture.held.remove(index);
        if capture.held.is_empty() {
            let best = chord_text(&capture.best);
            self.finish_capture(Some(best));
        }
        Effects::default()
    }

    fn finish_capture(&mut self, result: Option<String>) {
        if let Some(capture) = self.capture.take() {
            let _ = capture.reply.send(result);
        }
    }
}

enum Message {
    Input(InputEvent),
    /// Answers once every message before it has been handled.
    #[cfg_attr(not(test), allow(dead_code))]
    Flush(Sender<()>),
    Stop,
}

struct Running {
    tx: Sender<Message>,
    processor: Option<JoinHandle<()>>,
    processor_id: ThreadId,
}

struct Inner {
    core: Mutex<Core>,
    /// Keeps events in order when several threads call in; re-entrant so a
    /// callback may call back into the hook.
    order: ReentrantMutex<()>,
    deps: InputHookDeps,
    on_hotkey: Arc<dyn Fn(HotkeyEvent) + Send + Sync>,
    verify_interval: Duration,
}

impl Inner {
    /// Runs `f` on the core, then its effects with the core unlocked.
    fn with_core<R>(&self, f: impl FnOnce(&mut Core) -> (R, Effects)) -> R {
        let _order = self.order.lock();
        let (result, effects) = f(&mut self.core.lock());
        self.apply(effects);
        result
    }

    fn apply(&self, effects: Effects) {
        if let Some((on_progress, partial)) = effects.progress {
            on_progress(partial);
        }
        for (event, mask) in effects.events {
            if mask && let Some(mask_start_menu) = &self.deps.mask_start_menu {
                // Masking is cosmetic; the hotkey still works if it fails.
                let _ = catch_unwind(AssertUnwindSafe(|| mask_start_menu()));
            }
            (self.on_hotkey)(event);
        }
    }

    fn process(self: &Arc<Self>, rx: Receiver<Message>) {
        let mut next_tick: Option<Instant> = None;
        loop {
            if self.core.lock().verifying {
                next_tick.get_or_insert_with(|| Instant::now() + self.verify_interval);
            } else {
                next_tick = None;
            }
            let message = match next_tick {
                Some(deadline) => match rx.recv_deadline(deadline) {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout) => {
                        self.with_core(|core| ((), core.verify_tick()));
                        next_tick = Some((deadline + self.verify_interval).max(Instant::now()));
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(message) => message,
                    Err(_) => return,
                },
            };
            match message {
                Message::Input(input) => self.with_core(|core| ((), core.handle(input))),
                Message::Flush(done) => {
                    let _ = done.send(());
                }
                Message::Stop => return,
            }
        }
    }
}

/// The global hotkey hook. Emits [`HotkeyEvent`]s through `on_hotkey`.
pub struct InputHook {
    inner: Arc<Inner>,
    backend: Arc<dyn HookBackend>,
    running: Mutex<Option<Running>>,
    load_error: Mutex<Option<String>>,
}

impl InputHook {
    /// A hook over the real `WH_KEYBOARD_LL`/`WH_MOUSE_LL` hooks. Inert until [`InputHook::start`].
    pub fn new(deps: InputHookDeps, on_hotkey: Arc<dyn Fn(HotkeyEvent) + Send + Sync>) -> Arc<Self> {
        Self::with_backend(deps, on_hotkey, Arc::new(Win32HookBackend::default()), VERIFY_INTERVAL)
    }

    /// Test seam: supplies the input source and the verify interval.
    pub fn with_backend(
        deps: InputHookDeps,
        on_hotkey: Arc<dyn Fn(HotkeyEvent) + Send + Sync>,
        backend: Arc<dyn HookBackend>,
        verify_interval: Duration,
    ) -> Arc<Self> {
        let bindings = HotkeysConfig::default();
        let matcher = HotkeyMatcher::new(MatcherOptions {
            bindings: bindings.clone(),
            is_key_down: deps.is_key_down.clone(),
            ..MatcherOptions::default()
        });
        let core = Core {
            matcher,
            bindings,
            started: false,
            suspended: false,
            capture: None,
            verifying: false,
            can_verify: deps.is_key_down.is_some(),
        };
        Arc::new(Self {
            inner: Arc::new(Inner {
                core: Mutex::new(core),
                order: ReentrantMutex::new(()),
                deps,
                on_hotkey,
                verify_interval,
            }),
            backend,
            running: Mutex::new(None),
            load_error: Mutex::new(None),
        })
    }

    /// Starts the global hook. Returns false (and stays inert) if the hook cannot be installed.
    pub fn start(&self) -> bool {
        let mut running = self.running.lock();
        if running.is_some() {
            return true;
        }
        let (tx, rx) = crossbeam_channel::unbounded::<Message>();
        let sink_tx = tx.clone();
        let sink: InputSink = Arc::new(move |input| {
            let _ = sink_tx.send(Message::Input(input));
        });
        if let Err(error) = self.backend.start(sink) {
            *self.load_error.lock() = Some(error);
            return false;
        }
        let inner = self.inner.clone();
        let spawned = thread::Builder::new().name("flow-hotkeys".into()).spawn(move || inner.process(rx));
        let processor = match spawned {
            Ok(handle) => handle,
            Err(error) => {
                self.backend.stop();
                *self.load_error.lock() = Some(error.to_string());
                return false;
            }
        };
        let processor_id = processor.thread().id();
        *running = Some(Running { tx, processor: Some(processor), processor_id });
        self.inner.core.lock().started = true;
        true
    }

    pub fn stop(&self) {
        self.cancel_capture();
        let running = self.running.lock().take();
        self.inner.with_core(|core| {
            core.started = false;
            core.verifying = false;
            ((), Effects::default())
        });
        if let Some(mut running) = running {
            self.backend.stop();
            let _ = running.tx.send(Message::Stop);
            // Called from a hotkey callback, the processing thread is this one or
            // may be waiting for this one: it ends on its own once it gets `Stop`.
            let in_callback = self.inner.order.is_owned_by_current_thread();
            if !in_callback
                && thread::current().id() != running.processor_id
                && let Some(processor) = running.processor.take()
            {
                let _ = processor.join();
            }
        }
        self.inner.with_core(|core| ((), core.reset()));
    }

    /// Why `start()` returned false, for the "degraded" list.
    pub fn load_error(&self) -> Option<String> {
        self.load_error.lock().clone()
    }

    pub fn set_bindings(&self, bindings: &HotkeysConfig) {
        self.inner.with_core(|core| {
            core.bindings = bindings.clone();
            let events = core.matcher.set_bindings(bindings);
            ((), core.events(events))
        });
    }

    pub fn set_cancel_armed(&self, armed: bool) {
        self.inner.core.lock().matcher.set_cancel_armed(armed);
    }

    /// Paused: no events are emitted (tray "Pause dictation", and while recording
    /// a binding). A hold in progress is ended first, as aborted.
    pub fn set_suspended(&self, suspended: bool) {
        self.inner.with_core(|core| {
            let events = if suspended && !core.suspended { core.matcher.reset() } else { Vec::new() };
            core.suspended = suspended;
            ((), core.events(events))
        });
    }

    /// Forget all held keys (after sleep/resume). A hold in progress ends as aborted.
    pub fn reset(&self) {
        self.inner.with_core(|core| ((), core.reset()));
    }

    /// True while any binding's keys are partially or fully held.
    pub fn any_held(&self) -> bool {
        self.inner.core.lock().matcher.any_held()
    }

    /// Records the next chord for the settings hotkey recorder: tracks keys as
    /// they go down, reports the partial chord through `on_progress`, and answers
    /// with the largest chord held once every key is released. Dictation events
    /// are suspended meanwhile. Answers `None` if [`InputHook::cancel_capture`] is
    /// called or the hook is not running.
    pub fn capture_binding(&self, on_progress: Arc<dyn Fn(String) + Send + Sync>) -> Receiver<Option<String>> {
        self.cancel_capture();
        let (reply, result) = crossbeam_channel::bounded(1);
        self.inner.with_core(|core| {
            if !core.started {
                let _ = reply.send(None);
                return ((), Effects::default());
            }
            let events = core.matcher.reset();
            core.capture = Some(Capture { held: Vec::new(), best: Vec::new(), on_progress, reply });
            ((), core.events(events))
        });
        result
    }

    pub fn cancel_capture(&self) {
        self.inner.with_core(|core| {
            core.finish_capture(None);
            ((), Effects::default())
        });
    }

    /// Waits until every event delivered so far has been handled.
    #[cfg(test)]
    fn flush(&self) {
        let tx = self.running.lock().as_ref().map(|r| r.tx.clone());
        if let Some(tx) = tx {
            let (done, wait) = crossbeam_channel::bounded(1);
            if tx.send(Message::Flush(done)).is_ok() {
                let _ = wait.recv_timeout(Duration::from_secs(5));
            }
        }
    }

    /// The verify timer is running.
    #[cfg(test)]
    fn verifying(&self) -> bool {
        self.inner.core.lock().verifying
    }
}

impl Drop for InputHook {
    fn drop(&mut self) {
        if self.running.get_mut().is_some() {
            self.stop();
        }
    }
}

// ── The real hooks ───────────────────────────────────────────────────────────

thread_local! {
    /// The hook thread's sink. Hook procedures run on the thread that installed them.
    static SINK: RefCell<Option<InputSink>> = const { RefCell::new(None) };
}

fn deliver(input: InputEvent) {
    SINK.with(|sink| {
        if let Ok(sink) = sink.try_borrow()
            && let Some(sink) = sink.as_ref()
        {
            sink(input);
        }
    });
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && lparam.0 != 0 {
        // SAFETY: for HC_ACTION, lParam points to a KBDLLHOOKSTRUCT owned by the system for this call.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if let Some(input) =
            map_keyboard_event(wparam.0 as u32, info.vkCode, info.scanCode, info.flags.0, info.dwExtraInfo)
        {
            deliver(input);
        }
    }
    // Never swallow input: every event goes on to the next hook and the app.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && lparam.0 != 0 {
        // SAFETY: for HC_ACTION, lParam points to an MSLLHOOKSTRUCT owned by the system for this call.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if let Some(input) = map_mouse_event(wparam.0 as u32, info.mouseData, info.dwExtraInfo) {
            deliver(input);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

struct HookThread {
    thread_id: u32,
    handle: JoinHandle<()>,
}

/// `WH_KEYBOARD_LL` and `WH_MOUSE_LL` on a dedicated thread with its own message loop.
#[derive(Default)]
pub struct Win32HookBackend {
    thread: Mutex<Option<HookThread>>,
}

fn hook_thread(sink: InputSink, ready: Sender<Result<u32, String>>) {
    SINK.with(|slot| *slot.borrow_mut() = Some(sink));
    unsafe {
        // Make sure this thread has a message queue before anyone posts WM_QUIT to it.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        let module = GetModuleHandleW(PCWSTR::null()).ok().map(|m| HINSTANCE(m.0));
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        let (keyboard, mouse): (HHOOK, HHOOK) = match (keyboard, mouse) {
            (Ok(keyboard), Ok(mouse)) => (keyboard, mouse),
            (keyboard, mouse) => {
                let message = [&keyboard, &mouse]
                    .iter()
                    .find_map(|r| r.as_ref().err().map(|e| e.message()))
                    .unwrap_or_default();
                for hook in [keyboard, mouse].into_iter().flatten() {
                    let _ = UnhookWindowsHookEx(hook);
                }
                let _ = ready.send(Err(format!("SetWindowsHookEx failed: {message}")));
                return;
            }
        };
        let _ = ready.send(Ok(GetCurrentThreadId()));
        // Low-level hooks are called through this thread's message loop.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(keyboard);
        let _ = UnhookWindowsHookEx(mouse);
    }
    SINK.with(|slot| *slot.borrow_mut() = None);
}

impl HookBackend for Win32HookBackend {
    fn start(&self, sink: InputSink) -> Result<(), String> {
        let mut slot = self.thread.lock();
        if slot.is_some() {
            return Ok(());
        }
        let (ready, started) = crossbeam_channel::bounded(1);
        let handle = thread::Builder::new()
            .name("flow-input-hook".into())
            .spawn(move || hook_thread(sink, ready))
            .map_err(|e| e.to_string())?;
        match started.recv() {
            Ok(Ok(thread_id)) => {
                *slot = Some(HookThread { thread_id, handle });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                let _ = handle.join();
                Err("the hook thread exited".into())
            }
        }
    }

    fn stop(&self) {
        let Some(hook) = self.thread.lock().take() else { return };
        // The queue exists (see hook_thread), so the post only fails if the thread is gone.
        if unsafe { PostThreadMessageW(hook.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_ok() {
            let _ = hook.handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows::Win32::UI::WindowsAndMessaging::{
        LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_XBUTTONDOWN,
        WM_XBUTTONUP,
    };

    use super::*;
    use crate::injected::INJECT_TAG;
    use crate::vk::{is_extended_key, vk_from_key_id};

    /// Stands in for the low-level hooks: tests push Win32-shaped events through
    /// the same mapping the hook procedures use.
    #[derive(Default)]
    struct FakeBackend {
        sink: Mutex<Option<InputSink>>,
        running: Mutex<bool>,
        fail: Mutex<Option<String>>,
    }

    impl HookBackend for FakeBackend {
        fn start(&self, sink: InputSink) -> Result<(), String> {
            if let Some(error) = self.fail.lock().clone() {
                return Err(error);
            }
            *self.sink.lock() = Some(sink);
            *self.running.lock() = true;
            Ok(())
        }
        fn stop(&self) {
            *self.sink.lock() = None;
            *self.running.lock() = false;
        }
    }

    impl FakeBackend {
        fn running(&self) -> bool {
            *self.running.lock()
        }
        fn attached(&self) -> bool {
            self.sink.lock().is_some()
        }
        fn emit(&self, input: Option<InputEvent>) {
            let sink = self.sink.lock().clone();
            if let (Some(sink), Some(input)) = (sink, input) {
                sink(input);
            }
        }
        /// A physical key event, tagged with `extra_info`.
        fn key(&self, key: &str, down: bool, extra_info: usize) {
            let vk = vk_from_key_id(key).unwrap_or_else(|| panic!("{key} has no virtual key"));
            let mut flags = if is_extended_key(key) { LLKHF_EXTENDED.0 } else { 0 };
            if !down {
                flags |= LLKHF_UP.0;
            }
            if extra_info != 0 {
                flags |= LLKHF_INJECTED.0;
            }
            let message = if down { WM_KEYDOWN } else { WM_KEYUP };
            self.emit(map_keyboard_event(message, u32::from(vk), 0, flags, extra_info));
        }
        fn mouse(&self, message: u32, mouse_data: u32) {
            self.emit(map_mouse_event(message, mouse_data, 0));
        }
    }

    struct T {
        backend: Arc<FakeBackend>,
        hook: Arc<InputHook>,
        events: Arc<Mutex<Vec<HotkeyEvent>>>,
    }

    impl T {
        fn take(&self) -> Vec<HotkeyEvent> {
            std::mem::take(&mut *self.events.lock())
        }
        fn press(&self, key: &str) {
            self.backend.key(key, true, 0);
            self.hook.flush();
        }
        fn release(&self, key: &str) {
            self.backend.key(key, false, 0);
            self.hook.flush();
        }
        /// The echo of a key this process injected.
        fn injected(&self, key: &str, down: bool) {
            self.backend.key(key, down, INJECT_TAG);
            self.hook.flush();
        }
        fn mouse(&self, message: u32, button: u32) {
            self.backend.mouse(message, button << 16);
            self.hook.flush();
        }
        fn raw_key(&self, vk: u32, down: bool) {
            let message = if down { WM_KEYDOWN } else { WM_KEYUP };
            self.backend.emit(map_keyboard_event(message, vk, 0, 0, 0));
            self.hook.flush();
        }
    }

    fn setup_with(deps: InputHookDeps, verify_interval: Duration) -> T {
        let backend = Arc::new(FakeBackend::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let hook = InputHook::with_backend(
            deps,
            Arc::new(move |event| sink.lock().push(event)),
            backend.clone(),
            verify_interval,
        );
        T { backend, hook, events }
    }

    fn setup(deps: InputHookDeps) -> T {
        setup_with(deps, VERIFY_INTERVAL)
    }

    fn bindings(hold_to_talk: &str) -> HotkeysConfig {
        HotkeysConfig { hold_to_talk: hold_to_talk.into(), ..HotkeysConfig::default() }
    }

    fn mask_counter() -> (Arc<AtomicUsize>, InputHookDeps) {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        let deps = InputHookDeps {
            mask_start_menu: Some(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
            ..InputHookDeps::default()
        };
        (count, deps)
    }

    /// Polls until `done` holds, for the tests that run on real time.
    fn wait_until(done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
        done()
    }

    const START: HotkeyEvent = HotkeyEvent::HoldStart;
    const END: HotkeyEvent = HotkeyEvent::HoldEnd { aborted: false };
    const ABORT: HotkeyEvent = HotkeyEvent::HoldEnd { aborted: true };

    mod input_hook {
        use super::*;

        #[test]
        fn starts_the_hook_and_emits_hotkey_events_with_the_default_bindings() {
            let t = setup(InputHookDeps::default());
            assert!(t.hook.start());
            assert!(t.backend.running());
            t.press("LCtrl");
            t.press("LWin");
            assert!(t.hook.any_held());
            t.release("LWin");
            t.release("LCtrl");
            assert_eq!(t.take(), vec![START, END]);
            assert!(!t.hook.any_held());
        }

        #[test]
        fn start_is_idempotent_and_stop_detaches() {
            let t = setup(InputHookDeps::default());
            assert!(t.hook.start());
            assert!(t.hook.start());
            assert!(t.backend.attached());
            t.hook.stop();
            assert!(!t.backend.running());
            assert!(!t.backend.attached());
            t.hook.stop();
            // Restarting works.
            assert!(t.hook.start());
            t.press("LCtrl");
            t.press("LWin");
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn returns_false_and_stays_inert_when_the_hook_cannot_be_installed() {
            let t = setup(InputHookDeps::default());
            *t.backend.fail.lock() = Some("SetWindowsHookEx failed: Access is denied.".into());
            assert!(!t.hook.start());
            assert_eq!(t.hook.load_error().as_deref(), Some("SetWindowsHookEx failed: Access is denied."));
            assert!(!t.hook.any_held());
            assert_eq!(t.hook.capture_binding(Arc::new(|_| {})).recv().ok(), Some(None));
            t.hook.set_bindings(&HotkeysConfig::default());
            t.hook.set_suspended(true);
            t.hook.reset();
            t.hook.stop();
            assert!(!t.backend.attached());
            assert_eq!(t.take(), vec![]);
        }

        #[test]
        fn maps_mouse_buttons_and_ignores_left_right_click_and_unknown_keys() {
            let t = setup(InputHookDeps::default());
            t.hook.set_bindings(&bindings("Mouse4"));
            t.hook.start();
            t.mouse(WM_XBUTTONDOWN, 1);
            // Clicking, a media key and an id-less key during the hold do not abort it.
            t.mouse(WM_LBUTTONDOWN, 0);
            t.mouse(WM_LBUTTONUP, 0);
            t.raw_key(0xad, true); // volume mute
            t.raw_key(0xe7, true); // VK_PACKET: an injected Unicode character
            t.raw_key(0xe7, false);
            t.mouse(WM_XBUTTONUP, 1);
            assert_eq!(t.take(), vec![START, END]);
        }

        #[test]
        fn set_bindings_and_set_cancel_armed_reach_the_matcher() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.hook.set_bindings(&bindings("F9"));
            t.press("Esc");
            t.release("Esc");
            t.hook.set_cancel_armed(true);
            t.press("Esc");
            t.release("Esc");
            t.press("F9");
            assert_eq!(t.take(), vec![HotkeyEvent::Cancel, START]);
        }
    }

    mod suspension {
        use super::*;

        #[test]
        fn emits_nothing_while_suspended_and_resumes_cleanly() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.hook.set_suspended(true);
            t.press("LCtrl");
            t.press("LWin");
            t.release("LWin");
            t.release("LCtrl");
            assert_eq!(t.take(), vec![]);
            assert!(!t.hook.any_held());
            t.hook.set_suspended(false);
            t.press("LCtrl");
            t.press("LWin");
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn ends_a_hold_in_progress_as_aborted_when_suspended() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.press("LCtrl");
            t.press("LWin");
            t.hook.set_suspended(true);
            t.release("LWin");
            assert_eq!(t.take(), vec![START, ABORT]);
        }

        #[test]
        fn reset_ends_a_hold_in_progress_as_aborted() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.press("LCtrl");
            t.press("LWin");
            t.hook.reset();
            assert_eq!(t.take(), vec![START, ABORT]);
            assert!(!t.hook.any_held());
        }
    }

    mod start_menu_masking {
        use super::*;

        #[test]
        fn masks_when_a_binding_containing_win_is_engaged() {
            let (masks, deps) = mask_counter();
            let t = setup(deps);
            t.hook.start();
            t.press("LCtrl");
            assert_eq!(masks.load(Ordering::SeqCst), 0);
            t.press("LWin");
            assert_eq!(masks.load(Ordering::SeqCst), 1);
            t.press("Space"); // toggle: Ctrl+Win+Space
            assert_eq!(masks.load(Ordering::SeqCst), 2);
            t.release("Space");
            t.release("LWin");
            assert_eq!(masks.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn masks_lone_alt_bindings_and_sided_win_but_not_bindings_without_them() {
            let (masks, deps) = mask_counter();
            let t = setup(deps);
            t.hook.set_bindings(&HotkeysConfig {
                hold_to_talk: "RAlt".into(),
                toggle: "RWin".into(),
                cancel: "Esc".into(),
                paste_last: "Ctrl+Shift+V".into(),
                double_tap_lock: false,
            });
            t.hook.set_cancel_armed(true);
            t.hook.start();
            for key in ["Esc", "LCtrl", "LShift", "V"] {
                t.press(key);
            }
            for key in ["V", "LShift", "LCtrl", "Esc"] {
                t.release(key);
            }
            assert_eq!(t.take(), vec![HotkeyEvent::Cancel, HotkeyEvent::PasteLast]);
            assert_eq!(masks.load(Ordering::SeqCst), 0);
            t.press("RAlt");
            t.release("RAlt");
            t.press("RWin");
            assert_eq!(masks.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn a_failing_mask_does_not_stop_the_event() {
            let deps = InputHookDeps {
                mask_start_menu: Some(Arc::new(|| panic!("SendInput failed"))),
                ..InputHookDeps::default()
            };
            let t = setup(deps);
            t.hook.start();
            t.press("LCtrl");
            t.press("LWin");
            assert_eq!(t.take(), vec![START]);
        }
    }

    mod injected_keys {
        use super::*;

        #[test]
        fn ignores_the_echo_of_keys_this_process_injected() {
            let t = setup(InputHookDeps::default());
            t.hook.set_bindings(&HotkeysConfig { toggle: "Ctrl+V".into(), ..HotkeysConfig::default() });
            t.hook.start();
            t.injected("LCtrl", true);
            t.injected("V", true);
            t.injected("V", false);
            t.injected("LCtrl", false);
            assert_eq!(t.take(), vec![]);
            assert!(!t.hook.any_held());
            // The same keys pressed for real do fire.
            t.press("LCtrl");
            t.press("V");
            assert_eq!(t.take(), vec![HotkeyEvent::Toggle]);
        }

        #[test]
        fn an_injected_enter_while_typing_does_not_abort_a_hold() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.press("LCtrl");
            t.press("LWin");
            t.injected("Enter", true);
            t.injected("Enter", false);
            t.release("LWin");
            assert_eq!(t.take(), vec![START, END]);
        }

        #[test]
        fn an_injected_release_does_not_end_a_hold_the_user_is_physically_keeping() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.press("LCtrl");
            t.press("LWin");
            t.injected("LCtrl", true);
            t.injected("LCtrl", false);
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn input_injected_by_other_software_is_still_seen() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            // AutoHotkey, remote desktop, on-screen keyboards: injected, but not by us.
            t.backend.key("LCtrl", true, 0xFFC3_C3C3);
            t.backend.key("LWin", true, 0xFFC3_C3C3);
            t.hook.flush();
            assert_eq!(t.take(), vec![START]);
        }
    }

    mod capture_binding {
        use super::*;

        fn capture(t: &T) -> (Receiver<Option<String>>, Arc<Mutex<Vec<String>>>) {
            let progress = Arc::new(Mutex::new(Vec::new()));
            let sink = progress.clone();
            (t.hook.capture_binding(Arc::new(move |p| sink.lock().push(p))), progress)
        }

        fn result(rx: &Receiver<Option<String>>) -> Option<Option<String>> {
            rx.recv_timeout(Duration::from_secs(5)).ok()
        }

        #[test]
        fn reports_progress_and_resolves_with_the_largest_chord_once_all_keys_are_up() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            let (rx, progress) = capture(&t);
            t.press("LCtrl");
            t.press("LCtrl"); // auto-repeat
            t.press("LShift");
            t.press("D");
            t.release("D");
            t.release("LShift");
            t.release("LCtrl");
            assert_eq!(result(&rx), Some(Some("Ctrl+Shift+D".into())));
            assert_eq!(*progress.lock(), vec!["LCtrl", "Ctrl+Shift", "Ctrl+Shift+D"]);
        }

        #[test]
        fn suspends_dictation_events_while_recording_and_resumes_after() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            let (rx, _) = capture(&t);
            t.press("LCtrl");
            t.press("LWin");
            t.release("LWin");
            t.release("LCtrl");
            assert_eq!(result(&rx), Some(Some("Ctrl+Win".into())));
            assert_eq!(t.take(), vec![]);
            t.press("LCtrl");
            t.press("LWin");
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn records_a_lone_modifier_sided_and_a_mouse_button_by_name() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            let (rx, _) = capture(&t);
            t.press("RCtrl");
            t.release("RCtrl");
            assert_eq!(result(&rx), Some(Some("RCtrl".into())));

            let (rx, _) = capture(&t);
            t.mouse(WM_XBUTTONDOWN, 1);
            t.mouse(WM_XBUTTONUP, 1);
            assert_eq!(result(&rx), Some(Some("Mouse4".into())));

            let (rx, _) = capture(&t);
            t.press("Esc");
            t.release("Esc");
            assert_eq!(result(&rx), Some(Some("Esc".into())));
        }

        #[test]
        fn keeps_the_largest_chord_when_keys_are_released_one_by_one_or_re_pressed() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            let (rx, _) = capture(&t);
            t.press("LAlt");
            t.press("LShift");
            t.press("V");
            t.release("V");
            t.press("B"); // same size: the latest chord wins
            t.release("LAlt");
            t.release("B");
            t.release("LShift");
            assert_eq!(result(&rx), Some(Some("Alt+Shift+B".into())));
        }

        #[test]
        fn ignores_the_release_of_a_key_that_was_down_before_recording_began_and_clicks() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.press("Enter");
            let (rx, _) = capture(&t);
            t.release("Enter");
            t.mouse(WM_LBUTTONDOWN, 0);
            t.mouse(WM_LBUTTONUP, 0);
            t.press("F9");
            t.release("F9");
            assert_eq!(result(&rx), Some(Some("F9".into())));
        }

        #[test]
        fn resolves_none_on_cancel_capture_and_a_new_capture_cancels_the_previous_one() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            let (first, _) = capture(&t);
            let (second, _) = capture(&t);
            assert_eq!(result(&first), Some(None));
            t.press("F9");
            t.hook.cancel_capture();
            assert_eq!(result(&second), Some(None));
            t.hook.cancel_capture();
            // Dictation is live again, with no keys remembered from the capture.
            t.release("F9");
            t.press("LCtrl");
            t.press("LWin");
            assert_eq!(t.take(), vec![START]);
        }

        #[test]
        fn resolves_none_when_the_hook_stops_mid_capture() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            let (rx, _) = capture(&t);
            t.hook.stop();
            assert_eq!(result(&rx), Some(None));
        }

        #[test]
        fn ends_a_hold_in_progress_when_recording_begins() {
            let t = setup(InputHookDeps::default());
            t.hook.start();
            t.press("LCtrl");
            t.press("LWin");
            let _rx = capture(&t);
            assert_eq!(t.take(), vec![START, ABORT]);
            t.hook.cancel_capture();
        }
    }

    mod missed_key_releases {
        use super::*;

        const INTERVAL: Duration = Duration::from_millis(10);

        fn probe(os: Arc<Mutex<HashMap<String, bool>>>, calls: Arc<AtomicUsize>) -> InputHookDeps {
            InputHookDeps {
                is_key_down: Some(Arc::new(move |key: &str| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Some(os.lock().get(key).copied().unwrap_or(false))
                })),
                ..InputHookDeps::default()
            }
        }

        #[test]
        fn ends_a_hold_whose_key_up_never_arrived_then_stops_checking() {
            let os = Arc::new(Mutex::new(HashMap::new()));
            let calls = Arc::new(AtomicUsize::new(0));
            let t = setup_with(probe(os.clone(), calls.clone()), INTERVAL);
            t.hook.start();
            os.lock().extend([("LCtrl".to_string(), true), ("LWin".to_string(), true)]);
            t.press("LCtrl");
            t.press("LWin");
            let polled = calls.load(Ordering::SeqCst);
            assert!(wait_until(|| calls.load(Ordering::SeqCst) >= polled + 6));
            assert_eq!(t.take(), vec![START]);
            // Released while an elevated window had focus: the hook sees nothing.
            os.lock().clear();
            assert!(wait_until(|| !t.events.lock().is_empty()));
            assert_eq!(t.take(), vec![END]);
            assert!(!t.hook.any_held());
            assert!(!t.hook.verifying());
            let calls_after = calls.load(Ordering::SeqCst);
            thread::sleep(INTERVAL * 10);
            assert_eq!(calls.load(Ordering::SeqCst), calls_after);
        }

        #[test]
        fn does_not_poll_while_no_key_is_held() {
            let os = Arc::new(Mutex::new(HashMap::new()));
            let calls = Arc::new(AtomicUsize::new(0));
            let t = setup_with(probe(os, calls.clone()), INTERVAL);
            t.hook.start();
            thread::sleep(INTERVAL * 10);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(!t.hook.verifying());
            t.press("A");
            t.release("A");
            assert!(wait_until(|| !t.hook.verifying()));
            let calls_after = calls.load(Ordering::SeqCst);
            thread::sleep(INTERVAL * 10);
            assert_eq!(calls.load(Ordering::SeqCst), calls_after);
        }

        #[test]
        fn stop_clears_the_timer() {
            let os = Arc::new(Mutex::new(HashMap::from([("A".to_string(), true)])));
            let calls = Arc::new(AtomicUsize::new(0));
            let t = setup_with(probe(os, calls), INTERVAL);
            t.hook.start();
            t.press("A");
            assert!(t.hook.verifying());
            t.hook.stop();
            assert!(!t.hook.verifying());
        }

        #[test]
        fn no_timer_without_an_is_key_down_probe() {
            let t = setup_with(InputHookDeps::default(), INTERVAL);
            t.hook.start();
            t.press("A");
            assert!(!t.hook.verifying());
        }
    }

    #[test]
    fn needs_mask_only_for_win_and_alt() {
        assert!(needs_mask("Ctrl+Win"));
        assert!(needs_mask("RAlt"));
        assert!(needs_mask("Alt+Shift+V"));
        assert!(!needs_mask("Ctrl+Shift+V"));
        assert!(!needs_mask("Esc"));
        assert!(!needs_mask("not a key"));
    }

    #[test]
    fn records_chords_like_the_settings_recorder_expects() {
        assert_eq!(chord_text(&["RCtrl".into()]), "RCtrl");
        assert_eq!(chord_text(&["LWin".into(), "LCtrl".into()]), "Ctrl+Win");
        assert_eq!(chord_text(&["Mouse5".into()]), "Mouse5");
    }

    /// The real hook backend's lifecycle, without installing anything: stopping
    /// a backend that never started is a no-op.
    #[test]
    fn win32_backend_stop_without_start_is_harmless() {
        let backend = Win32HookBackend::default();
        backend.stop();
        backend.stop();
    }
}
