//! Win32 helpers shared by the overlay and the tray.

use windows::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, HMONITOR,
    MONITORINFO, MONITORINFOEXW, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};
use windows::core::PCWSTR;

/// A NUL-terminated UTF-16 copy of `s`.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A layered window's backing store: a top-down 32-bit DIB in a memory DC.
pub(crate) struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub width: u32,
    pub height: u32,
}

impl Surface {
    pub(crate) fn new(width: u32, height: u32) -> Option<Self> {
        // SAFETY: plain GDI object creation; every handle is released in Drop.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    // Negative: rows top to bottom, like the pixmap.
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
            let old = SelectObject(dc, bitmap.into());
            Some(Self { dc, bitmap, old, bits: bits.cast(), width, height })
        }
    }

    /// Copies premultiplied RGBA (tiny-skia) into the premultiplied BGRA DIB.
    pub(crate) fn upload(&mut self, rgba: &[u8]) {
        let len = (self.width * self.height * 4) as usize;
        if rgba.len() != len {
            return;
        }
        // SAFETY: the DIB holds width * height 32-bit pixels and lives as long as self.
        let dst = unsafe { std::slice::from_raw_parts_mut(self.bits, len) };
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }

    /// Puts the surface on screen at (`x`, `y`) with a constant `opacity` on top
    /// of the per-pixel alpha.
    pub(crate) fn present(&self, hwnd: HWND, x: i32, y: i32, opacity: f32) -> windows::core::Result<()> {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let pos = POINT { x, y };
        let size = SIZE { cx: self.width as i32, cy: self.height as i32 };
        let src = POINT { x: 0, y: 0 };
        // SAFETY: the DC and bitmap are valid; the pointers outlive the call.
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&pos),
                Some(&size),
                Some(self.dc),
                Some(&src),
                windows::Win32::Foundation::COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: releases what `new` created, in reverse order.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// A monitor's work area and device name.
pub(crate) struct MonitorInfo {
    pub work: RECT,
    pub device: Vec<u16>,
}

pub(crate) fn monitor_info(monitor: HMONITOR) -> Option<MonitorInfo> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: cbSize announces the extended struct.
    let ok = unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
    if !ok.as_bool() {
        return None;
    }
    let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
    let mut device = info.szDevice[..len].to_vec();
    device.push(0);
    Some(MonitorInfo { work: info.monitorInfo.rcWork, device })
}

/// The refresh rate of the monitor named `device` (NUL-terminated), in Hz.
pub(crate) fn refresh_rate(device: &[u16]) -> u32 {
    use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    // SAFETY: `device` is NUL-terminated; dmSize is set.
    let ok = unsafe { EnumDisplaySettingsW(PCWSTR(device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) };
    if ok.as_bool() && mode.dmDisplayFrequency > 1 { mode.dmDisplayFrequency } else { 60 }
}
