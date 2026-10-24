//! The system clipboard, read out and written back in full. Stands in for
//! Electron's async `clipboard` API (`read`/`readText`/`write`/`writeText`/`clear`)
//! that `src/main/insert/inserter.ts` used, at the level of raw Win32 formats.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};

// Predefined clipboard formats (winuser.h).
pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_METAFILEPICT: u32 = 3;
pub const CF_OEMTEXT: u32 = 7;
pub const CF_DIB: u32 = 8;
pub const CF_PALETTE: u32 = 9;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_ENHMETAFILE: u32 = 14;
pub const CF_LOCALE: u32 = 16;
pub const CF_DIBV5: u32 = 17;
const CF_OWNERDISPLAY: u32 = 0x0080;
const CF_DSPBITMAP: u32 = 0x0082;
const CF_DSPMETAFILEPICT: u32 = 0x0083;
const CF_DSPENHMETAFILE: u32 = 0x008e;
/// `CF_PRIVATEFIRST` to `CF_GDIOBJLAST`: handles of unknown kind, freed by nobody or by GDI.
const CF_PRIVATE_AND_GDI: std::ops::RangeInclusive<u32> = 0x0200..=0x03ff;
/// Registered (named) formats start here.
pub const FIRST_REGISTERED_FORMAT: u32 = 0xc000;

/// Marker formats written by `write_clipboard_excluded`; they carry no content of their own.
pub const EXCLUSION_FORMATS: [&str; 3] =
    ["ExcludeClipboardContentFromMonitorProcessing", "CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"];
/// OLE bookkeeping: these hold handles owned by the app that copied, and must not be written back.
pub const OLE_FORMATS: [&str; 2] = ["DataObject", "Ole Private Data"];
/// Registered formats that Electron reads as standard types (`text/html`, `text/rtf`,
/// `image/png`) rather than as raw OS formats.
pub const STANDARD_NAMED_FORMATS: [&str; 3] = ["HTML Format", "Rich Text Format", "PNG"];

const CLIPBOARD_OPEN_TRIES: usize = 6;
const CLIPBOARD_RETRY: Duration = Duration::from_millis(4);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClipboardError {
    /// Another process held the clipboard for longer than the retries allow.
    #[error("the clipboard is in use by another app")]
    Busy,
    #[error("{0}")]
    Failed(String),
}

/// One clipboard format and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardEntry {
    pub format: u32,
    /// The registered name, for formats from [`FIRST_REGISTERED_FORMAT`] up.
    pub name: Option<String>,
    pub data: Vec<u8>,
}

impl ClipboardEntry {
    pub fn new(format: u32, name: Option<String>, data: Vec<u8>) -> Self {
        Self { format, name, data }
    }

    /// Plain text (`CF_UNICODETEXT`).
    pub fn text(text: &str) -> Self {
        Self::new(CF_UNICODETEXT, None, encode_text(text))
    }

    pub fn is_text(&self) -> bool {
        self.format == CF_UNICODETEXT
    }

    /// The text, for a `CF_UNICODETEXT` entry.
    pub fn as_text(&self) -> Option<String> {
        self.is_text().then(|| decode_text(&self.data))
    }

    /// Formats Windows adds alongside plain text: the ANSI/OEM versions and the locale.
    pub fn is_text_companion(&self) -> bool {
        matches!(self.format, CF_TEXT | CF_OEMTEXT | CF_LOCALE)
    }

    fn name_in(&self, names: &[&str]) -> bool {
        self.name.as_deref().is_some_and(|name| names.iter().any(|n| n.eq_ignore_ascii_case(name)))
    }

    /// One of the clipboard-history exclusion markers.
    pub fn is_exclusion_marker(&self) -> bool {
        self.name_in(&EXCLUSION_FORMATS)
    }

    /// OLE bookkeeping (`DataObject`, `Ole Private Data`).
    pub fn is_ole(&self) -> bool {
        self.name_in(&OLE_FORMATS)
    }

    /// An app-specific registered format: what Electron exposes as
    /// `electron application/osclipboard;format="…"`. Not every one can be written back.
    pub fn is_raw(&self) -> bool {
        self.name.is_some() && !self.name_in(&STANDARD_NAMED_FORMATS)
    }
}

/// The clipboard as read out in full.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipboardSnapshot {
    /// Every format that could be read, in clipboard order.
    pub entries: Vec<ClipboardEntry>,
    /// Nothing was on the clipboard at all (not even formats that could not be read).
    pub empty: bool,
}

/// The system clipboard. `Win32Clipboard` in the app; fakes in tests.
pub trait Clipboard: Send + Sync {
    /// Every format on the clipboard, copied out. Formats that are GDI handles,
    /// synthesised duplicates or OLE bookkeeping are skipped. Fails only when the
    /// clipboard cannot be opened.
    fn read(&self) -> Result<ClipboardSnapshot, ClipboardError>;
    /// The clipboard's plain text, or an empty string when it holds none.
    fn read_text(&self) -> Result<String, ClipboardError>;
    /// Replaces the clipboard's content with these formats.
    fn write(&self, entries: &[ClipboardEntry]) -> Result<(), ClipboardError>;
    /// Replaces the clipboard's content with plain text.
    fn write_text(&self, text: &str) -> Result<(), ClipboardError>;
    fn clear(&self);
}

/// The Windows clipboard.
pub fn system_clipboard() -> Arc<dyn Clipboard> {
    Arc::new(Win32Clipboard)
}

/// UTF-16LE with the terminating NUL, as `CF_UNICODETEXT` holds it.
pub fn encode_text(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect()
}

/// `CF_UNICODETEXT` bytes up to the first NUL.
pub fn decode_text(data: &[u8]) -> String {
    let (pairs, _) = data.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).take_while(|unit| *unit != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Held while the clipboard is open; closes it when dropped.
pub(crate) struct OpenedClipboard(());

impl Drop for OpenedClipboard {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

/// Another process (a clipboard manager, the target app) may hold the clipboard
/// for a few milliseconds; retry briefly rather than give up.
pub(crate) fn open_clipboard() -> Option<OpenedClipboard> {
    for attempt in 0..CLIPBOARD_OPEN_TRIES {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Some(OpenedClipboard(()));
        }
        if attempt + 1 < CLIPBOARD_OPEN_TRIES {
            thread::sleep(CLIPBOARD_RETRY);
        }
    }
    None
}

/// Copies bytes into a movable global block and hands it to the open clipboard.
pub(crate) fn set_clipboard_data(format: u32, bytes: &[u8]) -> bool {
    unsafe {
        // A zero-sized block cannot be locked; an empty format gets one zero byte.
        let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len().max(1)) else { return false };
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
        let _ = GlobalUnlock(memory);
        // On success the clipboard owns the memory; on failure it is still ours to free.
        if SetClipboardData(format, Some(HANDLE(memory.0))).is_err() {
            let _ = GlobalFree(Some(memory));
            return false;
        }
        true
    }
}

/// The bytes of a format whose data is a global memory block, while the clipboard is open.
fn read_global(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        if handle.0.is_null() {
            return None;
        }
        let memory = HGLOBAL(handle.0);
        let size = GlobalSize(memory);
        if size == 0 {
            // A zero-length block: the format's presence is its content.
            return Some(Vec::new());
        }
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            return None;
        }
        let data = std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(memory);
        Some(data)
    }
}

fn format_name(format: u32) -> Option<String> {
    if format < FIRST_REGISTERED_FORMAT {
        return None;
    }
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClipboardFormatNameW(format, &mut buffer) };
    (length > 0).then(|| String::from_utf16_lossy(&buffer[..(length as usize).min(buffer.len())]))
}

/// Formats whose data is a GDI object (or another handle that cannot be copied as bytes).
fn is_handle_format(format: u32) -> bool {
    matches!(
        format,
        CF_BITMAP
            | CF_METAFILEPICT
            | CF_PALETTE
            | CF_ENHMETAFILE
            | CF_OWNERDISPLAY
            | CF_DSPBITMAP
            | CF_DSPMETAFILEPICT
            | CF_DSPENHMETAFILE
    ) || CF_PRIVATE_AND_GDI.contains(&format)
}

/// The Windows clipboard, opened without an owner window like the TS koffi code did.
pub struct Win32Clipboard;

impl Clipboard for Win32Clipboard {
    fn read(&self) -> Result<ClipboardSnapshot, ClipboardError> {
        let _open = open_clipboard().ok_or(ClipboardError::Busy)?;
        let mut entries = Vec::new();
        let mut any = false;
        let mut text_done = false;
        let mut image_done = false;
        let mut format = 0;
        loop {
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            any = true;
            // Windows converts between some formats on demand and lists the
            // conversions right after the original. Keep one of each family:
            // the Unicode text (the others are made from it again), and the
            // first device-independent bitmap.
            let target = match format {
                CF_TEXT | CF_OEMTEXT | CF_UNICODETEXT => {
                    if text_done {
                        continue;
                    }
                    text_done = true;
                    CF_UNICODETEXT
                }
                CF_BITMAP | CF_DIB | CF_DIBV5 => {
                    if image_done {
                        continue;
                    }
                    image_done = true;
                    if format == CF_BITMAP { CF_DIB } else { format }
                }
                f if is_handle_format(f) => continue,
                f => f,
            };
            let name = format_name(target);
            let entry = ClipboardEntry::new(target, name, Vec::new());
            if entry.is_ole() {
                continue;
            }
            // A format that cannot be read cannot be restored; keep the rest.
            if let Some(data) = read_global(target) {
                entries.push(ClipboardEntry { data, ..entry });
            }
        }
        Ok(ClipboardSnapshot { entries, empty: !any })
    }

    fn read_text(&self) -> Result<String, ClipboardError> {
        let _open = open_clipboard().ok_or(ClipboardError::Busy)?;
        Ok(read_global(CF_UNICODETEXT).map(|data| decode_text(&data)).unwrap_or_default())
    }

    fn write(&self, entries: &[ClipboardEntry]) -> Result<(), ClipboardError> {
        let _open = open_clipboard().ok_or(ClipboardError::Busy)?;
        unsafe { EmptyClipboard() }.map_err(|e| ClipboardError::Failed(e.message()))?;
        for entry in entries {
            if !set_clipboard_data(entry.format, &entry.data) {
                let name = entry.name.clone().unwrap_or_else(|| entry.format.to_string());
                return Err(ClipboardError::Failed(format!("cannot write the {name} format")));
            }
        }
        Ok(())
    }

    fn write_text(&self, text: &str) -> Result<(), ClipboardError> {
        self.write(&[ClipboardEntry::text(text)])
    }

    fn clear(&self) {
        if let Some(_open) = open_clipboard() {
            let _ = unsafe { EmptyClipboard() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Platform, Win32Platform};

    #[test]
    fn encodes_and_decodes_text() {
        assert_eq!(encode_text("hi"), vec![b'h', 0, b'i', 0, 0, 0]);
        assert_eq!(decode_text(&encode_text("héllo 👋")), "héllo 👋");
        // Bytes after the NUL (a block rounded up) are ignored.
        assert_eq!(decode_text(&[b'a', 0, 0, 0, b'b', 0]), "a");
        assert_eq!(decode_text(&[]), "");
        assert_eq!(ClipboardEntry::text("x").as_text().as_deref(), Some("x"));
    }

    #[test]
    fn classifies_formats() {
        let named = |name: &str| ClipboardEntry::new(0xc123, Some(name.into()), Vec::new());
        assert!(ClipboardEntry::text("x").is_text());
        assert!(ClipboardEntry::new(CF_LOCALE, None, vec![9, 4, 0, 0]).is_text_companion());
        assert!(named("CanIncludeInClipboardHistory").is_exclusion_marker());
        assert!(named("excludeclipboardcontentfrommonitorprocessing").is_exclusion_marker());
        assert!(named("Ole Private Data").is_ole());
        assert!(named("DataObject").is_raw());
        assert!(named("Custom App Format").is_raw());
        assert!(!named("HTML Format").is_raw());
        assert!(!named("PNG").is_raw());
        assert!(!ClipboardEntry::new(CF_DIB, None, Vec::new()).is_raw());
        assert!(is_handle_format(CF_BITMAP) && is_handle_format(CF_ENHMETAFILE) && is_handle_format(0x0300));
        assert!(!is_handle_format(CF_DIB) && !is_handle_format(CF_UNICODETEXT) && !is_handle_format(0xc000));
    }

    /// The real clipboard, saved first and put back after. Only a plain-text or
    /// empty clipboard is written to (with history exclusion), since that is
    /// what can be restored exactly; anything richer is only read.
    #[test]
    fn round_trips_text_through_the_real_clipboard_and_restores_it() {
        let clipboard = Win32Clipboard;
        let Ok(saved) = clipboard.read() else { return }; // held by another app right now
        let text_only = saved.entries.iter().any(ClipboardEntry::is_text)
            && saved.entries.iter().all(|e| e.is_text() || e.is_text_companion() || e.is_exclusion_marker());
        if !saved.empty && !text_only {
            return;
        }
        let original = saved.entries.iter().find_map(ClipboardEntry::as_text);
        let platform = Win32Platform::new().expect("the Win32 platform starts");

        let probe = "Flow clipboard test \u{2014} \u{1F44B}\r\nsecond line";
        let wrote = platform.write_clipboard_excluded(probe);
        let read_back = clipboard.read_text();
        let snapshot = clipboard.read();

        // Put the user's clipboard back before asserting anything.
        match &original {
            Some(text) => {
                if !platform.write_clipboard_excluded(text) {
                    let _ = clipboard.write_text(text);
                }
            }
            None => clipboard.clear(),
        }
        assert_eq!(clipboard.read_text().ok(), Some(original.clone().unwrap_or_default()));

        if wrote {
            assert_eq!(read_back.ok().as_deref(), Some(probe));
            let snapshot = snapshot.expect("the clipboard reads");
            assert!(!snapshot.empty);
            assert_eq!(snapshot.entries.iter().find_map(ClipboardEntry::as_text).as_deref(), Some(probe));
            let markers = snapshot.entries.iter().filter(|e| e.is_exclusion_marker()).count();
            assert_eq!(markers, 3);
            // The ANSI and OEM conversions Windows offers are not saved twice.
            assert!(!snapshot.entries.iter().any(|e| e.format == CF_TEXT || e.format == CF_OEMTEXT));
        }
    }
}
