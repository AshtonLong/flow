//! The tray icon and its menu: model switcher, pause, copy last, retry,
//! settings, quit. Port of `src/main/windows/tray.ts`.
//!
//! The icon lives on its own thread with a hidden window that receives the
//! icon's messages; a left click opens the same menu as a right click and a
//! double click opens Settings. The icon is added again when Explorer restarts
//! (`TaskbarCreated`) and redrawn for the taskbar's DPI.

#[cfg(not(windows))]
use std::sync::Arc;

/// What a menu choice asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    ActivateModel(String),
    SetPaused(bool),
    CopyLast,
    RetryLast,
    OpenSettings,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayModel {
    pub id: String,
    pub name: String,
    pub cloud: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayState {
    pub active_model_id: String,
    /// Models that can be switched to right now (installed, or connected).
    pub models: Vec<TrayModel>,
    pub paused: bool,
    pub has_last: bool,
    pub can_retry: bool,
    /// Set when the global hook could not start, so hotkeys do nothing.
    pub hook_failed: bool,
}

/// How a menu item shows its state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    None,
    Checkbox(bool),
    Radio(bool),
}

/// The menu as data (what `Menu.buildFromTemplate` receives).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item { label: String, enabled: bool, check: Check, action: Option<TrayAction> },
    Separator,
    Submenu { label: String, items: Vec<MenuEntry> },
}

fn item(label: impl Into<String>, action: TrayAction) -> MenuEntry {
    MenuEntry::Item { label: label.into(), enabled: true, check: Check::None, action: Some(action) }
}

fn disabled(label: impl Into<String>) -> MenuEntry {
    MenuEntry::Item { label: label.into(), enabled: false, check: Check::None, action: None }
}

fn active_name(state: &TrayState) -> &str {
    state
        .models
        .iter()
        .find(|m| m.id == state.active_model_id)
        .map_or(state.active_model_id.as_str(), |m| m.name.as_str())
}

/// The menu tray.ts builds for `state`.
pub fn build_menu(state: &TrayState) -> Vec<MenuEntry> {
    let model = |m: &TrayModel| MenuEntry::Item {
        label: m.name.clone(),
        enabled: true,
        check: Check::Radio(m.id == state.active_model_id),
        action: Some(TrayAction::ActivateModel(m.id.clone())),
    };
    let local: Vec<MenuEntry> = state.models.iter().filter(|m| !m.cloud).map(model).collect();
    let cloud: Vec<MenuEntry> = state.models.iter().filter(|m| m.cloud).map(model).collect();
    let mut switcher = Vec::new();
    let both = !local.is_empty() && !cloud.is_empty();
    switcher.extend(local);
    if both {
        switcher.push(MenuEntry::Separator);
    }
    switcher.extend(cloud);
    if switcher.is_empty() {
        switcher.push(disabled("No models are ready — open Settings"));
    }

    let mut menu = vec![
        MenuEntry::Submenu { label: format!("Model: {}", active_name(state)), items: switcher },
        MenuEntry::Separator,
        MenuEntry::Item {
            label: "Pause dictation".into(),
            enabled: true,
            check: Check::Checkbox(state.paused),
            // Electron flips the checkbox before the click handler reads it.
            action: Some(TrayAction::SetPaused(!state.paused)),
        },
        MenuEntry::Item {
            label: "Copy last transcript".into(),
            enabled: state.has_last,
            check: Check::None,
            action: Some(TrayAction::CopyLast),
        },
        MenuEntry::Item {
            label: "Retry last dictation".into(),
            enabled: state.can_retry,
            check: Check::None,
            action: Some(TrayAction::RetryLast),
        },
        MenuEntry::Separator,
    ];
    if state.hook_failed {
        menu.push(disabled("Hotkeys are unavailable (input hook failed)"));
    }
    menu.push(item("Settings…", TrayAction::OpenSettings));
    menu.push(item("Quit Flow", TrayAction::Quit));
    menu
}

/// The icon's tooltip.
pub fn tooltip(state: &TrayState) -> String {
    if state.paused { "Flow — paused".to_string() } else { format!("Flow — {}", active_name(state)) }
}

#[cfg(windows)]
pub use imp::Tray;

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
    use parking_lot::Mutex;
    use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetSystemMetricsForDpi,
        SetThreadDpiAwarenessContext,
    };
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
        NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
        GetCursorPos, GetMenuItemCount, GetMessageW, GetSystemMetrics, HICON, HMENU, ICONINFO, InsertMenuItemW,
        MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED, MFS_ENABLED, MFS_UNCHECKED, MFT_RADIOCHECK, MFT_SEPARATOR,
        MFT_STRING, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, MSG, PostMessageW, PostQuitMessage,
        RegisterClassExW, RegisterWindowMessageW, SM_CXSMICON, SM_MENUDROPALIGNMENT, SetForegroundWindow,
        TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON,
        TrackPopupMenuEx, TranslateMessage, WM_APP, WM_CONTEXTMENU, WM_DISPLAYCHANGE, WM_DPICHANGED,
        WM_LBUTTONDBLCLK, WM_NULL, WM_SETTINGCHANGE, WM_USER, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::{PWSTR, w};

    use super::{Check, MenuEntry, TrayAction, TrayState, build_menu, tooltip};
    use crate::win::wide;

    const WM_WAKE: u32 = WM_APP + 1;
    const WM_TRAY: u32 = WM_APP + 2;
    const NIN_SELECT: u32 = WM_USER;
    const NIN_KEYSELECT: u32 = WM_USER + 1;
    const ICON_ID: u32 = 1;

    static TRAY_PNG: [&[u8]; 3] = [
        include_bytes!("../../../../resources/icons/tray.png"),
        include_bytes!("../../../../resources/icons/tray@2x.png"),
        include_bytes!("../../../../resources/icons/tray@3x.png"),
    ];
    static PAUSED_PNG: [&[u8]; 3] = [
        include_bytes!("../../../../resources/icons/tray-paused.png"),
        include_bytes!("../../../../resources/icons/tray-paused@2x.png"),
        include_bytes!("../../../../resources/icons/tray-paused@3x.png"),
    ];

    enum Command {
        Update(Box<TrayState>),
        Destroy,
    }

    /// The tray icon. Methods only queue work for the tray thread.
    pub struct Tray {
        tx: Sender<Command>,
        hwnd: Option<isize>,
        thread: Mutex<Option<JoinHandle<()>>>,
    }

    impl Tray {
        /// Adds the icon (no menu or tooltip until the first `update`).
        pub fn create(on_action: Arc<dyn Fn(TrayAction) + Send + Sync>) -> Self {
            let (tx, rx) = unbounded();
            let (ready_tx, ready_rx) = bounded(1);
            let thread = std::thread::Builder::new()
                .name("flow-tray".into())
                .spawn(move || run(rx, ready_tx, on_action))
                .map_err(|err| log::error!(target: "tray", "could not start the tray thread: {err}"))
                .ok();
            let hwnd = if thread.is_some() { ready_rx.recv().ok().flatten() } else { None };
            Self { tx, hwnd, thread: Mutex::new(thread) }
        }

        fn send(&self, command: Command) {
            if self.tx.send(command).is_ok()
                && let Some(hwnd) = self.hwnd
            {
                // SAFETY: posting to the tray thread's window.
                let _ = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)) };
            }
        }

        /// Rebuilds the menu, icon and tooltip.
        pub fn update(&self, state: TrayState) {
            self.send(Command::Update(Box::new(state)));
        }

        /// Removes the icon and ends the tray thread.
        pub fn destroy(&self) {
            self.send(Command::Destroy);
            if let Some(thread) = self.thread.lock().take() {
                let _ = thread.join();
            }
        }

        /// The hidden window that owns the icon (for tests).
        pub fn window(&self) -> Option<isize> {
            self.hwnd
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            self.destroy();
        }
    }

    struct Inner {
        hwnd: HWND,
        rx: Receiver<Command>,
        on_action: Arc<dyn Fn(TrayAction) + Send + Sync>,
        taskbar_created: u32,
        state: Option<TrayState>,
        menu: Option<HMENU>,
        actions: Vec<TrayAction>,
        icon: Option<HICON>,
        tip: String,
        /// A menu is open: updates wait until it closes.
        menu_open: bool,
        pending: Option<TrayState>,
    }

    thread_local! {
        static INNER: RefCell<Option<Inner>> = const { RefCell::new(None) };
    }

    fn with<R>(f: impl FnOnce(&mut Inner) -> R) -> Option<R> {
        INNER.with(|cell| cell.try_borrow_mut().ok().and_then(|mut inner| inner.as_mut().map(f)))
    }

    fn run(rx: Receiver<Command>, ready: Sender<Option<isize>>, on_action: Arc<dyn Fn(TrayAction) + Send + Sync>) {
        // SAFETY: this thread's DPI awareness only.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let hwnd = match create_window() {
            Ok(h) => h,
            Err(err) => {
                log::error!(target: "tray", "could not create the tray window: {err}");
                let _ = ready.send(None);
                return;
            }
        };
        // SAFETY: registering a well-known message name.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        INNER.with(|cell| {
            *cell.borrow_mut() = Some(Inner {
                hwnd,
                rx,
                on_action,
                taskbar_created,
                state: None,
                menu: None,
                actions: Vec::new(),
                icon: None,
                tip: String::new(),
                menu_open: false,
                pending: None,
            });
        });
        with(|inner| {
            inner.refresh_icon(false);
            inner.add();
        });
        let _ = ready.send(Some(hwnd.0 as isize));
        let mut msg = MSG::default();
        // SAFETY: the standard message loop; ends at WM_QUIT.
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        INNER.with(|cell| cell.borrow_mut().take());
    }

    fn create_window() -> windows::core::Result<HWND> {
        // SAFETY: window class registration and creation on this thread.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("FlowTray");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            // A hidden top-level window: message-only windows miss the TaskbarCreated broadcast.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!("Flow tray"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_WAKE => {
                drain();
                return LRESULT(0);
            }
            WM_TRAY => {
                // NOTIFYICON_VERSION_4: the event is in the low word of lParam.
                match (lparam.0 as u32) & 0xffff {
                    NIN_SELECT | NIN_KEYSELECT | WM_CONTEXTMENU => show_menu(),
                    WM_LBUTTONDBLCLK => {
                        if let Some(on_action) = with(|inner| inner.on_action.clone()) {
                            on_action(TrayAction::OpenSettings);
                        }
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                with(|inner| {
                    let paused = inner.state.as_ref().is_some_and(|s| s.paused);
                    inner.refresh_icon(paused);
                    inner.modify();
                });
            }
            _ => {
                let recreated = with(|inner| inner.taskbar_created != 0 && msg == inner.taskbar_created);
                if recreated == Some(true) {
                    // Explorer restarted: the icon is gone.
                    with(|inner| {
                        let paused = inner.state.as_ref().is_some_and(|s| s.paused);
                        inner.refresh_icon(paused);
                        inner.add();
                    });
                    return LRESULT(0);
                }
            }
        }
        // SAFETY: default handling.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn drain() {
        loop {
            let Some(command) = with(|inner| inner.rx.try_recv().ok()).flatten() else { return };
            match command {
                Command::Update(state) => {
                    with(|inner| {
                        if inner.menu_open {
                            inner.pending = Some(*state);
                        } else {
                            inner.apply(*state);
                        }
                    });
                }
                Command::Destroy => {
                    with(|inner| inner.teardown());
                    // SAFETY: ends this thread's message loop.
                    unsafe { PostQuitMessage(0) };
                    return;
                }
            }
        }
    }

    fn show_menu() {
        let Some(Some((hwnd, menu))) = with(|inner| inner.menu.map(|m| (inner.hwnd, m))) else { return };
        with(|inner| inner.menu_open = true);
        let mut pt = POINT::default();
        // SAFETY: the classic tray-menu sequence: foreground, track, then WM_NULL.
        let chosen = unsafe {
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
            let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | align;
            let id = TrackPopupMenuEx(menu, flags.0, pt.x, pt.y, hwnd, None).0;
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            id
        };
        let action = with(|inner| {
            inner.menu_open = false;
            if let Some(state) = inner.pending.take() {
                inner.apply(state);
            }
            usize::try_from(chosen).ok().and_then(|id| id.checked_sub(1)).and_then(|i| inner.actions.get(i).cloned())
        })
        .flatten();
        if let Some(action) = action
            && let Some(on_action) = with(|inner| inner.on_action.clone())
        {
            on_action(action);
        }
    }

    /// Labels go to Win32 menus, where `&` marks a mnemonic.
    fn menu_text(label: &str) -> Vec<u16> {
        wide(&label.replace('&', "&&"))
    }

    fn append(menu: HMENU, entry: &MenuEntry, actions: &mut Vec<TrayAction>) {
        // SAFETY: `menu` is a live menu; strings outlive the insert call.
        unsafe {
            let position = GetMenuItemCount(Some(menu)).max(0) as u32;
            match entry {
                MenuEntry::Separator => {
                    let info = MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_FTYPE,
                        fType: MFT_SEPARATOR,
                        ..Default::default()
                    };
                    let _ = InsertMenuItemW(menu, position, true, &info);
                }
                MenuEntry::Item { label, enabled, check, action } => {
                    let mut text = menu_text(label);
                    let id = match action {
                        Some(a) => {
                            actions.push(a.clone());
                            actions.len() as u32
                        }
                        None => 0,
                    };
                    let (radio, checked) = match check {
                        Check::None => (false, false),
                        Check::Checkbox(c) => (false, *c),
                        Check::Radio(c) => (true, *c),
                    };
                    let mut state = if *enabled { MFS_ENABLED } else { MFS_DISABLED };
                    state |= if checked { MFS_CHECKED } else { MFS_UNCHECKED };
                    let info = MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_STRING,
                        fType: if radio { MFT_STRING | MFT_RADIOCHECK } else { MFT_STRING },
                        fState: state,
                        wID: id,
                        dwTypeData: PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    let _ = InsertMenuItemW(menu, position, true, &info);
                }
                MenuEntry::Submenu { label, items } => {
                    let Ok(sub) = CreatePopupMenu() else { return };
                    for child in items {
                        append(sub, child, actions);
                    }
                    let mut text = menu_text(label);
                    let info = MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_FTYPE | MIIM_STRING | MIIM_SUBMENU,
                        fType: MFT_STRING,
                        hSubMenu: sub,
                        dwTypeData: PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    let _ = InsertMenuItemW(menu, position, true, &info);
                }
            }
        }
    }

    impl Inner {
        fn data(&self, flags: windows::Win32::UI::Shell::NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: ICON_ID,
                uFlags: flags,
                uCallbackMessage: WM_TRAY,
                ..Default::default()
            };
            if let Some(icon) = self.icon {
                nid.hIcon = icon;
            }
            let tip: Vec<u16> = self.tip.encode_utf16().take(nid.szTip.len() - 1).collect();
            nid.szTip[..tip.len()].copy_from_slice(&tip);
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            nid
        }

        fn add(&mut self) {
            let nid = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP);
            // SAFETY: a fully initialised NOTIFYICONDATAW for our window.
            unsafe {
                if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                    // Already there (a repeated TaskbarCreated): update instead.
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
                }
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
            }
        }

        fn modify(&mut self) {
            let nid = self.data(NIF_ICON | NIF_TIP | NIF_SHOWTIP);
            // SAFETY: as in `add`.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }

        fn apply(&mut self, state: TrayState) {
            let entries = build_menu(&state);
            let mut actions = Vec::new();
            // SAFETY: the new menu replaces (and frees) the old one, which is not open.
            unsafe {
                if let Ok(menu) = CreatePopupMenu() {
                    for entry in &entries {
                        append(menu, entry, &mut actions);
                    }
                    if let Some(old) = self.menu.replace(menu) {
                        let _ = DestroyMenu(old);
                    }
                }
            }
            self.actions = actions;
            self.tip = tooltip(&state);
            let paused_changed = self.state.as_ref().is_none_or(|s| s.paused != state.paused);
            if paused_changed {
                self.refresh_icon(state.paused);
            }
            self.state = Some(state);
            self.modify();
        }

        /// Redraws the icon for the current DPI.
        fn refresh_icon(&mut self, paused: bool) {
            // SAFETY: plain queries on our window.
            let size = unsafe {
                let dpi = GetDpiForWindow(self.hwnd).max(96);
                GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32
            };
            let set = if paused { &PAUSED_PNG } else { &TRAY_PNG };
            match make_icon(set, size) {
                Some(icon) => {
                    if let Some(old) = self.icon.replace(icon) {
                        // SAFETY: the shell copies the icon; ours can go once replaced.
                        unsafe {
                            let _ = DestroyIcon(old);
                        }
                    }
                }
                None => log::warn!(target: "tray", "could not build the tray icon"),
            }
        }

        fn teardown(&mut self) {
            let nid = self.data(NIF_ICON);
            // SAFETY: removes our icon and frees our handles.
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                if let Some(menu) = self.menu.take() {
                    let _ = DestroyMenu(menu);
                }
                if let Some(icon) = self.icon.take() {
                    let _ = DestroyIcon(icon);
                }
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    /// Decodes an RGBA PNG into a premultiplied pixmap.
    fn decode_png(bytes: &[u8]) -> Option<Pixmap> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width, info.height);
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => buf[..(w * h * 4) as usize].to_vec(),
            png::ColorType::Rgb => {
                buf[..(w * h * 3) as usize].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
            }
            png::ColorType::GrayscaleAlpha => {
                buf[..(w * h * 2) as usize].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
            }
            png::ColorType::Grayscale => buf[..(w * h) as usize].iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return None,
        };
        let mut pixmap = Pixmap::new(w, h)?;
        for (dst, src) in pixmap.data_mut().as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
            let a = src[3] as u32;
            dst[0] = ((src[0] as u32 * a + 127) / 255) as u8;
            dst[1] = ((src[1] as u32 * a + 127) / 255) as u8;
            dst[2] = ((src[2] as u32 * a + 127) / 255) as u8;
            dst[3] = src[3];
        }
        Some(pixmap)
    }

    /// The best representation (@1x/@2x/@3x) scaled to exactly `size` px, as nativeImage
    /// picks the closest scale factor.
    pub(super) fn icon_pixels(set: &[&[u8]; 3], size: u32) -> Option<Pixmap> {
        let images: Vec<Pixmap> = set.iter().filter_map(|b| decode_png(b)).collect();
        let source = images.iter().find(|p| p.width() >= size).or(images.last())?;
        if source.width() == size {
            return Some(source.clone());
        }
        let mut out = Pixmap::new(size, size)?;
        let s = size as f32 / source.width() as f32;
        let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
        out.draw_pixmap(0, 0, source.as_ref(), &paint, Transform::from_scale(s, s), None);
        Some(out)
    }

    fn make_icon(set: &[&[u8]; 3], size: u32) -> Option<HICON> {
        let pixels = icon_pixels(set, size)?;
        // SAFETY: GDI objects created here are released before returning; the
        // icon keeps its own copies.
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size as i32,
                    biHeight: -(size as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            if bits.is_null() {
                let _ = DeleteObject(color.into());
                return None;
            }
            let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (size * size * 4) as usize);
            // Icons take straight (not premultiplied) BGRA.
            for (d, p) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pixels.pixels()) {
                let c = p.demultiply();
                d[0] = c.blue();
                d[1] = c.green();
                d[2] = c.red();
                d[3] = c.alpha();
            }
            let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
            let icon = CreateIconIndirect_(color, mask);
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            icon
        }
    }

    #[allow(non_snake_case)]
    unsafe fn CreateIconIndirect_(
        color: windows::Win32::Graphics::Gdi::HBITMAP,
        mask: windows::Win32::Graphics::Gdi::HBITMAP,
    ) -> Option<HICON> {
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        // SAFETY: both bitmaps are valid for the call.
        unsafe { windows::Win32::UI::WindowsAndMessaging::CreateIconIndirect(&info).ok() }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn picks_and_scales_icon_representations() {
            for size in [16, 20, 24, 32, 40, 48, 64] {
                let p = icon_pixels(&TRAY_PNG, size).expect("icon");
                assert_eq!((p.width(), p.height()), (size, size));
                assert!(p.pixels().iter().any(|c| c.alpha() > 0));
            }
            assert!(icon_pixels(&PAUSED_PNG, 16).is_some());
        }

        #[test]
        fn builds_hicons() {
            let icon = make_icon(&TRAY_PNG, 20).expect("HICON");
            // SAFETY: freeing the icon we just made.
            unsafe {
                let _ = DestroyIcon(icon);
            }
        }

        #[test]
        #[ignore = "adds a real tray icon for a moment"]
        fn adds_updates_and_removes_the_icon() {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let tray = Tray::create(Arc::new(move |a| {
                let _ = tx.send(a);
            }));
            assert!(tray.window().is_some());
            tray.update(TrayState {
                active_model_id: "m".into(),
                models: vec![super::super::TrayModel { id: "m".into(), name: "Model & Co".into(), cloud: false }],
                ..TrayState::default()
            });
            std::thread::sleep(std::time::Duration::from_millis(1500));
            // The shell knows where the icon is: it was added.
            let id = windows::Win32::UI::Shell::NOTIFYICONIDENTIFIER {
                cbSize: std::mem::size_of::<windows::Win32::UI::Shell::NOTIFYICONIDENTIFIER>() as u32,
                hWnd: HWND(tray.window().unwrap_or_default() as *mut _),
                uID: ICON_ID,
                ..Default::default()
            };
            // SAFETY: a query about our own icon.
            let rect = unsafe { windows::Win32::UI::Shell::Shell_NotifyIconGetRect(&id) };
            assert!(rect.is_ok(), "icon not found: {rect:?}");
            tray.update(TrayState { paused: true, ..TrayState::default() });
            std::thread::sleep(std::time::Duration::from_millis(1500));
            tray.destroy();
        }

        #[test]
        fn menu_labels_escape_ampersands() {
            let text = menu_text("Speech & Notes");
            assert_eq!(String::from_utf16_lossy(&text[..text.len() - 1]), "Speech && Notes");
        }
    }
}

/// Keeps the signature in one place for non-Windows builds (nothing to show).
#[cfg(not(windows))]
pub struct Tray;

#[cfg(not(windows))]
impl Tray {
    pub fn create(_on_action: Arc<dyn Fn(TrayAction) + Send + Sync>) -> Self {
        Self
    }
    pub fn update(&self, _state: TrayState) {}
    pub fn destroy(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: &str, cloud: bool) -> TrayModel {
        TrayModel { id: id.into(), name: name.into(), cloud }
    }

    fn state() -> TrayState {
        TrayState {
            active_model_id: "parakeet-tdt-0.6b-v2".into(),
            models: vec![
                model("parakeet-tdt-0.6b-v2", "Parakeet TDT 0.6B v2", false),
                model("whisper-base.en", "Whisper base.en", false),
                model("groq/whisper-large-v3-turbo", "Groq Whisper Large v3 Turbo", true),
            ],
            paused: false,
            has_last: true,
            can_retry: false,
            hook_failed: false,
        }
    }

    #[test]
    fn menu_matches_tray_ts() {
        let menu = build_menu(&state());
        let MenuEntry::Submenu { label, items } = &menu[0] else { panic!("model submenu first") };
        assert_eq!(label, "Model: Parakeet TDT 0.6B v2");
        assert_eq!(items.len(), 4, "two local, a separator, one cloud");
        assert_eq!(items[2], MenuEntry::Separator);
        assert_eq!(
            items[0],
            MenuEntry::Item {
                label: "Parakeet TDT 0.6B v2".into(),
                enabled: true,
                check: Check::Radio(true),
                action: Some(TrayAction::ActivateModel("parakeet-tdt-0.6b-v2".into())),
            }
        );
        assert!(matches!(&items[1], MenuEntry::Item { check: Check::Radio(false), .. }));
        let labels: Vec<&str> = menu
            .iter()
            .map(|e| match e {
                MenuEntry::Item { label, .. } | MenuEntry::Submenu { label, .. } => label.as_str(),
                MenuEntry::Separator => "-",
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Model: Parakeet TDT 0.6B v2",
                "-",
                "Pause dictation",
                "Copy last transcript",
                "Retry last dictation",
                "-",
                "Settings…",
                "Quit Flow"
            ]
        );
        assert!(matches!(&menu[2], MenuEntry::Item { check: Check::Checkbox(false), action: Some(TrayAction::SetPaused(true)), .. }));
        assert!(matches!(&menu[3], MenuEntry::Item { enabled: true, .. }));
        assert!(matches!(&menu[4], MenuEntry::Item { enabled: false, .. }));
    }

    #[test]
    fn no_models_and_a_failed_hook() {
        let s = TrayState {
            active_model_id: "gone".into(),
            models: vec![],
            paused: true,
            hook_failed: true,
            ..TrayState::default()
        };
        let menu = build_menu(&s);
        let MenuEntry::Submenu { label, items } = &menu[0] else { panic!() };
        assert_eq!(label, "Model: gone");
        assert_eq!(
            items,
            &vec![MenuEntry::Item {
                label: "No models are ready — open Settings".into(),
                enabled: false,
                check: Check::None,
                action: None
            }]
        );
        assert!(matches!(&menu[2], MenuEntry::Item { check: Check::Checkbox(true), action: Some(TrayAction::SetPaused(false)), .. }));
        assert_eq!(
            menu[6],
            MenuEntry::Item {
                label: "Hotkeys are unavailable (input hook failed)".into(),
                enabled: false,
                check: Check::None,
                action: None
            }
        );
        assert_eq!(tooltip(&s), "Flow — paused");
    }

    #[test]
    fn only_cloud_models_need_no_separator() {
        let mut s = state();
        s.models.retain(|m| m.cloud);
        let MenuEntry::Submenu { items, .. } = &build_menu(&s)[0] else { panic!() };
        assert_eq!(items.len(), 1);
        assert_eq!(tooltip(&s), "Flow — parakeet-tdt-0.6b-v2");
        assert_eq!(tooltip(&state()), "Flow — Parakeet TDT 0.6B v2");
    }
}
