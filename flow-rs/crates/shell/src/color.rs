//! Colours from `brand.css` and `overlay/styles.css`, and the `color-mix(in
//! oklab, …)` the cloud icon uses.

/// A straight (not premultiplied) sRGB colour with alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// 0..1
    pub a: f32,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn new(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// From a Win32 `COLORREF` (0x00BBGGRR).
    pub fn from_colorref(c: u32) -> Self {
        Self::rgb((c & 0xff) as u8, ((c >> 8) & 0xff) as u8, ((c >> 16) & 0xff) as u8)
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    pub fn to_skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.r, self.g, self.b, (self.a.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    /// `color-mix(in oklab, self p, other)`: `p` of this colour, the rest of `other`.
    pub fn mix_oklab(self, other: Rgba, p: f32) -> Rgba {
        let a = to_oklab(self);
        let b = to_oklab(other);
        let q = 1.0 - p;
        // Both colours are mixed premultiplied by alpha, as CSS Color 5 specifies.
        let alpha = self.a * p + other.a * q;
        if alpha <= 0.0 {
            return Rgba::new(0, 0, 0, 0.0);
        }
        let mixed = [
            (a[0] * self.a * p + b[0] * other.a * q) / alpha,
            (a[1] * self.a * p + b[1] * other.a * q) / alpha,
            (a[2] * self.a * p + b[2] * other.a * q) / alpha,
        ];
        from_oklab(mixed, alpha)
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn to_oklab(c: Rgba) -> [f32; 3] {
    let (r, g, b) = (srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab(lab: [f32; 3], alpha: f32) -> Rgba {
    let l = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    Rgba::new(linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b), alpha)
}

/// `brand.css`
pub mod brand {
    use super::Rgba;
    /// Live microphone. A fill only: never a text colour.
    pub const VOICE: Rgba = Rgba::rgb(0xff, 0x5a, 0x1f);
    /// Audio leaving the PC.
    pub const CLOUD: Rgba = Rgba::rgb(0x1f, 0x5e, 0xff);
    pub const OK: Rgba = Rgba::rgb(0x37, 0xd6, 0x7a);
    pub const ERR: Rgba = Rgba::rgb(0xff, 0x6b, 0x5e);
    pub const WHITE: Rgba = Rgba::rgb(0xff, 0xff, 0xff);
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
}

/// The Windows high-contrast colours the overlay's stylesheet names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemColors {
    /// `Canvas` (COLOR_WINDOW)
    pub canvas: Rgba,
    /// `CanvasText` (COLOR_WINDOWTEXT)
    pub canvas_text: Rgba,
    /// `Highlight` (COLOR_HIGHLIGHT)
    pub highlight: Rgba,
}

impl Default for SystemColors {
    /// The "High Contrast Black" theme.
    fn default() -> Self {
        Self { canvas: brand::BLACK, canvas_text: brand::WHITE, highlight: Rgba::rgb(0x1a, 0xeb, 0xff) }
    }
}

impl SystemColors {
    /// Reads the current system colours.
    #[cfg(windows)]
    pub fn current() -> Self {
        use windows::Win32::Graphics::Gdi::{COLOR_HIGHLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor};
        // SAFETY: GetSysColor has no preconditions.
        unsafe {
            Self {
                canvas: Rgba::from_colorref(GetSysColor(COLOR_WINDOW)),
                canvas_text: Rgba::from_colorref(GetSysColor(COLOR_WINDOWTEXT)),
                highlight: Rgba::from_colorref(GetSysColor(COLOR_HIGHLIGHT)),
            }
        }
    }

    #[cfg(not(windows))]
    pub fn current() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_in_oklab_like_css() {
        // color-mix(in oklab, #1f5eff 55%, #fff): #82adff in the Electron screenshot.
        let cloud = brand::CLOUD.mix_oklab(brand::WHITE, 0.55);
        assert_eq!(cloud, Rgba::rgb(0x82, 0xad, 0xff));
        assert_eq!(brand::WHITE.mix_oklab(brand::WHITE, 0.3), brand::WHITE);
        assert_eq!(brand::BLACK.mix_oklab(brand::BLACK, 0.3), brand::BLACK);
    }

    #[test]
    fn reads_colorrefs() {
        assert_eq!(Rgba::from_colorref(0x00ff_8000), Rgba::rgb(0x00, 0x80, 0xff));
    }
}
