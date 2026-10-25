//! Presents the CPU-rendered frame: a top-down 32-bit DIB section the
//! renderer paints into directly, copied to the window with `BitBlt`. The
//! copy keeps the alpha bytes, so with the frame extended into the client
//! area (Mica) DWM composites the premultiplied pixels over the backdrop.

#![allow(unsafe_code)]

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GdiFlush, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
    };

    pub struct Surface {
        hwnd: HWND,
        dc: HDC,
        bitmap: HBITMAP,
        old: HGDIOBJ,
        bits: *mut u32,
        width: usize,
        height: usize,
    }

    impl Surface {
        pub fn new(hwnd: isize) -> Option<Self> {
            let hwnd = HWND(hwnd as *mut _);
            // SAFETY: a memory DC compatible with the screen; freed in Drop.
            let dc = unsafe { CreateCompatibleDC(None) };
            if dc.is_invalid() {
                return None;
            }
            Some(Self {
                hwnd,
                dc,
                bitmap: HBITMAP::default(),
                old: HGDIOBJ::default(),
                bits: std::ptr::null_mut(),
                width: 0,
                height: 0,
            })
        }

        fn release(&mut self) {
            if !self.bitmap.is_invalid() {
                // SAFETY: the bitmap was selected into our DC by `resize`.
                unsafe {
                    SelectObject(self.dc, self.old);
                    let _ = DeleteObject(self.bitmap.into());
                }
                self.bitmap = HBITMAP::default();
                self.bits = std::ptr::null_mut();
                self.width = 0;
                self.height = 0;
            }
        }

        /// Makes the buffer `width`×`height`: `None` if it could not be
        /// allocated, `Some(true)` when it is a new (blank) buffer.
        pub fn resize(&mut self, width: usize, height: usize) -> Option<bool> {
            if width == self.width && height == self.height && !self.bits.is_null() {
                return Some(false);
            }
            self.release();
            if width == 0 || height == 0 {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            // SAFETY: `info` describes a 32-bit top-down DIB; GDI owns the memory until DeleteObject.
            let bitmap = unsafe { CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) };
            let Ok(bitmap) = bitmap else { return None };
            if bits.is_null() {
                // SAFETY: we own the bitmap just created.
                unsafe {
                    let _ = DeleteObject(bitmap.into());
                }
                return None;
            }
            // SAFETY: selecting our own bitmap into our own DC.
            self.old = unsafe { SelectObject(self.dc, bitmap.into()) };
            self.bitmap = bitmap;
            self.bits = bits.cast();
            self.width = width;
            self.height = height;
            Some(true)
        }

        /// The pixels (`0xAARRGGBB`, top row first).
        pub fn pixels(&mut self) -> &mut [u32] {
            if self.bits.is_null() {
                return &mut [];
            }
            // SAFETY: GDI may still be writing to the section; flush before touching it.
            unsafe {
                let _ = GdiFlush();
            }
            // SAFETY: the DIB section holds width*height u32s while the bitmap lives,
            // and `&mut self` keeps anyone else from touching it.
            unsafe { std::slice::from_raw_parts_mut(self.bits, self.width * self.height) }
        }

        /// Copies the buffer to the window.
        pub fn present(&self) {
            if self.bits.is_null() {
                return;
            }
            // SAFETY: plain GDI calls on our window and our memory DC.
            unsafe {
                let window_dc = GetDC(Some(self.hwnd));
                if window_dc.is_invalid() {
                    return;
                }
                let _ = BitBlt(
                    window_dc,
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                    Some(self.dc),
                    0,
                    0,
                    SRCCOPY,
                );
                ReleaseDC(Some(self.hwnd), window_dc);
            }
        }
    }

    impl Drop for Surface {
        fn drop(&mut self) {
            self.release();
            // SAFETY: our memory DC, no longer used.
            unsafe {
                let _ = DeleteDC(self.dc);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    /// Elsewhere the frame is kept in memory only (tests, previews of the layout).
    pub struct Surface {
        pixels: Vec<u32>,
    }

    impl Surface {
        pub fn new(_hwnd: isize) -> Option<Self> {
            Some(Self { pixels: Vec::new() })
        }
        pub fn resize(&mut self, width: usize, height: usize) -> Option<bool> {
            let new = self.pixels.len() != width * height;
            self.pixels.resize(width * height, 0);
            (width > 0 && height > 0).then_some(new)
        }
        pub fn pixels(&mut self) -> &mut [u32] {
            &mut self.pixels
        }
        pub fn present(&self) {}
    }
}

pub use imp::Surface;
