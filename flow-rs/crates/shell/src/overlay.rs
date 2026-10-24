//! The overlay window: port of `src/main/windows/overlay.ts`, with the state
//! handling, waveform loop and sounds of `src/renderer/overlay/main.ts`.
//!
//! A layered, click-through, topmost tool window that never activates, on its
//! own thread with its own message loop. It is created at launch and kept
//! hidden so it appears instantly. While it is visible and something moves (the
//! waveform, the dots, the entrance or fade) it draws at the monitor's refresh
//! rate, paced by a high-resolution waitable timer; otherwise the thread sleeps
//! in the message loop with no timers at all.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded, unbounded};
use flow_audio::{Analyser, FREQUENCY_BIN_COUNT};
use flow_core::config::{OverlayPosition, OverlayStyle};
use flow_core::types::{OverlayAppearance, OverlayPhase, OverlayState};
use parking_lot::Mutex;
use tiny_skia::Pixmap;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer, TIMER_ALL_ACCESS,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    HTTRANSPARENT, HWND_TOPMOST, MA_NOACTIVATE, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, PostMessageW, QS_ALLINPUT, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowPos, ShowWindow, TranslateMessage, WM_APP, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_QUIT,
    WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::anim::{CubicBezier, Transition};
use crate::color::SystemColors;
use crate::pill::{self, BARS, Motion, PillLook, PillRenderer, PillStyle, window_size};
use crate::sounds;
use crate::win::{Surface, monitor_info, refresh_rate};

/// The window's margin from the work-area edge, in CSS pixels.
const MARGIN: f32 = 28.0;
/// Lets the fade-out finish before the window is hidden.
const HIDE_DELAY: Duration = Duration::from_millis(220);
/// Posted to wake the window thread after a command was queued.
const WM_WAKE: u32 = WM_APP + 1;

enum Command {
    Configure(OverlayAppearance, OverlayPosition),
    State(OverlayState),
    Analyser(Option<Arc<Analyser>>),
    Destroy,
}

/// The overlay pill. Every method only queues work for the window thread, so
/// all are cheap and safe to call from any thread.
pub struct Overlay {
    tx: Sender<Command>,
    /// The window handle as an integer (HWND is not Send); `None` if creation failed.
    hwnd: Option<isize>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Overlay {
    /// Creates the (hidden) window on its own thread.
    pub fn create() -> Self {
        let (tx, rx) = unbounded();
        let (ready_tx, ready_rx) = bounded(1);
        let thread = std::thread::Builder::new()
            .name("flow-overlay".into())
            .spawn(move || run(rx, ready_tx))
            .map_err(|err| log::error!(target: "overlay", "could not start the overlay thread: {err}"))
            .ok();
        let hwnd = if thread.is_some() { ready_rx.recv().ok().flatten() } else { None };
        Self { tx, hwnd, thread: Mutex::new(thread) }
    }

    fn send(&self, command: Command) {
        if self.tx.send(command).is_ok()
            && let Some(hwnd) = self.hwnd
        {
            // SAFETY: posting to a window owned by another thread is allowed; a
            // destroyed window just drops the message.
            let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)) };
        }
    }

    /// The pill's look and where it sits. `style = none` hides it for good.
    pub fn configure(&self, appearance: OverlayAppearance, position: OverlayPosition) {
        self.send(Command::Configure(appearance, position));
    }

    pub fn set_state(&self, state: OverlayState) {
        self.send(Command::State(state));
    }

    /// The waveform's source while listening (the capture's analyser).
    pub fn set_analyser(&self, analyser: Option<Arc<Analyser>>) {
        self.send(Command::Analyser(analyser));
    }

    /// Hides and destroys the window and ends its thread.
    pub fn destroy(&self) {
        self.send(Command::Destroy);
        if let Some(thread) = self.thread.lock().take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.destroy();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Click-through and never activated, so it cannot interrupt typing.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn create_window() -> windows::core::Result<HWND> {
    // SAFETY: standard window class registration and creation on this thread.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("FlowOverlay");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        // Fails harmlessly when a second overlay registers the same class.
        RegisterClassExW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class,
            w!("Flow overlay"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

fn run(rx: Receiver<Command>, ready: Sender<Option<isize>>) {
    // SAFETY: affects only this thread: coordinates are physical pixels on every monitor.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(err) => {
            log::error!(target: "overlay", "could not create the overlay window: {err}");
            let _ = ready.send(None);
            return;
        }
    };
    let _ = ready.send(Some(hwnd.0 as isize));
    // SAFETY: an unnamed timer for this thread; closed below.
    let timer = unsafe {
        CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0)
            .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))
            .ok()
    };
    let mut window = Window::new(hwnd);
    'outer: loop {
        loop {
            match rx.try_recv() {
                Ok(Command::Destroy) | Err(TryRecvError::Disconnected) => break 'outer,
                Ok(command) => window.handle(command),
                Err(TryRecvError::Empty) => break,
            }
        }
        let now = Instant::now();
        window.tick(now);
        wait(timer, window.next_wake(now), now);
        let mut msg = MSG::default();
        // SAFETY: the standard message pump for this thread's windows.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'outer;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    window.hide_now();
    // SAFETY: the window and timer belong to this thread.
    unsafe {
        let _ = DestroyWindow(hwnd);
        if let Some(timer) = timer {
            let _ = CloseHandle(timer);
        }
    }
}

/// Sleeps until a message arrives or until `wake`.
fn wait(timer: Option<HANDLE>, wake: Option<Instant>, now: Instant) {
    let handles: Vec<HANDLE>;
    let mut timeout = INFINITE;
    match (wake, timer) {
        (Some(at), Some(timer)) => {
            // Relative due time in 100 ns units (negative).
            let due = -((at.saturating_duration_since(now).as_nanos() / 100).max(1) as i64);
            // SAFETY: a valid timer handle and due time.
            if unsafe { SetWaitableTimer(timer, &due, 0, None, None, false) }.is_ok() {
                handles = vec![timer];
            } else {
                handles = Vec::new();
                timeout = at.saturating_duration_since(now).as_millis().max(1) as u32;
            }
        }
        (Some(at), None) => {
            handles = Vec::new();
            timeout = at.saturating_duration_since(now).as_millis().max(1) as u32;
        }
        (None, _) => handles = Vec::new(),
    }
    let handles = (!handles.is_empty()).then_some(&handles[..]);
    // SAFETY: waits on this thread's queue and the timer.
    unsafe { MsgWaitForMultipleObjectsEx(handles, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
}

/// The window thread's state: overlay.ts's fields and the page's.
struct Window {
    hwnd: HWND,
    renderer: PillRenderer,
    frame: Option<Pixmap>,
    surface: Option<Surface>,
    // overlay.ts
    configured: bool,
    sounds: bool,
    position: OverlayPosition,
    state: OverlayState,
    hide_at: Option<Instant>,
    visible: bool,
    origin: (i32, i32),
    dpr: f32,
    period: Duration,
    // main.ts and styles.css
    look: PillLook,
    style: PillStyle,
    levels: [f32; BARS],
    analyser: Option<Arc<Analyser>>,
    spectrum: [u8; FREQUENCY_BIN_COUNT],
    dots_since: Instant,
    opacity: Transition,
    enter: Transition,
    next_frame: Instant,
    was_animating: bool,
    dirty: bool,
}

impl Window {
    fn new(hwnd: HWND) -> Self {
        let now = Instant::now();
        Self {
            hwnd,
            renderer: PillRenderer::new(),
            frame: None,
            surface: None,
            configured: false,
            sounds: false,
            position: OverlayPosition::BottomCenter,
            state: OverlayState::hidden(),
            hide_at: None,
            visible: false,
            origin: (0, 0),
            dpr: 1.0,
            period: Duration::from_micros(16_667),
            look: PillLook::default(),
            style: PillStyle::default(),
            levels: [0.0; BARS],
            analyser: None,
            spectrum: [0; FREQUENCY_BIN_COUNT],
            dots_since: now,
            opacity: Transition::settled(0.0, CubicBezier::EASE, now),
            enter: Transition::settled(0.0, pill::MOVE_EASING, now),
            next_frame: now,
            was_animating: false,
            dirty: false,
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Configure(appearance, position) => self.configure(appearance, position),
            Command::State(state) => self.set_state(state),
            Command::Analyser(analyser) => self.analyser = analyser,
            Command::Destroy => {}
        }
    }

    /// overlay.ts `configure` + the page's `applyAppearance`.
    fn configure(&mut self, appearance: OverlayAppearance, position: OverlayPosition) {
        self.position = position;
        let system = if appearance.theme.high_contrast { SystemColors::current() } else { SystemColors::default() };
        self.style = PillStyle::from_appearance(&appearance, system);
        self.sounds = appearance.sounds;
        self.configured = true;
        if self.style.reduced_motion {
            let now = Instant::now();
            self.opacity.snap(self.opacity.target(), now);
            self.enter.snap(self.enter.target(), now);
        }
        self.dirty = true;
        if self.style.style == OverlayStyle::None {
            self.hide_now();
        }
    }

    /// overlay.ts `setState` + the page's `applyState`.
    fn set_state(&mut self, state: OverlayState) {
        let changed = state.phase != self.state.phase
            || state.cloud != self.state.cloud
            || state.locked != self.state.locked
            || state.message != self.state.message
            || state.queued != self.state.queued;
        if !changed {
            return;
        }
        let now = Instant::now();
        let previous = self.look.phase;
        self.look.apply(&state);
        let phase = self.look.phase;
        if previous != phase {
            if self.configured && self.sounds {
                sounds::play_transition(previous, phase);
            }
            if phase == OverlayPhase::Transcribing {
                // The dots' animation restarts whenever they reappear.
                self.dots_since = now;
            }
        }
        if phase != OverlayPhase::Listening {
            self.levels = [0.0; BARS];
        }
        let target = if phase == OverlayPhase::Hidden { 0.0 } else { 1.0 };
        if self.style.reduced_motion {
            self.opacity.snap(target, now);
            self.enter.snap(target, now);
        } else {
            self.opacity.retarget(target, pill::FADE.as_secs_f64(), now);
            self.enter.retarget(target, pill::MOVE.as_secs_f64(), now);
        }
        self.dirty = true;
        self.state = state;

        if phase == OverlayPhase::Hidden {
            if self.hide_at.is_none() {
                self.hide_at = Some(now + HIDE_DELAY);
            }
            return;
        }
        self.hide_at = None;
        if self.style.style == OverlayStyle::None {
            return;
        }
        if !self.visible {
            self.place();
            self.visible = true;
            self.next_frame = now;
            self.draw(now);
            // SAFETY: our own window; shown without activation.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                // Some full-screen windows push other topmost windows down; reassert.
                let _ =
                    SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn hide_now(&mut self) {
        self.hide_at = None;
        if self.visible {
            // SAFETY: our own window.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
        let now = Instant::now();
        self.opacity.snap(self.opacity.target(), now);
        self.enter.snap(self.enter.target(), now);
        self.was_animating = false;
        // Nothing to keep while hidden: buffers come back on the next show.
        self.renderer.release();
        self.surface = None;
        self.frame = None;
    }

    /// Positions the window on the display the user is working on.
    fn place(&mut self) {
        let mut cursor = POINT::default();
        // SAFETY: plain queries with valid out-pointers.
        unsafe {
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_err() {
                dpi_x = 96;
            }
            self.dpr = dpi_x.max(48) as f32 / 96.0;
            let Some(info) = monitor_info(monitor) else { return };
            let (w, h) = window_size(self.dpr);
            let (w, h) = (w as i32, h as i32);
            let margin = (MARGIN * self.dpr).round() as i32;
            let area = info.work;
            let x = match self.position {
                OverlayPosition::BottomLeft | OverlayPosition::TopLeft => area.left + margin,
                OverlayPosition::BottomRight | OverlayPosition::TopRight => area.right - w - margin,
                OverlayPosition::BottomCenter | OverlayPosition::TopCenter => {
                    area.left + ((area.right - area.left - w) as f32 / 2.0).round() as i32
                }
            };
            let y = match self.position {
                OverlayPosition::TopLeft | OverlayPosition::TopCenter | OverlayPosition::TopRight => area.top + margin,
                _ => area.bottom - h - margin,
            };
            self.origin = (x, y);
            self.period = Duration::from_secs_f64(1.0 / refresh_rate(&info.device) as f64);
        }
    }

    fn animating(&self, now: Instant) -> bool {
        if self.style.style == OverlayStyle::None {
            return false;
        }
        let moving = self.opacity.running(now) || self.enter.running(now);
        let wave = self.look.phase == OverlayPhase::Listening && self.style.show_waveform && self.analyser.is_some();
        let dots = self.look.phase == OverlayPhase::Transcribing && !self.style.reduced_motion;
        moving || wave || dots
    }

    fn tick(&mut self, now: Instant) {
        if self.hide_at.is_some_and(|at| now >= at) {
            self.hide_now();
        }
        if !self.visible {
            return;
        }
        let animating = self.animating(now);
        if self.was_animating && !animating {
            // One more frame at the end state.
            self.dirty = true;
        }
        self.was_animating = animating;
        if self.dirty || (animating && now >= self.next_frame) {
            self.draw(now);
            if now >= self.next_frame {
                self.next_frame += self.period;
                if self.next_frame <= now {
                    self.next_frame = now + self.period;
                }
            }
        }
    }

    fn next_wake(&self, now: Instant) -> Option<Instant> {
        let frame = (self.visible && self.animating(now)).then_some(self.next_frame);
        match (frame, self.hide_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn draw(&mut self, now: Instant) {
        let forced = std::mem::take(&mut self.dirty);
        if self.look.phase == OverlayPhase::Listening
            && self.style.show_waveform
            && let Some(analyser) = &self.analyser
        {
            let before = self.levels;
            analyser.byte_frequency_data(&mut self.spectrum);
            pill::update_levels(&mut self.levels, &self.spectrum);
            // A bar that moved less than a few hundredths of a pixel would draw the
            // same frame again (silence): skip it unless something else moves.
            let still = before.iter().zip(&self.levels).all(|(a, b)| (a - b).abs() < 0.002);
            let moving = self.opacity.running(now) || self.enter.running(now);
            if still && !moving && !forced {
                return;
            }
        }
        let motion = Motion {
            levels: &self.levels,
            dots_time: now.saturating_duration_since(self.dots_since).as_secs_f32(),
            enter: self.enter.value(now) as f32,
        };
        let (w, h) = window_size(self.dpr);
        if self.frame.as_ref().is_none_or(|f| f.width() != w || f.height() != h) {
            self.frame = Pixmap::new(w, h);
        }
        let Some(frame) = self.frame.as_mut() else { return };
        self.renderer.render(&self.look, &self.style, self.dpr, &motion, frame);
        if self.surface.as_ref().is_none_or(|s| s.width != w || s.height != h) {
            self.surface = Surface::new(w, h);
        }
        let opacity = self.opacity.value(now) as f32;
        if let Some(surface) = self.surface.as_mut() {
            surface.upload(frame.data());
            if let Err(err) = surface.present(self.hwnd, self.origin.0, self.origin.1, opacity) {
                log::debug!(target: "overlay", "UpdateLayeredWindow failed: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flow_core::config::OverlaySize;
    use flow_core::types::ThemeInfo;

    fn appearance() -> OverlayAppearance {
        OverlayAppearance {
            style: OverlayStyle::Pill,
            size: OverlaySize::Medium,
            show_waveform: true,
            sounds: false,
            theme: ThemeInfo::default(),
        }
    }

    #[test]
    fn creates_a_hidden_window_and_destroys_it() {
        let overlay = Overlay::create();
        assert!(overlay.hwnd.is_some());
        overlay.configure(appearance(), OverlayPosition::BottomCenter);
        overlay.set_analyser(Some(Arc::new(Analyser::new())));
        // Hidden stays hidden: no window appears.
        overlay.set_state(OverlayState::hidden());
        overlay.destroy();
        overlay.destroy();
    }

    #[test]
    #[ignore = "shows the pill on screen for a moment"]
    fn shows_and_hides_the_pill() {
        let overlay = Overlay::create();
        overlay.configure(appearance(), OverlayPosition::TopRight);
        overlay.set_state(OverlayState { phase: OverlayPhase::Done, ..OverlayState::default() });
        std::thread::sleep(Duration::from_millis(600));
        overlay.set_state(OverlayState::hidden());
        std::thread::sleep(Duration::from_millis(400));
    }
}
