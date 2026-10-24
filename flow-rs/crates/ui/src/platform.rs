//! The few Win32 calls the settings window makes itself: the system theme
//! before the main process answers, high-contrast system colours, the window's
//! DWM attributes, the clipboard, and (for the mock only) the keyboard state.

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
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, GetClassNameW, IsIconic, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    use windows::core::BOOL;

    /// A window handle that can be stored (HWND itself is not `Send`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Handle(isize);

    impl Handle {
        fn hwnd(self) -> HWND {
            HWND(self.0 as *mut _)
        }
    }

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Vec` passed by `own_window`, alive for the enumeration.
        let found = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        found.push(hwnd);
        BOOL(1)
    }

    /// The top-level winit window created on this (the UI) thread.
    pub fn own_window() -> Option<Handle> {
        let mut found: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes into `found`, which outlives the call.
        unsafe {
            let _ = EnumThreadWindows(GetCurrentThreadId(), Some(collect), LPARAM((&raw mut found) as isize));
        }
        let mut class = [0u16; 64];
        found
            .into_iter()
            .find(|&hwnd| {
                // SAFETY: `class` is a valid buffer; hwnd came from the enumeration.
                let len = unsafe { GetClassNameW(hwnd, &mut class) };
                let name = String::from_utf16_lossy(&class[..len.max(0) as usize]);
                name == "Window Class" && unsafe { IsWindowVisible(hwnd) }.as_bool()
            })
            .map(|hwnd| Handle(hwnd.0 as isize))
    }

    /// Rounded corners (Windows 11) and a dark or light system frame.
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
    pub fn own_window() -> Option<Handle> {
        None
    }
    pub fn style(_handle: Handle, _dark: bool) {}
    pub fn bring_to_front(_handle: Handle) {}
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
