//! Every colour and shadow of the window, for light, dark and high contrast.
//! Port of the custom properties in `common/brand.css` and the `:root`,
//! `[data-theme='dark']` and `[data-contrast='high']` blocks of
//! `settings/styles.css`. Field names follow the CSS names (`--fg-2` → `fg_2`).

use egui::{Color32, Vec2, vec2};

use crate::platform::SystemColors;
use crate::stores::Appearance;

/// `rgb(r g b / a)` as egui's premultiplied colour.
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, (a * 255.0 + 0.5) as u8)
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// One layer of a CSS `box-shadow` (outer shadows only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxShadow {
    pub offset: Vec2,
    /// The CSS blur radius (a Gaussian with σ = blur / 2).
    pub blur: f32,
    pub spread: f32,
    pub color: Color32,
}

impl BoxShadow {
    /// A hard ring `0 0 0 <width>px <color>`.
    pub const fn ring(width: f32, color: Color32) -> Self {
        Self { offset: Vec2::ZERO, blur: 0.0, spread: width, color }
    }

    /// `0 <y> <blur> <spread> <color>`, in rem where the CSS uses rem (1 rem = 16 px).
    pub const fn drop(y: f32, blur: f32, spread: f32, color: Color32) -> Self {
        Self { offset: vec2(0.0, y), blur, spread, color }
    }
}

/// Brand colours (`common/brand.css`).
pub mod brand {
    use super::{Color32, hex};
    pub const PAPER: Color32 = hex(0xe9ecf0);
    pub const SURFACE: Color32 = hex(0xffffff);
    pub const INK: Color32 = hex(0x000000);
    pub const INK_2: Color32 = hex(0x3b3f45);
    pub const INK_3: Color32 = hex(0x5b6068);
    /// Live microphone. A fill only: never a text colour.
    pub const VOICE: Color32 = hex(0xff5a1f);
    /// Audio leaving the PC.
    pub const CLOUD: Color32 = hex(0x1f5eff);
    pub const OK: Color32 = hex(0x37d67a);
    pub const ERR: Color32 = hex(0xff6b5e);
    /// The side wall of a keycap.
    pub const WALL: Color32 = hex(0xb7bec8);
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,

    pub voice: Color32,
    pub cloud: Color32,
    pub brand_ok: Color32,
    pub brand_err: Color32,
    pub wall: Color32,

    /// The window background (paper).
    pub mica: Color32,
    pub fg: Color32,
    pub fg_2: Color32,
    pub fg_3: Color32,
    pub fg_disabled: Color32,
    pub focus: Color32,

    pub layer: Color32,
    pub card: Color32,
    pub inset: Color32,
    pub flyout: Color32,
    pub line: Color32,
    pub line_strong: Color32,

    pub ctl: Color32,
    pub ctl_hover: Color32,
    pub ctl_press: Color32,
    pub ctl_disabled: Color32,
    pub ctl_line: Color32,
    pub ctl_line_top: Color32,
    pub ctl_line_bottom: Color32,
    pub ctl_wall: Color32,
    pub input_line: Color32,
    pub input_focus: Color32,
    pub subtle_hover: Color32,
    pub subtle_press: Color32,
    pub switch_off: Color32,
    pub switch_line: Color32,
    pub switch_knob: Color32,
    pub smoke: Color32,
    pub shadow_card: Vec<BoxShadow>,
    pub shadow_flyout: Vec<BoxShadow>,
    pub shadow_dialog: Vec<BoxShadow>,

    pub danger: Color32,
    pub danger_bg: Color32,
    pub warn: Color32,
    pub warn_bg: Color32,
    pub ok: Color32,
    pub ok_bg: Color32,

    /// Controls are ink: a checked switch and the selected page are black (white in dark).
    pub accent_fill: Color32,
    pub on_accent: Color32,
    pub accent_text: Color32,
    pub accent_hover: Color32,
    pub accent_press: Color32,
    pub accent_soft: Color32,
    /// The primary button is the site's dark keycap.
    pub key_ink_top: Color32,
    pub key_ink_wall: Color32,
    /// `.btn-accent:hover`: `color-mix(in oklab, key-ink-top 88%, on-accent)`.
    pub key_ink_hover: Color32,

    /// High contrast draws `1px solid CanvasText` around cards, badges, meter
    /// tracks, dialogs and flyouts.
    pub hc_border: Option<Color32>,
    /// High contrast outlines the selected page and highlighted menu items (`Highlight`).
    pub hc_outline: Option<Color32>,
}

impl Tokens {
    pub fn new(appearance: Appearance, system: &SystemColors) -> Self {
        let mut tokens = if appearance.dark { Self::dark() } else { Self::light() };
        if appearance.high_contrast {
            tokens.apply_high_contrast(system);
        }
        tokens.reduced_motion = appearance.reduced_motion;
        tokens
    }

    pub fn light() -> Self {
        let key_ink_top = hex(0x25282e);
        let on_accent = hex(0xffffff);
        Self {
            dark: false,
            high_contrast: false,
            reduced_motion: false,
            voice: brand::VOICE,
            cloud: brand::CLOUD,
            brand_ok: brand::OK,
            brand_err: brand::ERR,
            wall: brand::WALL,

            mica: brand::PAPER,
            fg: brand::INK,
            fg_2: brand::INK_2,
            fg_3: brand::INK_3,
            fg_disabled: rgba(0, 0, 0, 0.38),
            focus: brand::INK,

            layer: rgba(255, 255, 255, 0.34),
            card: brand::SURFACE,
            inset: rgba(0, 0, 0, 0.045),
            flyout: hex(0xffffff),
            line: rgba(0, 0, 0, 0.1),
            line_strong: rgba(0, 0, 0, 0.2),

            ctl: hex(0xffffff),
            ctl_hover: hex(0xf6f7f9),
            ctl_press: hex(0xeceef1),
            ctl_disabled: rgba(255, 255, 255, 0.5),
            ctl_line: rgba(0, 0, 0, 0.1),
            ctl_line_top: rgba(0, 0, 0, 0.1),
            ctl_line_bottom: rgba(0, 0, 0, 0.1),
            ctl_wall: brand::WALL,
            input_line: rgba(0, 0, 0, 0.45),
            input_focus: hex(0xffffff),
            subtle_hover: rgba(0, 0, 0, 0.06),
            subtle_press: rgba(0, 0, 0, 0.1),
            switch_off: rgba(0, 0, 0, 0.2),
            switch_line: rgba(0, 0, 0, 0.5),
            switch_knob: hex(0xffffff),
            smoke: rgba(20, 30, 50, 0.35),
            shadow_card: vec![
                BoxShadow::ring(1.0, rgba(0, 0, 0, 0.07)),
                BoxShadow::drop(0.4 * 16.0, 0.9 * 16.0, -0.55 * 16.0, rgba(20, 30, 50, 0.3)),
            ],
            shadow_flyout: vec![
                BoxShadow::ring(1.0, rgba(0, 0, 0, 0.07)),
                BoxShadow::drop(0.6 * 16.0, 1.4 * 16.0, -0.4 * 16.0, rgba(20, 30, 50, 0.35)),
            ],
            shadow_dialog: vec![
                BoxShadow::ring(1.0, rgba(0, 0, 0, 0.07)),
                BoxShadow::drop(1.9 * 16.0, 3.75 * 16.0, -1.25 * 16.0, rgba(20, 30, 50, 0.45)),
                BoxShadow::drop(0.5 * 16.0, 1.1 * 16.0, -0.5 * 16.0, rgba(20, 30, 50, 0.25)),
            ],

            danger: hex(0xb3261a),
            danger_bg: hex(0xfde4e1),
            warn: hex(0x7a4a00),
            warn_bg: hex(0xfdeccb),
            ok: hex(0x0c6b32),
            ok_bg: hex(0xd9f6e4),

            accent_fill: brand::INK,
            on_accent,
            accent_text: brand::INK,
            accent_hover: hex(0x25282e),
            accent_press: hex(0x3b3f45),
            accent_soft: rgba(0, 0, 0, 0.08),
            key_ink_top,
            key_ink_wall: hex(0x000000),
            key_ink_hover: mix_oklab(key_ink_top, on_accent, 0.88),

            hc_border: None,
            hc_outline: None,
        }
    }

    /// The site is light only; dark follows its black app windows.
    pub fn dark() -> Self {
        let key_ink_top = hex(0xffffff);
        let on_accent = hex(0x000000);
        Self {
            dark: true,
            mica: hex(0x0e0f11),
            fg: hex(0xffffff),
            fg_2: rgba(255, 255, 255, 0.8),
            fg_3: rgba(255, 255, 255, 0.62),
            fg_disabled: rgba(255, 255, 255, 0.36),
            focus: hex(0xffffff),

            layer: rgba(255, 255, 255, 0.025),
            card: hex(0x1b1d21),
            inset: rgba(0, 0, 0, 0.3),
            flyout: hex(0x25282e),
            line: rgba(255, 255, 255, 0.1),
            line_strong: rgba(255, 255, 255, 0.2),

            ctl: hex(0x2b2e34),
            ctl_hover: hex(0x33363d),
            ctl_press: hex(0x25282e),
            ctl_disabled: rgba(255, 255, 255, 0.05),
            ctl_line: rgba(255, 255, 255, 0.1),
            ctl_line_top: rgba(255, 255, 255, 0.14),
            ctl_line_bottom: rgba(255, 255, 255, 0.1),
            ctl_wall: hex(0x000000),
            input_line: rgba(255, 255, 255, 0.5),
            input_focus: hex(0x0e0f11),
            subtle_hover: rgba(255, 255, 255, 0.08),
            subtle_press: rgba(255, 255, 255, 0.05),
            switch_off: rgba(255, 255, 255, 0.22),
            switch_line: rgba(255, 255, 255, 0.6),
            switch_knob: hex(0xffffff),
            smoke: rgba(0, 0, 0, 0.55),
            shadow_card: vec![BoxShadow::ring(1.0, rgba(255, 255, 255, 0.08))],
            shadow_flyout: vec![
                BoxShadow::ring(1.0, rgba(255, 255, 255, 0.1)),
                BoxShadow::drop(0.6 * 16.0, 1.4 * 16.0, -0.4 * 16.0, rgba(0, 0, 0, 0.6)),
            ],
            shadow_dialog: vec![
                BoxShadow::ring(1.0, rgba(255, 255, 255, 0.1)),
                BoxShadow::drop(1.9 * 16.0, 3.75 * 16.0, -1.25 * 16.0, rgba(0, 0, 0, 0.7)),
            ],

            danger: hex(0xff9d94),
            danger_bg: hex(0x44211e),
            warn: hex(0xffd166),
            warn_bg: hex(0x40300f),
            ok: hex(0x37d67a),
            ok_bg: hex(0x153524),

            accent_fill: hex(0xffffff),
            on_accent,
            accent_text: hex(0xffffff),
            accent_hover: hex(0xe9ecf0),
            accent_press: hex(0xc9ccd2),
            accent_soft: rgba(255, 255, 255, 0.12),
            key_ink_top,
            key_ink_wall: brand::WALL,
            key_ink_hover: mix_oklab(key_ink_top, on_accent, 0.88),
            ..Self::light()
        }
    }

    /// Windows high contrast: solid system colours and visible borders everywhere.
    fn apply_high_contrast(&mut self, sys: &SystemColors) {
        let canvas = hex(sys.canvas);
        let canvas_text = hex(sys.canvas_text);
        let button_face = hex(sys.button_face);
        let button_text = hex(sys.button_text);
        let highlight = hex(sys.highlight);
        let highlight_text = hex(sys.highlight_text);
        let link_text = hex(sys.link_text);
        let gray_text = hex(sys.gray_text);
        self.high_contrast = true;
        self.mica = canvas;
        self.fg = canvas_text;
        self.fg_2 = canvas_text;
        self.fg_3 = canvas_text;
        self.fg_disabled = gray_text;
        self.focus = highlight;
        self.layer = canvas;
        self.card = canvas;
        self.inset = canvas;
        self.flyout = canvas;
        self.line = canvas_text;
        self.line_strong = canvas_text;
        self.ctl = button_face;
        self.ctl_hover = button_face;
        self.ctl_press = button_face;
        self.ctl_disabled = canvas;
        self.ctl_line = button_text;
        self.ctl_line_top = button_text;
        self.ctl_line_bottom = button_text;
        self.input_line = canvas_text;
        self.input_focus = canvas;
        self.subtle_hover = Color32::TRANSPARENT;
        self.subtle_press = Color32::TRANSPARENT;
        self.ctl_wall = Color32::TRANSPARENT;
        self.switch_off = canvas;
        self.switch_line = canvas_text;
        self.switch_knob = canvas_text;
        self.smoke = rgba(0, 0, 0, 0.6);
        self.shadow_card = Vec::new();
        self.key_ink_top = highlight;
        self.key_ink_wall = Color32::TRANSPARENT;
        self.voice = highlight;
        self.shadow_flyout = vec![BoxShadow::ring(1.0, canvas_text)];
        self.shadow_dialog = vec![BoxShadow::ring(2.0, canvas_text)];
        self.danger = canvas_text;
        self.danger_bg = canvas;
        self.warn = canvas_text;
        self.warn_bg = canvas;
        self.ok = canvas_text;
        self.ok_bg = canvas;
        self.accent_fill = highlight;
        self.on_accent = highlight_text;
        self.accent_text = link_text;
        self.accent_hover = highlight;
        self.accent_press = highlight;
        self.accent_soft = Color32::TRANSPARENT;
        self.key_ink_hover = mix_oklab(highlight, highlight_text, 0.88);
        self.hc_border = Some(canvas_text);
        self.hc_outline = Some(highlight);
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn to_oklab(c: Color32) -> [f32; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    let (r, g, b) =
        (srgb_to_linear(r as f32 / 255.0), srgb_to_linear(g as f32 / 255.0), srgb_to_linear(b as f32 / 255.0));
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> Color32 {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    let to_u8 = |c: f32| (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Color32::from_rgb(to_u8(r), to_u8(g), to_u8(b))
}

/// `color-mix(in oklab, a <weight>, b)` for opaque colours.
pub fn mix_oklab(a: Color32, b: Color32, weight_a: f32) -> Color32 {
    let (x, y) = (to_oklab(a), to_oklab(b));
    from_oklab([0, 1, 2].map(|i| x[i] * weight_a + y[i] * (1.0 - weight_a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_colours_match_css() {
        assert_eq!(rgba(0, 0, 0, 0.1).to_srgba_unmultiplied(), [0, 0, 0, 26]);
        assert_eq!(hex(0x1b1d21).to_srgba_unmultiplied(), [0x1b, 0x1d, 0x21, 255]);
    }

    #[test]
    fn oklab_mixes_round_trip() {
        assert_eq!(mix_oklab(hex(0x25282e), hex(0x25282e), 0.5), hex(0x25282e));
        let hover = mix_oklab(hex(0xffffff), hex(0x000000), 0.88);
        let [r, g, b, _] = hover.to_srgba_unmultiplied();
        assert!(r == g && g == b && r > 200 && r < 230, "{hover:?}");
    }
}
