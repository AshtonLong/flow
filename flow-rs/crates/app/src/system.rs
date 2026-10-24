//! The main process's link to Windows: the hidden window that receives system
//! broadcasts (resume, lock, theme changes, a second instance), the
//! single-instance lock, the system theme, launch at login and shell opens.

use std::ffi::c_void;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use flow_core::types::ThemeInfo;
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetMessageW, MSG, PBT_APMRESUMEAUTOMATIC,
    PBT_APMRESUMESUSPEND, PostMessageW, PostQuitMessage, RegisterClassW, SPI_GETCLIENTAREAANIMATION,
    SPI_GETHIGHCONTRAST, SW_SHOWNORMAL, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SystemParametersInfoW,
    TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_COPYDATA, WM_POWERBROADCAST, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE,
    WM_THEMECHANGED, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPED, WTS_SESSION_LOCK,
};
use windows::core::{PCWSTR, w};

const WINDOW_CLASS: PCWSTR = w!("FlowMainWindow.rs");
/// `WM_COPYDATA` tag for a second instance's command line.
const COPYDATA_ARGS: usize = 0x464C_4F57; // "FLOW"
/// `WM_DWMCOLORIZATIONCOLORCHANGED`, sent when the accent colour changes.
const WM_DWMCOLORIZATIONCOLORCHANGED: u32 = 0x0320;
/// Posted to the window to wake the loop and run queued main-thread work.
const WM_FLOW_WAKE: u32 = WM_APP + 1;
/// Posted to end the message loop.
const WM_FLOW_QUIT: u32 = WM_APP + 2;

/// The value Electron's `setLoginItemSettings` wrote, kept so an upgrade replaces it.
const RUN_VALUE: PCWSTR = w!("computer.flow.dictation");
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");

#[derive(Debug, Clone, PartialEq)]
pub enum SystemEvent {
    /// Another `Flow.exe` was started; these are its arguments.
    SecondInstance(Vec<String>),
    /// The machine woke from sleep.
    Resumed,
    /// The session was locked.
    Locked,
    /// The colour scheme, accent, contrast or animation settings changed.
    SettingsChanged,
}

type Handler = Arc<dyn Fn(SystemEvent) + Send + Sync>;
static HANDLER: OnceLock<Handler> = OnceLock::new();
static WINDOW: OnceLock<isize> = OnceLock::new();
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Takes the single-instance lock. On failure, forwards `args` to the running
/// instance and returns false.
pub fn acquire_single_instance(args: &[String]) -> bool {
    // The handle is deliberately leaked: the lock lives as long as the process.
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\computer.flow.dictation.rs")) };
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if handle.is_ok() && !already {
        return true;
    }
    if let Ok(hwnd) = unsafe { FindWindowW(WINDOW_CLASS, PCWSTR::null()) } {
        let payload = args.join("\u{1f}");
        let bytes = payload.as_bytes();
        let data = COPYDATASTRUCT { dwData: COPYDATA_ARGS, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut c_void };
        unsafe {
            SendMessageW(hwnd, WM_COPYDATA, Some(WPARAM(0)), Some(LPARAM(&data as *const _ as isize)));
        }
    }
    false
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let emit = |event: SystemEvent| {
        if let Some(handler) = HANDLER.get() {
            handler(event);
        }
    };
    match msg {
        WM_COPYDATA => {
            let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            if data.dwData == COPYDATA_ARGS && !data.lpData.is_null() {
                let bytes = unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
                let text = String::from_utf8_lossy(bytes);
                let args = text.split('\u{1f}').filter(|a| !a.is_empty()).map(str::to_string).collect();
                emit(SystemEvent::SecondInstance(args));
                return LRESULT(1);
            }
        }
        WM_POWERBROADCAST => {
            if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC || wparam.0 as u32 == PBT_APMRESUMESUSPEND {
                emit(SystemEvent::Resumed);
            }
            return LRESULT(1);
        }
        WM_WTSSESSION_CHANGE => {
            if wparam.0 as u32 == WTS_SESSION_LOCK {
                emit(SystemEvent::Locked);
            }
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
            emit(SystemEvent::SettingsChanged);
        }
        WM_FLOW_WAKE => {
            if let Some(wake) = WAKE.get() {
                wake();
            }
            return LRESULT(0);
        }
        WM_FLOW_QUIT => {
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Creates the hidden window that receives system broadcasts. Main thread only.
pub fn create_main_window(handler: Handler, wake: Box<dyn Fn() + Send + Sync>) -> windows::core::Result<()> {
    let _ = HANDLER.set(handler);
    let _ = WAKE.set(wake);
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: WINDOW_CLASS, ..Default::default() };
        RegisterClassW(&class);
        // A real top-level window (never shown): message-only windows miss broadcasts.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            WINDOW_CLASS,
            w!("Flow"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        let _ = WINDOW.set(hwnd.0 as isize);
    }
    Ok(())
}

/// Runs the main thread's message loop until [`quit`].
pub fn run_message_loop() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn post(msg: u32) {
    if let Some(&hwnd) = WINDOW.get() {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// Wakes the main thread to run its queued work. Any thread.
pub fn wake_main_thread() {
    post(WM_FLOW_WAKE);
}

/// Ends the message loop. Any thread.
pub fn quit() {
    post(WM_FLOW_QUIT);
}

// ── Theme ──────────────────────────────────────────────────────────────────

fn read_dword(key: &str, value: &str) -> Option<u32> {
    unsafe {
        let mut handle = HKEY::default();
        let key_w = wide(key);
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr()), Some(0), KEY_READ, &mut handle).is_err() {
            return None;
        }
        let value_w = wide(value);
        let mut data = 0u32;
        let mut size = 4u32;
        let mut kind = REG_DWORD;
        let status = RegQueryValueExW(
            handle,
            PCWSTR(value_w.as_ptr()),
            None,
            Some(&mut kind),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(handle);
        (status.is_ok() && kind == REG_DWORD).then_some(data)
    }
}

const FALLBACK_ACCENT: &str = "#0067c0";

fn accent_color() -> String {
    // `AccentColor` is 0xAABBGGRR.
    if let Some(abgr) = read_dword(r"Software\Microsoft\Windows\DWM", "AccentColor") {
        let (r, g, b) = (abgr & 0xff, (abgr >> 8) & 0xff, (abgr >> 16) & 0xff);
        return format!("#{r:02x}{g:02x}{b:02x}");
    }
    let mut argb = 0u32;
    let mut opaque = windows::core::BOOL(0);
    if unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.is_ok() {
        return format!("#{:06x}", argb & 0x00ff_ffff);
    }
    FALLBACK_ACCENT.into()
}

fn high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some(&mut hc as *mut _ as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && hc.dwFlags.contains(HCF_HIGHCONTRASTON)
    }
}

fn reduced_motion() -> bool {
    let mut animate = windows::core::BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut animate as *mut _ as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    ok && !animate.as_bool()
}

/// Whether Windows apps are set to dark mode.
pub fn system_prefers_dark() -> bool {
    read_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme") == Some(0)
}

/// The system theme with `general.theme` applied on top, as Electron's
/// `nativeTheme.themeSource` did.
pub fn system_theme(setting: flow_core::config::ThemeSetting) -> ThemeInfo {
    use flow_core::config::ThemeSetting;
    let dark = match setting {
        ThemeSetting::System => system_prefers_dark(),
        ThemeSetting::Light => false,
        ThemeSetting::Dark => true,
    };
    ThemeInfo { dark, accent: accent_color(), high_contrast: high_contrast(), reduced_motion: reduced_motion() }
}

// ── Launch at login ────────────────────────────────────────────────────────

/// Adds or removes the `Run` entry that starts Flow hidden in the tray.
pub fn set_launch_at_login(enabled: bool, exe: &Path) -> windows::core::Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, Some(0), KEY_SET_VALUE | KEY_READ, &mut key).ok()?;
        let result = if enabled {
            let command = wide(&format!("\"{}\" --hidden", exe.display()));
            let bytes = std::slice::from_raw_parts(command.as_ptr() as *const u8, command.len() * 2);
            RegSetValueExW(key, RUN_VALUE, Some(0), REG_SZ, Some(bytes)).ok()
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
            Ok(())
        };
        let _ = RegCloseKey(key);
        result
    }
}

// ── Shell ──────────────────────────────────────────────────────────────────

/// Opens a file, folder or URL with its default handler.
pub fn shell_open(target: &str) -> bool {
    let target = wide(target);
    let result = unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(target.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    // Values above 32 mean success.
    result.0 as isize > 32
}

/// Lets `pid` take the foreground (the settings window, when the tray asks for it).
pub fn allow_set_foreground(pid: u32) {
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
    }
}

/// Opts the process into per-monitor DPI awareness and sets its AppUserModelID.
pub fn init_process() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(w!("computer.flow.dictation"));
    }
}

#[allow(dead_code)]
fn _assert_handle_send(_: HANDLE) {}
