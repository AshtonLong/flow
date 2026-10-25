//! The few Win32 calls the settings window makes itself: the system theme
//! before the main process answers, high-contrast system colours, the window's
//! DWM attributes (Mica), the maximise button for snap layouts, the clipboard, and (for the mock only) the keyboard state.

#![allow(unsafe_code)]

use flow_core::types::ThemeInfo;

/// The Windows theme as the window can see it without the main process: the
/// stand-in for the TS `matchMedia` fallback in `lib/theme.ts`.
pub fn system_theme() -> ThemeInfo {
    ThemeInfo {
        dark: !apps_use_light_theme(),
        accent: "#0067c0".into(),
        high_contrast: high_contrast_on(),
        reduced_motion: !client_area_animation(),
    }
}

#[cfg(windows)]
fn apps_use_light_theme() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::w;
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: plain out-parameters sized for a DWORD.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    status.is_err() || value != 0
}

#[cfg(windows)]
fn high_contrast_on() -> bool {
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    // SAFETY: `hc` is a correctly sized HIGHCONTRASTW.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&raw mut hc).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && hc.dwFlags.contains(HCF_HIGHCONTRASTON)
}

#[cfg(windows)]
fn client_area_animation() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut on = windows::core::BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on.as_bool()
}

#[cfg(not(windows))]
fn apps_use_light_theme() -> bool {
    true
}
#[cfg(not(windows))]
fn high_contrast_on() -> bool {
    false
}
#[cfg(not(windows))]
fn client_area_animation() -> bool {
    true
}

/// The CSS system colours high contrast uses (`Canvas`, `CanvasText`, …) as `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemColors {
    pub canvas: u32,
    pub canvas_text: u32,
    pub button_face: u32,
    pub button_text: u32,
    pub highlight: u32,
    pub highlight_text: u32,
    pub link_text: u32,
    pub gray_text: u32,
}

#[cfg(windows)]
pub fn system_colors() -> SystemColors {
    use windows::Win32::Graphics::Gdi::{
        COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT,
        COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
    };
    // GetSysColor returns 0x00BBGGRR.
    let get = |index: SYS_COLOR_INDEX| {
        // SAFETY: no pointers involved.
        let bgr = unsafe { GetSysColor(index) };
        ((bgr & 0xff) << 16) | (bgr & 0xff00) | ((bgr >> 16) & 0xff)
    };
    SystemColors {
        canvas: get(COLOR_WINDOW),
        canvas_text: get(COLOR_WINDOWTEXT),
        button_face: get(COLOR_BTNFACE),
        button_text: get(COLOR_BTNTEXT),
        highlight: get(COLOR_HIGHLIGHT),
        highlight_text: get(COLOR_HIGHLIGHTTEXT),
        link_text: get(COLOR_HOTLIGHT),
        gray_text: get(COLOR_GRAYTEXT),
    }
}

#[cfg(not(windows))]
pub fn system_colors() -> SystemColors {
    SystemColors {
        canvas: 0x000000,
        canvas_text: 0xffffff,
        button_face: 0x000000,
        button_text: 0xffffff,
        highlight: 0x1aebff,
        highlight_text: 0x000000,
        link_text: 0xffff00,
        gray_text: 0x3ff23f,
    }
}

/// The settings window's HWND and what is done to it.
#[cfg(windows)]
pub mod window {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWM_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
        DwmSetWindowAttribute,
    };
    use windows::Win32::UI::Controls::MARGINS;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow};
    use windows::core::BOOL;

    /// A window handle that can be stored (HWND itself is not `Send`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Handle(isize);

    impl Handle {
        /// The raw HWND (from raw-window-handle); `None` for 0.
        pub fn from_raw(hwnd: isize) -> Option<Self> {
            (hwnd != 0).then_some(Self(hwnd))
        }

        pub(super) fn hwnd(self) -> HWND {
            HWND(self.0 as *mut _)
        }
    }

    /// Rounded corners (Windows 11) and a dark or light system frame (which
    /// also tints Mica).
    pub fn style(handle: Handle, dark: bool) {
        let hwnd = handle.hwnd();
        let corner = DWMWCP_ROUND;
        let dark = BOOL(dark as i32);
        // SAFETY: each attribute gets a pointer to a value of the documented size.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const corner).cast(),
                std::mem::size_of_val(&corner) as u32,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                std::mem::size_of_val(&dark) as u32,
            );
        }
    }

    /// Puts the Mica backdrop behind the whole window (`backgroundMaterial:
    /// 'mica'`), or takes it away. Mica shows through wherever the frame is
    /// not opaque, so the frame is extended over the client area. Returns
    /// whether Mica is on: it needs Windows 11 22H2, and older systems keep a
    /// solid window.
    pub fn backdrop(handle: Handle, mica: bool) -> bool {
        let hwnd = handle.hwnd();
        let kind: DWM_SYSTEMBACKDROP_TYPE = if mica { DWMSBT_MAINWINDOW } else { DWMSBT_NONE };
        // SAFETY: the attribute gets a pointer to a DWORD-sized value; MARGINS is plain data.
        unsafe {
            let set = DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                (&raw const kind).cast(),
                std::mem::size_of_val(&kind) as u32,
            )
            .is_ok();
            let on = mica && set;
            let inset = if on { -1 } else { 0 };
            let margins = MARGINS { cxLeftWidth: inset, cxRightWidth: inset, cyTopHeight: inset, cyBottomHeight: inset };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            on
        }
    }

    /// Restores and raises the window (the tray asked for Settings again).
    pub fn bring_to_front(handle: Handle) {
        let hwnd = handle.hwnd();
        // SAFETY: plain window calls on our own window.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(not(windows))]
pub mod window {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Handle;
    impl Handle {
        pub fn from_raw(_hwnd: isize) -> Option<Self> {
            None
        }
    }
    pub fn style(_handle: Handle, _dark: bool) {}
    pub fn backdrop(_handle: Handle, _mica: bool) -> bool {
        false
    }
    pub fn bring_to_front(_handle: Handle) {}
}

/// The caption buttons, as Windows sees them. Flow draws its own minimise,
/// maximise and close buttons; the window procedure is subclassed so that
/// `WM_NCHITTEST` answers `HTMINBUTTON`, `HTMAXBUTTON` and `HTCLOSE` over
/// them, as for Electron's native `titleBarOverlay` buttons. That gives
/// Windows 11's snap-layouts flyout on maximise, and keeps the buttons out of
/// egui's reach: they work above a modal's backdrop, and a click on them is
/// not a click outside a dialog. Hover, press and click arrive as `WM_NC*`
/// messages and are kept here for [`crate::shell`] to draw and act on.
#[cfg(windows)]
pub mod caption {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, GWLP_WNDPROC, HTCLOSE, HTMAXBUTTON, HTMINBUTTON, SetWindowLongPtrW, WM_NCHITTEST,
        WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_NCRBUTTONDOWN,
        WM_NCRBUTTONUP, WNDPROC,
    };

    use super::window::Handle;

    /// The buttons left to right: minimise, maximise, close.
    const CODES: [u32; 3] = [HTMINBUTTON, HTMAXBUTTON, HTCLOSE];
    const NONE: i32 = -1;

    struct Shared {
        ctx: egui::Context,
        previous: AtomicIsize,
        /// The three buttons in client pixels: left, top, right, bottom (empty when hidden).
        strip: [AtomicI32; 4],
        hover: AtomicI32,
        pressed: AtomicI32,
        clicked: AtomicI32,
        tracking: AtomicBool,
    }

    static SHARED: OnceLock<Shared> = OnceLock::new();

    fn button_of(code: usize) -> i32 {
        CODES.iter().position(|&c| c as usize == code).map_or(NONE, |i| i as i32)
    }

    impl Shared {
        /// The button under the screen point in `lparam`, if any.
        fn hit(&self, hwnd: HWND, lparam: LPARAM) -> i32 {
            let mut p = POINT { x: (lparam.0 & 0xffff) as i16 as i32, y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 };
            // SAFETY: `p` is a valid POINT; hwnd is the window being subclassed.
            if !unsafe { ScreenToClient(hwnd, &mut p) }.as_bool() {
                return NONE;
            }
            let [l, t, r, b] = self.strip.each_ref().map(|v| v.load(Ordering::Relaxed));
            if r <= l || p.x < l || p.x >= r || p.y < t || p.y >= b {
                return NONE;
            }
            (((p.x - l) * 3 / (r - l)) as i32).min(2)
        }

        fn set(&self, slot: &AtomicI32, value: i32) {
            if slot.swap(value, Ordering::Relaxed) != value {
                self.ctx.request_repaint();
            }
        }
    }

    unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let Some(shared) = SHARED.get() else { return LRESULT(0) };
        let button = button_of(wparam.0);
        match msg {
            WM_NCHITTEST => {
                let hit = shared.hit(hwnd, lparam);
                if hit != NONE {
                    return LRESULT(CODES[hit as usize] as isize);
                }
            }
            WM_NCMOUSEMOVE => {
                shared.set(&shared.hover, button);
                if shared.pressed.load(Ordering::Relaxed) != button {
                    shared.set(&shared.pressed, NONE);
                }
                if !shared.tracking.swap(true, Ordering::Relaxed) {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: a correctly sized TRACKMOUSEEVENT for our window.
                    let _ = unsafe { TrackMouseEvent(&mut track) };
                }
                if button != NONE {
                    return LRESULT(0);
                }
            }
            WM_NCMOUSELEAVE => {
                shared.tracking.store(false, Ordering::Relaxed);
                shared.set(&shared.hover, NONE);
                shared.set(&shared.pressed, NONE);
            }
            // Swallowed: DefWindowProc would draw classic buttons and track the press itself.
            WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if button != NONE => {
                shared.set(&shared.pressed, button);
                return LRESULT(0);
            }
            WM_NCLBUTTONUP if button != NONE => {
                if shared.pressed.load(Ordering::Relaxed) == button {
                    shared.clicked.store(button, Ordering::Relaxed);
                }
                shared.set(&shared.pressed, NONE);
                shared.ctx.request_repaint();
                return LRESULT(0);
            }
            // No system menu from a right click on the buttons.
            WM_NCRBUTTONDOWN | WM_NCRBUTTONUP if button != NONE => return LRESULT(0),
            _ => {}
        }
        // SAFETY: `previous` holds the window procedure SetWindowLongPtrW replaced.
        let previous = unsafe { std::mem::transmute::<isize, WNDPROC>(shared.previous.load(Ordering::Relaxed)) };
        // SAFETY: forwarding the message to the original procedure.
        unsafe { CallWindowProcW(previous, hwnd, msg, wparam, lparam) }
    }

    /// Subclasses the window (once per process). Returns whether it is in place.
    pub fn install(handle: Handle, ctx: &egui::Context) -> bool {
        let shared = Shared {
            ctx: ctx.clone(),
            previous: AtomicIsize::new(0),
            strip: [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)],
            hover: AtomicI32::new(NONE),
            pressed: AtomicI32::new(NONE),
            clicked: AtomicI32::new(NONE),
            tracking: AtomicBool::new(false),
        };
        if SHARED.set(shared).is_err() {
            return installed();
        }
        let Some(shared) = SHARED.get() else { return false };
        let procedure: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = procedure;
        // SAFETY: replaces the window procedure of our own window on its thread;
        // `procedure` forwards everything it does not handle.
        let previous = unsafe { SetWindowLongPtrW(handle.hwnd(), GWLP_WNDPROC, procedure as *const () as isize) };
        shared.previous.store(previous, Ordering::Relaxed);
        previous != 0
    }

    /// Whether Windows is reporting the buttons (else egui handles them).
    pub fn installed() -> bool {
        SHARED.get().is_some_and(|s| s.previous.load(Ordering::Relaxed) != 0)
    }

    /// Where the three buttons are, in client pixels (`None` hides them from Windows).
    pub fn set_strip(rect: Option<[i32; 4]>) {
        if let Some(shared) = SHARED.get() {
            for (slot, v) in shared.strip.iter().zip(rect.unwrap_or_default()) {
                slot.store(v, Ordering::Relaxed);
            }
        }
    }

    /// The hovered and the pressed button (0 minimise, 1 maximise, 2 close).
    pub fn state() -> (Option<usize>, Option<usize>) {
        let get = |slot: &AtomicI32| usize::try_from(slot.load(Ordering::Relaxed)).ok();
        SHARED.get().map_or((None, None), |s| (get(&s.hover), get(&s.pressed)))
    }

    /// The button clicked since the last call.
    pub fn take_click() -> Option<usize> {
        SHARED.get().and_then(|s| usize::try_from(s.clicked.swap(NONE, Ordering::Relaxed)).ok())
    }
}

#[cfg(not(windows))]
pub mod caption {
    use super::window::Handle;
    pub fn install(_handle: Handle, _ctx: &egui::Context) -> bool {
        false
    }
    pub fn installed() -> bool {
        false
    }
    pub fn set_strip(_rect: Option<[i32; 4]>) {}
    pub fn state() -> (Option<usize>, Option<usize>) {
        (None, None)
    }
    pub fn take_click() -> Option<usize> {
        None
    }
}

/// Plain-text clipboard. eframe is built without its clipboard feature (it
/// would pull in `arboard`), so copy and paste go through these.
#[cfg(windows)]
pub mod clipboard {
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    /// Another app may hold the clipboard for a moment; try a few times.
    fn open() -> bool {
        for _ in 0..5 {
            // SAFETY: no pointers; a null owner is allowed.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    pub fn get_text() -> Option<String> {
        if !open() {
            return None;
        }
        // SAFETY: the clipboard is open; the handle is locked only while read.
        let text = unsafe {
            match GetClipboardData(u32::from(CF_UNICODETEXT.0)) {
                Ok(handle) if !handle.0.is_null() => {
                    let global = HGLOBAL(handle.0);
                    let ptr = GlobalLock(global) as *const u16;
                    if ptr.is_null() {
                        None
                    } else {
                        let mut len = 0usize;
                        while *ptr.add(len) != 0 {
                            len += 1;
                        }
                        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                        let _ = GlobalUnlock(global);
                        Some(text)
                    }
                }
                _ => None,
            }
        };
        // SAFETY: we opened it above.
        unsafe {
            let _ = CloseClipboard();
        }
        text
    }

    pub fn set_text(text: &str) {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        if !open() {
            return;
        }
        // SAFETY: the clipboard is open; the buffer is sized for `wide` and
        // ownership passes to the system on success.
        unsafe {
            let _ = EmptyClipboard();
            if let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
                let ptr = GlobalLock(global) as *mut u16;
                if !ptr.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                    let _ = GlobalUnlock(global);
                    let _ = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(global.0)));
                }
            }
            let _ = CloseClipboard();
        }
    }
}

#[cfg(not(windows))]
pub mod clipboard {
    pub fn get_text() -> Option<String> {
        None
    }
    pub fn set_text(_text: &str) {}
}

/// Key ids (as the hook names them) of every key and mouse button held now.
/// Used by the mock's hotkey capture only.
#[cfg(windows)]
pub fn keys_down() -> Vec<&'static str> {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    const LETTERS: [&str; 26] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V",
        "W", "X", "Y", "Z",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    const FKEYS: [&str; 24] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15", "F16", "F17",
        "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    ];
    const NUMPAD: [&str; 10] =
        ["Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9"];
    const NAMED: [(i32, &str); 42] = [
        (0xA2, "LCtrl"),
        (0xA3, "RCtrl"),
        (0xA4, "LAlt"),
        (0xA5, "RAlt"),
        (0xA0, "LShift"),
        (0xA1, "RShift"),
        (0x5B, "LWin"),
        (0x5C, "RWin"),
        (0x20, "Space"),
        (0x1B, "Esc"),
        (0x0D, "Enter"),
        (0x09, "Tab"),
        (0x08, "Backspace"),
        (0x2E, "Delete"),
        (0x2D, "Insert"),
        (0x24, "Home"),
        (0x23, "End"),
        (0x21, "PageUp"),
        (0x22, "PageDown"),
        (0x26, "Up"),
        (0x28, "Down"),
        (0x25, "Left"),
        (0x27, "Right"),
        (0x14, "CapsLock"),
        (0x91, "ScrollLock"),
        (0x90, "NumLock"),
        (0x2C, "PrintScreen"),
        (0x13, "Pause"),
        (0xBD, "Minus"),
        (0xBB, "Equal"),
        (0xDB, "BracketLeft"),
        (0xDD, "BracketRight"),
        (0xDC, "Backslash"),
        (0xBA, "Semicolon"),
        (0xDE, "Quote"),
        (0xBC, "Comma"),
        (0xBE, "Period"),
        (0xBF, "Slash"),
        (0xC0, "Backquote"),
        (0x04, "Mouse3"),
        (0x05, "Mouse4"),
        (0x06, "Mouse5"),
    ];
    // SAFETY: GetAsyncKeyState takes a virtual-key code and nothing else.
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
    let mut keys = Vec::new();
    for (vk, name) in NAMED {
        if down(vk) {
            keys.push(name);
        }
    }
    for (i, name) in LETTERS.iter().enumerate() {
        if down(0x41 + i as i32) {
            keys.push(name);
        }
    }
    for (i, name) in DIGITS.iter().enumerate() {
        if down(0x30 + i as i32) {
            keys.push(name);
        }
    }
    for (i, name) in FKEYS.iter().enumerate() {
        if down(0x70 + i as i32) {
            keys.push(name);
        }
    }
    for (i, name) in NUMPAD.iter().enumerate() {
        if down(0x60 + i as i32) {
            keys.push(name);
        }
    }
    keys
}

#[cfg(not(windows))]
pub fn keys_down() -> Vec<&'static str> {
    Vec::new()
}
