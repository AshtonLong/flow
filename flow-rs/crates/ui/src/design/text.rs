//! Text styles and the helpers that lay text out the way the CSS does: an
//! explicit line height with the glyphs centred in it, letter spacing in em,
//! and Recursive's weights and axes picked by family.
//!
//! The Tailwind scale (`styles.css` `@theme`): caption 12/16, body 14/20,
//! subtitle 20/28, title 28/36. Body text is weight 440.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align, Color32, FontId, Galley, Response, Sense, Stroke, Ui, Vec2};

use super::fonts::{Weight, family, logo_family};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub line_height: f32,
    pub weight: Weight,
    /// Recursive's `MONO 1` (code, key caps).
    pub mono: bool,
    /// Letter spacing in em (`h1`, `h2`: -0.03em).
    pub tracking: f32,
    /// The logo face (`CASL 1`, `slnt -12`, `CRSV 1`, weight 880).
    pub logo: bool,
}

impl TextStyle {
    /// `text-caption`: 12/16.
    pub const CAPTION: Self = Self::new(12.0, 16.0);
    /// `text-body`: 14/20, the default.
    pub const BODY: Self = Self::new(14.0, 20.0);
    /// `text-subtitle`: 20/28.
    pub const SUBTITLE: Self = Self::new(20.0, 28.0);
    /// `text-title`: 28/36.
    pub const TITLE: Self = Self::new(28.0, 36.0);
    /// `h1`/`h2` letter spacing.
    pub const HEADING_TRACKING: f32 = -0.03;

    pub const fn new(size: f32, line_height: f32) -> Self {
        Self { size, line_height, weight: Weight::Body, mono: false, tracking: 0.0, logo: false }
    }

    pub const fn weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }

    pub const fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// `text-[13px]` and friends: a size with its own line height.
    pub const fn size(mut self, size: f32, line_height: f32) -> Self {
        self.size = size;
        self.line_height = line_height;
        self
    }

    pub const fn tracking(mut self, em: f32) -> Self {
        self.tracking = em;
        self
    }

    /// A heading (`h1`, `h2`): letter spacing -0.03em.
    pub const fn heading(self) -> Self {
        self.tracking(Self::HEADING_TRACKING)
    }

    pub const fn logo(mut self) -> Self {
        self.logo = true;
        self
    }

    pub fn font_id(&self) -> FontId {
        let family = if self.logo { logo_family() } else { family(self.weight, self.mono) };
        FontId::new(self.size, family)
    }

    pub fn format(&self, color: Color32) -> TextFormat {
        TextFormat {
            font_id: self.font_id(),
            extra_letter_spacing: self.tracking * self.size,
            line_height: Some(self.line_height),
            color,
            valign: Align::Center,
            ..TextFormat::default()
        }
    }
}

/// Recursive's typographic ascent and descent (`USE_TYPO_METRICS` is set), per em.
const ASCENT: f32 = 0.95;
const DESCENT: f32 = 0.25;

/// Where Chromium puts the baseline in a line box: it rounds the ascent and
/// descent to whole pixels, centres them in the line height, and snaps the
/// result down to a whole pixel.
pub fn css_baseline(size: f32, line_height: f32) -> f32 {
    let ascent = (ASCENT * size).round();
    let descent = (DESCENT * size).round();
    ((line_height - ascent - descent) / 2.0 + ascent - 0.01).round()
}

/// Moves each row so its baseline sits where Chromium would draw it, on a
/// whole pixel. egui centres the unrounded font height in the line, which
/// leaves 14 px body text 0.9 px low and blurred across an extra pixel row.
pub fn snap_baselines(galley: Arc<Galley>, style: TextStyle) -> Arc<Galley> {
    let target = css_baseline(style.size, style.line_height);
    let off = |row: &egui::epaint::text::PlacedRow| row.row.glyphs.first().map(|g| target - g.pos.y).unwrap_or(0.0);
    if galley.rows.iter().all(|row| off(row).abs() < 0.01) {
        return galley;
    }
    let mut snapped = (*galley).clone();
    for row in &mut snapped.rows {
        row.pos.y += off(row);
    }
    Arc::new(snapped)
}

/// Lays out one run of text. `wrap` is the maximum width (`f32::INFINITY` for one line).
pub fn galley(ui: &Ui, text: impl Into<String>, style: TextStyle, color: Color32, wrap: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.into(), style.format(color));
    job.wrap = TextWrapping { max_width: css_wrap_width(wrap), ..TextWrapping::default() };
    snap_baselines(ui.painter().layout_job(job), style)
}

/// egui lets a line run 0.5 px past its wrap width (`LayoutJob::effective_wrap_width`);
/// Chromium breaks as soon as the text is wider than its box. Cancel the slack.
pub fn css_wrap_width(width: f32) -> f32 {
    if width.is_finite() { (width - 0.5).max(0.0) } else { width }
}

/// One line, cut with an ellipsis to fit `max_width` (`truncate`).
pub fn galley_truncated(
    ui: &Ui,
    text: impl Into<String>,
    style: TextStyle,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.into(), style.format(color));
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    snap_baselines(ui.painter().layout_job(job), style)
}

/// Text that does not wrap, allocated at its size. Not interactive.
pub fn label(ui: &mut Ui, text: impl Into<String>, style: TextStyle, color: Color32) -> Response {
    let galley = galley(ui, text, style, color, f32::INFINITY);
    paint_galley(ui, galley)
}

/// Text wrapped at the available width.
pub fn paragraph(ui: &mut Ui, text: impl Into<String>, style: TextStyle, color: Color32) -> Response {
    let width = ui.available_width();
    let galley = galley(ui, text, style, color, width);
    paint_galley(ui, galley)
}

/// Text wrapped at `max_width` (`max-w-[72ch]` and the like).
pub fn paragraph_max(
    ui: &mut Ui,
    text: impl Into<String>,
    style: TextStyle,
    color: Color32,
    max_width: f32,
) -> Response {
    let width = ui.available_width().min(max_width);
    let galley = galley(ui, text, style, color, width);
    paint_galley(ui, galley)
}

/// Allocates a laid-out galley at the cursor and paints it.
pub fn paint_galley(ui: &mut Ui, galley: Arc<Galley>) -> Response {
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, Color32::PLACEHOLDER);
    response
}

/// Width of a run of text in a style.
pub fn width(ui: &Ui, text: &str, style: TextStyle) -> f32 {
    galley(ui, text, style, Color32::PLACEHOLDER, f32::INFINITY).size().x
}

/// `ch` units: the advance of "0" in a style (`max-w-[72ch]`).
pub fn ch(ui: &Ui, style: TextStyle) -> f32 {
    width(ui, "0", style)
}

/// One styled span of [`Rich`] text.
#[derive(Debug, Clone)]
pub struct Span {
    pub text: String,
    pub style: TextStyle,
    pub color: Option<Color32>,
    pub underline: bool,
    /// Clickable spans report their index from [`Rich::show`].
    pub link: bool,
}

/// Mixed inline text: plain words, `code`, emphasis and links in one wrapped
/// paragraph. The stand-in for JSX such as `Open it in <b>Models</b>`.
#[derive(Debug, Clone)]
pub struct Rich {
    base: TextStyle,
    color: Color32,
    spans: Vec<Span>,
}

/// What a [`Rich`] paragraph reported.
pub struct RichResponse {
    pub response: Response,
    /// Index (in the order added) of the link span that was clicked.
    pub clicked: Option<usize>,
}

impl Rich {
    pub fn new(base: TextStyle, color: Color32) -> Self {
        Self { base, color, spans: Vec::new() }
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.spans.push(Span { text: text.into(), style: self.base, color: None, underline: false, link: false });
        self
    }

    pub fn styled(mut self, text: impl Into<String>, style: TextStyle, color: Option<Color32>) -> Self {
        self.spans.push(Span { text: text.into(), style, color, underline: false, link: false });
        self
    }

    /// `<code>`: the mono face at the same size.
    pub fn code(self, text: impl Into<String>) -> Self {
        let style = self.base.mono();
        self.styled(text, style, None)
    }

    /// `font-semibold` inline.
    pub fn strong(self, text: impl Into<String>) -> Self {
        let style = self.base.weight(Weight::Strong);
        self.styled(text, style, None)
    }

    /// `.link`: weight 620, underlined, clickable.
    pub fn link(mut self, text: impl Into<String>) -> Self {
        let style = self.base.weight(Weight::Strong);
        self.spans.push(Span { text: text.into(), style, color: None, underline: true, link: true });
        self
    }

    pub fn job(&self, wrap: f32, hovered_link: Option<usize>) -> LayoutJob {
        let mut job = LayoutJob::default();
        let mut link_index = 0;
        for span in &self.spans {
            let mut format = span.style.format(span.color.unwrap_or(self.color));
            if span.underline {
                let hovered = span.link && hovered_link == Some(link_index);
                let thickness = span.style.size * if hovered { 0.16 } else { 0.09 };
                format.underline = Stroke::new(thickness.max(1.0), format.color);
            }
            if span.link {
                link_index += 1;
            }
            job.append(&span.text, 0.0, format);
        }
        job.wrap = TextWrapping { max_width: wrap, ..TextWrapping::default() };
        job
    }

    /// Wraps at the available width and reports clicks on links.
    pub fn show(self, ui: &mut Ui) -> RichResponse {
        let wrap = ui.available_width();
        let id = ui.next_auto_id();
        let hovered: Option<usize> = ui.data(|d| d.get_temp(id));
        let galley = snap_baselines(ui.painter().layout_job(self.job(wrap, hovered)), self.base);
        let has_links = self.spans.iter().any(|s| s.link);
        let sense = if has_links { Sense::click() } else { Sense::hover() };
        let (rect, response) = ui.allocate_exact_size(galley.size(), sense);
        let mut clicked = None;
        let mut now_hovered = None;
        if has_links && let Some(pointer) = response.hover_pos() {
            let cursor = galley.cursor_from_pos(pointer - rect.min);
            // Map the character index back to the span it falls in.
            let mut offset = 0usize;
            let mut link_index = 0;
            for span in &self.spans {
                let len = span.text.chars().count();
                if span.link {
                    if cursor.index.0 >= offset && cursor.index.0 < offset + len {
                        now_hovered = Some(link_index);
                    }
                    link_index += 1;
                }
                offset += len;
            }
            if now_hovered.is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    clicked = now_hovered;
                }
            }
        }
        if now_hovered != hovered {
            ui.data_mut(|d| d.insert_temp(id, now_hovered));
            ui.ctx().request_repaint();
        }
        ui.painter().galley(rect.min, galley, self.color);
        RichResponse { response, clicked }
    }
}

/// The first line's baseline, from the galley's top.
pub fn baseline(galley: &Galley) -> f32 {
    galley
        .rows
        .first()
        .map(|row| row.pos.y + row.row.glyphs.first().map(|g| g.pos.y).unwrap_or(row.row.size.y * 0.75))
        .unwrap_or(0.0)
}

/// The size of one line box in a style.
pub fn line_size(width: f32, style: TextStyle) -> Vec2 {
    Vec2::new(width, style.line_height)
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    /// A line that fits, but only without its trailing space, stays whole
    /// (vendor/epaint); one a little too wide still wraps.
    #[test]
    fn wraps_where_chromium_wraps() {
        let mut h = crate::testing::Harness::new();
        h.frame(|ui| {
            let line = "Fix words Flow mishears: names, product terms, acronyms. The right spellings are also given";
            let fits = width(ui, line, TextStyle::CAPTION);
            let rows = |wrap: f32| {
                let galley =
                    galley(ui, format!("{line} to the speech model."), TextStyle::CAPTION, Color32::WHITE, wrap);
                galley.rows.first().map(|r| r.row.text().trim_end().to_string()).unwrap_or_default()
            };
            assert_eq!(rows(fits + 1.0), line);
            assert_eq!(rows(fits), line);
            assert!(rows(fits - 2.0).ends_with("also"));
        });
    }
}
