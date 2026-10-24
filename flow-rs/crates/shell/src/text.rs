//! The pill's text: Recursive at weight 600 (`brand.css`: MONO 0, CASL 0, slnt
//! 0, CRSV 0.5, which are the font's defaults), shaped as Chromium shapes it
//! (HarfBuzz rules, kerning on) and filled from its outlines.

use std::cell::RefCell;
use std::collections::HashMap;

use harfrust::{Feature, FontRef, ShaperData, ShaperInstance, Tag, UnicodeBuffer, Variation};
use tiny_skia::{Path, PathBuilder};

/// Recursive (variable), latin subset. Kept in the binary: pages are touched
/// only for the glyphs drawn.
pub static FONT_DATA: &[u8] = include_bytes!("../../../resources/fonts/Recursive-latin.ttf");

/// The weight the pill uses (`#pill { font-weight: 600 }`).
pub const PILL_WEIGHT: f32 = 600.0;

/// A glyph placed on the line, in font units from the line start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    pub cluster: u32,
}

/// A shaped line of text, in font units.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShapedText {
    pub glyphs: Vec<PlacedGlyph>,
    pub advance: f32,
}

impl ShapedText {
    /// Width in pixels at `size` px.
    pub fn width(&self, font: &Font, size: f32) -> f32 {
        self.advance * font.scale(size)
    }
}

pub struct Font {
    face: ttf_parser::Face<'static>,
    font_ref: FontRef<'static>,
    shaper_data: ShaperData,
    instance: ShaperInstance,
    units_per_em: f32,
    ascender: f32,
    descender: f32,
    /// Outlines in font units, y up.
    outlines: RefCell<HashMap<u16, Option<Path>>>,
}

impl Font {
    /// Loads Recursive at `weight`. `None` only if the embedded font is broken.
    pub fn load(weight: f32) -> Option<Self> {
        let mut face = ttf_parser::Face::parse(FONT_DATA, 0).ok()?;
        face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), weight)?;
        let font_ref = FontRef::new(FONT_DATA).ok()?;
        let shaper_data = ShaperData::new(&font_ref);
        let instance = ShaperInstance::from_variations(&font_ref, [Variation { tag: Tag::new(b"wght"), value: weight }]);
        let units_per_em = face.units_per_em() as f32;
        // USE_TYPO_METRICS is set: Chromium takes the typo ascender and descender.
        let ascender = face.typographic_ascender().unwrap_or(face.ascender()) as f32;
        let descender = -(face.typographic_descender().unwrap_or(face.descender()) as f32);
        Some(Self {
            face,
            font_ref,
            shaper_data,
            instance,
            units_per_em,
            ascender,
            descender,
            outlines: RefCell::new(HashMap::new()),
        })
    }

    /// Pixels per font unit at `size` px.
    pub fn scale(&self, size: f32) -> f32 {
        size / self.units_per_em
    }

    /// Chromium's rounded ascent and descent at `size` px.
    pub fn rounded_metrics(&self, size: f32) -> (f32, f32) {
        let s = self.scale(size);
        ((self.ascender * s).round(), (self.descender * s).round())
    }

    /// Where the baseline sits below the top of a line box of `line_height` px
    /// (LayoutNG floors the half-leading above the text).
    pub fn baseline_offset(&self, size: f32, line_height: f32) -> f32 {
        let (ascent, descent) = self.rounded_metrics(size);
        ascent + ((line_height - (ascent + descent)) / 2.0).floor()
    }

    /// Shapes one line, left to right.
    pub fn shape(&self, text: &str, features: &[Feature]) -> ShapedText {
        let shaper = self.shaper_data.shaper(&self.font_ref).instance(Some(&self.instance)).build();
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let glyphs = shaper.shape(buffer, harfrust::ShapeOptions::new().features(features));
        let mut pen = 0.0f32;
        let mut out = Vec::with_capacity(glyphs.len());
        for (info, pos) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            out.push(PlacedGlyph {
                id: info.glyph_id as u16,
                x: pen + pos.x_offset as f32,
                y: pos.y_offset as f32,
                advance: pos.x_advance as f32,
                cluster: info.cluster,
            });
            pen += pos.x_advance as f32;
        }
        ShapedText { glyphs: out, advance: pen }
    }

    /// `text-overflow: ellipsis`: the text cut at a glyph boundary so that it and
    /// "…" fit in `max_width` px.
    pub fn ellipsize(&self, text: &ShapedText, size: f32, max_width: f32) -> ShapedText {
        if text.width(self, size) <= max_width + 0.01 {
            return text.clone();
        }
        let ellipsis = self.shape("\u{2026}", &[]);
        let room = max_width / self.scale(size) - ellipsis.advance;
        let mut out = ShapedText::default();
        for g in &text.glyphs {
            if g.x + g.advance > room {
                break;
            }
            out.glyphs.push(*g);
            out.advance = g.x + g.advance;
        }
        let start = out.advance;
        out.glyphs.extend(ellipsis.glyphs.iter().map(|g| PlacedGlyph { x: g.x + start, ..*g }));
        out.advance = start + ellipsis.advance;
        out
    }

    /// The glyph's outline in font units (y up), cached.
    pub fn outline(&self, id: u16) -> Option<Path> {
        if let Some(path) = self.outlines.borrow().get(&id) {
            return path.clone();
        }
        let mut builder = Outline(PathBuilder::new());
        let path = self.face.outline_glyph(ttf_parser::GlyphId(id), &mut builder).and_then(|_| builder.0.finish());
        self.outlines.borrow_mut().insert(id, path.clone());
        path
    }
}

/// How Chromium's text looks on Windows: DirectWrite renders glyph coverage
/// with horizontal subpixel precision and a low-pass filter, then boosts
/// partial coverage ("enhanced contrast"). Plain area coverage looks thin and
/// grey next to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextLook {
    /// Weight of each horizontal neighbour in a [s, 1 - 2s, s] filter.
    pub spread: f32,
    /// k in `c (k + 1) / (c k + 1)`.
    pub contrast: f32,
}

impl Default for TextLook {
    /// Fitted against the Electron screenshots (label pixels, 100 % scale).
    fn default() -> Self {
        Self { spread: 0.07, contrast: 2.0 }
    }
}

/// Supersampling per axis for glyph coverage (tiny-skia's own anti-aliasing has
/// 16 levels; 4 × 4 samples of it give smooth edges).
const SUPERSAMPLE: u32 = 4;

/// A line of glyphs as an alpha mask positioned at (`x`, `y`) in the target.
pub struct LineMask {
    pub mask: tiny_skia::Mask,
    pub x: i32,
    pub y: i32,
}

impl Font {
    /// Rasterises `text` at `size` px with its pen at `x` and baseline at
    /// `baseline` (device pixels), cut off at `clip_right`.
    pub fn rasterize(
        &self,
        text: &ShapedText,
        size: f32,
        x: f32,
        baseline: f32,
        clip_right: f32,
        look: TextLook,
    ) -> Option<LineMask> {
        let scale = self.scale(size);
        let x0 = x.floor() as i32 - 2;
        let right = (x + text.advance * scale).min(clip_right).ceil() as i32 + 2;
        let y0 = (baseline - self.ascender * scale * 1.3).floor() as i32 - 1;
        let y1 = (baseline + self.descender * scale * 1.5).ceil() as i32 + 1;
        let (w, h) = ((right - x0).max(1) as u32, (y1 - y0).max(1) as u32);
        let ss = SUPERSAMPLE;
        let mut hi = tiny_skia::Mask::new(w * ss, h * ss)?;
        let ssf = ss as f32;
        for g in &text.glyphs {
            let gx = x + g.x * scale;
            if gx >= clip_right {
                break;
            }
            let Some(path) = self.outline(g.id) else { continue };
            let ts = tiny_skia::Transform::from_row(
                scale * ssf,
                0.0,
                0.0,
                -scale * ssf,
                (gx - x0 as f32) * ssf,
                (baseline - g.y * scale - y0 as f32) * ssf,
            );
            hi.fill_path(&path, tiny_skia::FillRule::Winding, true, ts);
        }
        // Box-downsample to pixel coverage.
        let mut cov = vec![0.0f32; (w * h) as usize];
        let src = hi.data();
        let hw = (w * ss) as usize;
        for py in 0..h as usize {
            for px in 0..w as usize {
                let mut sum = 0u32;
                for sy in 0..ss as usize {
                    let row = (py * ss as usize + sy) * hw + px * ss as usize;
                    sum += src[row..row + ss as usize].iter().map(|&v| v as u32).sum::<u32>();
                }
                cov[py * w as usize + px] = sum as f32 / (255 * ss * ss) as f32;
            }
        }
        // Horizontal low-pass, then enhanced contrast.
        let s = look.spread;
        let k = look.contrast;
        let mut mask = tiny_skia::Mask::new(w, h)?;
        let out = mask.data_mut();
        for py in 0..h as usize {
            let row = &cov[py * w as usize..(py + 1) * w as usize];
            for px in 0..w as usize {
                let left = if px > 0 { row[px - 1] } else { 0.0 };
                let right = if px + 1 < w as usize { row[px + 1] } else { 0.0 };
                let c = (s * left + (1.0 - 2.0 * s) * row[px] + s * right).clamp(0.0, 1.0);
                let c = c * (k + 1.0) / (c * k + 1.0);
                out[py * w as usize + px] = (c * 255.0).round() as u8;
            }
        }
        // Nothing may spill past the clip (the label's overflow: hidden).
        let clip = (clip_right.ceil() as i32 - x0).clamp(0, w as i32) as usize;
        if clip < w as usize {
            for py in 0..h as usize {
                out[py * w as usize + clip..(py + 1) * w as usize].fill(0);
            }
        }
        Some(LineMask { mask, x: x0, y: y0 })
    }
}

struct Outline(PathBuilder);

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// `font-variant-numeric: tabular-nums`
pub fn tabular_nums() -> Feature {
    Feature::new(Tag::new(b"tnum"), 1, ..)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_shapes_recursive_semibold() {
        let font = Font::load(PILL_WEIGHT).unwrap();
        let typed = font.shape("Typed", &[]);
        assert_eq!(typed.glyphs.len(), 5);
        let w = typed.width(&font, 14.0);
        // "Typed" in the Electron screenshot measures about 41 px at 14 px.
        assert!((w - 41.0).abs() < 2.0, "width {w}");
        // Recursive keeps its widths across weights, so a weight change never reflows.
        let light = Font::load(300.0).unwrap();
        assert_eq!(light.shape("Typed", &[]).advance, typed.advance);
    }

    #[test]
    fn baseline_follows_chromium_rounding() {
        let font = Font::load(PILL_WEIGHT).unwrap();
        // 14 px, line-height 1: ascent 13, descent 4, half-leading floor(-1.5) = -2.
        assert_eq!(font.rounded_metrics(14.0), (13.0, 4.0));
        assert_eq!(font.baseline_offset(14.0, 14.0), 11.0);
    }

    #[test]
    fn ellipsis_fits_the_room() {
        let font = Font::load(PILL_WEIGHT).unwrap();
        let long = font.shape("No text field focused — copied to clipboard", &[]);
        let cut = font.ellipsize(&long, 14.0, 120.0);
        assert!(cut.width(&font, 14.0) <= 120.0);
        assert!(cut.glyphs.len() < long.glyphs.len());
        let short = font.shape("Typed", &[]);
        assert_eq!(font.ellipsize(&short, 14.0, 200.0), short);
    }

    #[test]
    fn every_glyph_of_the_labels_has_an_outline() {
        let font = Font::load(PILL_WEIGHT).unwrap();
        for word in ["Listening", "Transcribing", "Typed", "+3", "…", "—"] {
            for g in font.shape(word, &[]).glyphs {
                assert!(font.outline(g.id).is_some(), "{word}: glyph {}", g.id);
            }
        }
        assert!(font.outline(font.shape(" ", &[]).glyphs[0].id).is_none());
    }
}
