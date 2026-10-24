//! The Windows implementation of `Platform`: every Win32 call the dictation
//! path makes (foreground window, simulated input, key state) goes through this
//! file. Mirrors `src/main/platform/win32.ts`, with the `windows` crate in place
//! of koffi.

use std::ffi::c_void;
use std::thread;
use std::time::{Duration, Instant};

use flow_core::types::FocusedApp;
use tokio_util::sync::CancellationToken;
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::DataExchange::{EmptyClipboard, RegisterClipboardFormatW};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetClassNameW, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindow, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::{PCWSTR, PWSTR};

use super::{Platform, PlatformError};
use crate::clipboard::{CF_UNICODETEXT, EXCLUSION_FORMATS, encode_text, open_clipboard, set_clipboard_data};
use crate::injected::{INJECT_TAG, shortcut_keys};
use crate::vk::{MODIFIER_VKS, VK_MASK_KEY, is_extended_key, vk_from_key_id};

/// Characters per SendInput call, and the pause between calls, when typing.
/// Windows timers tick every ~16 ms whatever delay is asked for, so the chunk
/// size sets the top speed (about 4,000 characters a second). In practice every
/// injected event also runs each keyboard hook installed on the machine.
pub const TYPE_CHUNK_CHARS: usize = 64;
pub const TYPE_CHUNK_DELAY: Duration = Duration::from_millis(8);
const MODIFIER_POLL: Duration = Duration::from_millis(15);

/// Foreground windows that have no text field: the desktop, the taskbar, the task switcher.
const SHELL_CLASSES: [&str; 9] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
    "ForegroundStaging",
];

#[derive(Debug, Clone, Copy)]
struct KeyStroke {
    vk: u16,
    /// UTF-16 code unit for `KEYEVENTF_UNICODE`, or the hardware scan code.
    scan: u16,
    flags: KEYBD_EVENT_FLAGS,
}

/// A window id is the HWND as a decimal string; anything else is not a window.
fn parse_handle(window_id: &str) -> Option<HWND> {
    if window_id.is_empty() || !window_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: usize = window_id.parse().ok()?;
    (value != 0).then_some(HWND(value as *mut c_void))
}

fn window_id(hwnd: HWND) -> String {
    (hwnd.0 as usize).to_string()
}

fn base_name(full_path: &str) -> &str {
    full_path.rsplit('\\').next().unwrap_or(full_path)
}

fn wide_string(max_chars: usize, read: impl FnOnce(&mut [u16]) -> i32) -> String {
    let mut buffer = vec![0u16; max_chars];
    let length = read(&mut buffer);
    if length > 0 { String::from_utf16_lossy(&buffer[..(length as usize).min(max_chars)]) } else { String::new() }
}

fn cancelled(cancel: Option<&CancellationToken>) -> bool {
    cancel.is_some_and(CancellationToken::is_cancelled)
}

pub struct Win32Platform {
    self_elevated: bool,
}

impl Win32Platform {
    /// Fails when the `INPUT` layout is not what SendInput expects.
    pub fn new() -> Result<Self, PlatformError> {
        // The classic SendInput trap: a wrong INPUT layout makes every call fail silently.
        let expected = if cfg!(target_pointer_width = "64") { 40 } else { 28 };
        let size = size_of::<INPUT>();
        if size != expected {
            return Err(PlatformError::Init(format!("INPUT is {size} bytes, expected {expected}")));
        }
        let self_elevated = token_elevated(unsafe { GetCurrentProcess() }) == Some(true);
        Ok(Self { self_elevated })
    }

    fn any_modifier_down(&self) -> bool {
        MODIFIER_VKS.iter().any(|vk| key_state_down(*vk))
    }

    fn key_stroke(&self, key: &str, down: bool) -> Option<KeyStroke> {
        let vk = vk_from_key_id(key)?;
        let mut flags = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
        if is_extended_key(key) {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        Some(KeyStroke {
            vk,
            // Some apps (terminals, remote desktops, games) read the scan code rather than the key.
            scan: (unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } & 0xff) as u16,
            flags,
        })
    }

    fn key_tap(&self, key: &str) -> Vec<KeyStroke> {
        [true, false].into_iter().filter_map(|down| self.key_stroke(key, down)).collect()
    }
}

fn key_state_down(vk: u16) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16 & 0x8000) != 0
}

/// Returns how many events were inserted into the input stream. Every event is
/// tagged with [`INJECT_TAG`], so our own input hook ignores it.
fn send(strokes: &[KeyStroke]) -> usize {
    if strokes.is_empty() {
        return 0;
    }
    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|s| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(s.vk),
                    wScan: s.scan,
                    dwFlags: s.flags,
                    time: 0,
                    dwExtraInfo: INJECT_TAG,
                },
            },
        })
        .collect();
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) as usize }
}

/// The executable name and whether the process is elevated.
fn process_info(pid: u32) -> (String, bool) {
    let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => handle,
        // Access denied means the process is out of our reach, which is also
        // when Windows refuses our simulated input: treat it as elevated.
        Err(error) => return (String::new(), error.code() == ERROR_ACCESS_DENIED.to_hresult()),
    };
    let mut buffer = vec![0u16; 2048];
    let mut size = buffer.len() as u32;
    let path = PWSTR(buffer.as_mut_ptr());
    let ok = unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, path, &mut size) }.is_ok();
    let name = if ok {
        let full = String::from_utf16_lossy(&buffer[..(size as usize).min(buffer.len())]);
        base_name(&full).to_string()
    } else {
        String::new()
    };
    let elevated = token_elevated(handle).unwrap_or(true);
    let _ = unsafe { CloseHandle(handle) };
    (name, elevated)
}

/// True/false from the process token; `None` when the token cannot be opened (access denied).
fn token_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    if let Err(error) = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } {
        return if error.code() == ERROR_ACCESS_DENIED.to_hresult() { None } else { Some(false) };
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    }
    .is_ok();
    let _ = unsafe { CloseHandle(token) };
    Some(ok && elevation.TokenIsElevated != 0)
}

impl Platform for Win32Platform {
    fn available(&self) -> bool {
        true
    }

    fn self_elevated(&self) -> bool {
        self.self_elevated
    }

    // ── Focused window ───────────────────────────────────────────────────────

    fn get_focused_app(&self) -> Option<FocusedApp> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return None;
        }
        let (process_name, elevated) = process_info(pid);
        Some(FocusedApp {
            window_id: window_id(hwnd),
            process_name,
            title: wide_string(512, |buf| unsafe { GetWindowTextW(hwnd, buf) }),
            pid,
            elevated,
        })
    }

    fn is_window_alive(&self, window_id: &str) -> bool {
        parse_handle(window_id).is_some_and(|hwnd| unsafe { IsWindow(Some(hwnd)) }.as_bool())
    }

    fn focus_window(&self, window_id: &str) -> bool {
        let Some(hwnd) = parse_handle(window_id) else { return false };
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return false;
            }
            if GetForegroundWindow() == hwnd {
                return true;
            }
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
            if GetForegroundWindow() == hwnd {
                return true;
            }

            // Windows only lets the process that received the last input event take
            // the foreground. Injecting a no-op key makes that us; sharing an input
            // queue with the current foreground thread covers the remaining cases.
            self.mask_start_menu();
            let foreground = GetForegroundWindow();
            let this = GetCurrentThreadId();
            let other = if foreground.0.is_null() { 0 } else { GetWindowThreadProcessId(foreground, None) };
            let attached = other != 0 && other != this && AttachThreadInput(this, other, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(this, other, false);
            }
            GetForegroundWindow() == hwnd
        }
    }

    fn is_text_target(&self, app: Option<&FocusedApp>) -> bool {
        let Some(app) = app else { return false };
        let Some(hwnd) = parse_handle(&app.window_id) else { return false };
        // A closed window has no class name: that is "target closed", not "no target".
        let class_name = wide_string(256, |buf| unsafe { GetClassNameW(hwnd, buf) });
        !SHELL_CLASSES.contains(&class_name.as_str())
    }

    // ── Simulated input ──────────────────────────────────────────────────────

    fn type_text(&self, text: &str, cancel: Option<&CancellationToken>) -> Result<(), PlatformError> {
        let chars: Vec<char> = text.replace("\r\n", "\n").replace('\r', "\n").chars().collect();
        for (index, chunk) in chars.chunks(TYPE_CHUNK_CHARS).enumerate() {
            if cancelled(cancel) {
                return Ok(());
            }
            if index > 0 {
                thread::sleep(TYPE_CHUNK_DELAY);
            }
            if cancelled(cancel) {
                return Ok(());
            }
            let mut strokes = Vec::with_capacity(chunk.len() * 2);
            for &ch in chunk {
                // Only newlines become a key press. A tab stays a character: the Tab
                // key would move focus out of a form field and lose the rest of the text.
                if ch == '\n' {
                    strokes.extend(self.key_tap("Enter"));
                } else {
                    // A character outside the BMP is two code units, sent as two packets.
                    let mut units = [0u16; 2];
                    for &unit in ch.encode_utf16(&mut units).iter() {
                        strokes.push(KeyStroke { vk: 0, scan: unit, flags: KEYEVENTF_UNICODE });
                        strokes.push(KeyStroke { vk: 0, scan: unit, flags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP });
                    }
                }
            }
            // UIPI makes SendInput insert nothing when the foreground window is elevated.
            if send(&strokes) != strokes.len() {
                return Err(PlatformError::Blocked);
            }
        }
        Ok(())
    }

    fn send_shortcut(&self, binding: &str) -> bool {
        let Some(keys) = shortcut_keys(binding) else { return false };
        if keys.is_empty() {
            return false;
        }
        let mut strokes = Vec::with_capacity(keys.len() * 2);
        for key in &keys {
            let Some(stroke) = self.key_stroke(key, true) else { return false };
            strokes.push(stroke);
        }
        for key in keys.iter().rev() {
            let Some(stroke) = self.key_stroke(key, false) else { return false };
            strokes.push(stroke);
        }
        // One SendInput call is atomic: no real key event can land inside the chord.
        send(&strokes) == strokes.len()
    }

    fn wait_for_modifiers_up(&self, timeout: Duration, cancel: Option<&CancellationToken>) {
        let deadline = Instant::now() + timeout;
        while self.any_modifier_down() && Instant::now() < deadline && !cancelled(cancel) {
            thread::sleep(MODIFIER_POLL);
        }
    }

    fn is_key_down(&self, key: &str) -> Option<bool> {
        vk_from_key_id(key).map(key_state_down)
    }

    fn mask_start_menu(&self) {
        // VK 0xE8 is unassigned, so no app reacts to it, but it still counts as a
        // key pressed while Win/Alt is down — which is what stops the Start menu
        // (or a window's menu bar) from opening when the modifier is released.
        send(&[
            KeyStroke { vk: VK_MASK_KEY, scan: 0, flags: KEYBD_EVENT_FLAGS(0) },
            KeyStroke { vk: VK_MASK_KEY, scan: 0, flags: KEYEVENTF_KEYUP },
        ]);
    }

    // ── Clipboard ────────────────────────────────────────────────────────────

    fn write_clipboard_excluded(&self, text: &str) -> bool {
        let Some(_open) = open_clipboard() else { return false };
        if unsafe { EmptyClipboard() }.is_err() {
            return false;
        }
        if !set_clipboard_data(CF_UNICODETEXT, &encode_text(text)) {
            return false;
        }
        // Presence alone excludes the item from clipboard managers; the other
        // two formats hold a DWORD 0 meaning "no" (Win+V history, cloud sync).
        for name in EXCLUSION_FORMATS {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let format = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
            if format != 0 {
                set_clipboard_data(format, &[0, 0, 0, 0]);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    //! Read-only Win32 calls against the real desktop. Nothing here sends input
    //! or touches the clipboard.

    use super::*;

    fn platform() -> Win32Platform {
        Win32Platform::new().expect("the Win32 platform starts")
    }

    #[test]
    fn starts_with_a_correctly_sized_input_struct() {
        // create_platform falls back to NullPlatform when sizeof(INPUT) is not 40 (28 on 32-bit).
        assert!(crate::platform::create_platform().available());
        let _ = platform().self_elevated();
    }

    #[test]
    fn describes_the_foreground_window_when_there_is_one() {
        let platform = platform();
        // A headless session has no foreground window.
        let Some(app) = platform.get_focused_app() else { return };
        assert!(!app.window_id.is_empty() && app.window_id.bytes().all(|b| b.is_ascii_digit()));
        assert!(app.pid > 0);
        if !app.elevated {
            assert!(app.process_name.to_lowercase().ends_with(".exe"), "{}", app.process_name);
        }
        assert!(platform.is_window_alive(&app.window_id));
        let _ = platform.is_text_target(Some(&app));
    }

    #[test]
    fn rejects_window_ids_that_are_not_live_windows() {
        let platform = platform();
        for id in ["", "0", "abc", "-5", "1e3", " 12", "99999999999999999999999"] {
            assert!(!platform.is_window_alive(id), "{id}");
            assert!(!platform.focus_window(id), "{id}");
        }
        assert!(!platform.is_text_target(None));
        let bogus = FocusedApp {
            window_id: "abc".into(),
            process_name: String::new(),
            title: String::new(),
            pid: 0,
            elevated: false,
        };
        assert!(!platform.is_text_target(Some(&bogus)));
    }

    #[test]
    fn reads_key_state_and_refuses_shortcuts_it_cannot_send() {
        let platform = platform();
        assert!(platform.is_key_down("F24").is_some());
        assert!(platform.is_key_down("Mouse5").is_some());
        assert_eq!(platform.is_key_down("NotAKey"), None);
        assert!(!platform.send_shortcut(""));
        assert!(!platform.send_shortcut("Ctrl+Nope"));
        assert!(!platform.send_shortcut("Ctrl+Mouse4"));
    }

    #[test]
    fn wait_for_modifiers_up_honours_its_timeout_and_a_cancel() {
        let platform = platform();
        let started = Instant::now();
        platform.wait_for_modifiers_up(Duration::from_millis(60), None);
        let cancel = CancellationToken::new();
        cancel.cancel();
        platform.wait_for_modifiers_up(Duration::from_millis(60), Some(&cancel));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn typing_nothing_sends_nothing() {
        assert_eq!(platform().type_text("", None), Ok(()));
        // Cancelled before the first chunk: nothing is sent either.
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(platform().type_text("not typed", Some(&cancel)), Ok(()));
    }

    #[test]
    fn parses_window_ids() {
        assert_eq!(parse_handle("0"), None);
        assert_eq!(parse_handle("+1"), None);
        assert_eq!(parse_handle("1234").map(window_id).as_deref(), Some("1234"));
        assert_eq!(base_name(r"C:\Windows\System32\notepad.exe"), "notepad.exe");
        assert_eq!(base_name("notepad.exe"), "notepad.exe");
    }
}
