//! Lucide icons (`flow-rs/resources/icons/lucide/`). Each SVG is parsed once
//! and flattened into polylines in its 24×24 view box (`<path>` with arcs and
//! curves, `<circle>`, `<rect>`, `<line>`, `<polyline>`). Drawing strokes the
//! polylines with round caps and joins, as lucide's `stroke-linecap="round"
//! stroke-linejoin="round"` asks: each glyph is rendered from its distance
//! field into a small texture at the exact device size, so joins never
//! double-blend and thin strokes stay crisp. Textures are cached per
//! (icon, size, stroke width, pixels per point).
//!
//! Sizes follow lucide-react: `size` in points and `strokeWidth` in view-box
//! units, so the drawn stroke is `stroke_width * size / 24`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use egui::emath::GuiRounding as _;
use egui::epaint::{Mesh, Shape};
use egui::{
    Color32, ColorImage, Context, Pos2, Rect, Response, Sense, TextureHandle, TextureOptions, Ui,
    Vec2, pos2, vec2,
};
use parking_lot::Mutex;

/// Every icon the TSX imports from `lucide-react`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    AppWindow,
    ArrowRight,
    Boxes,
    Check,
    ChevronDown,
    CircleAlert,
    CircleCheck,
    Copy,
    Download,
    History,
    Info,
    Keyboard,
    LoaderCircle,
    Mic,
    Pencil,
    Plus,
    RefreshCw,
    RotateCcw,
    Search,
    Settings2,
    SpellCheck,
    Square,
    Trash2,
    TriangleAlert,
    Wrench,
    X,
}

macro_rules! svg {
    ($file:literal) => {
        include_str!(concat!(
            "../../../../resources/icons/lucide/",
            $file,
            ".svg"
        ))
    };
}

impl Icon {
    pub const ALL: [Self; 26] = [
        Self::AppWindow,
        Self::ArrowRight,
        Self::Boxes,
        Self::Check,
        Self::ChevronDown,
        Self::CircleAlert,
        Self::CircleCheck,
        Self::Copy,
        Self::Download,
        Self::History,
        Self::Info,
        Self::Keyboard,
        Self::LoaderCircle,
        Self::Mic,
        Self::Pencil,
        Self::Plus,
        Self::RefreshCw,
        Self::RotateCcw,
        Self::Search,
        Self::Settings2,
        Self::SpellCheck,
        Self::Square,
        Self::Trash2,
        Self::TriangleAlert,
        Self::Wrench,
        Self::X,
    ];

    fn source(self) -> &'static str {
        match self {
            Self::AppWindow => svg!("app-window"),
            Self::ArrowRight => svg!("arrow-right"),
            Self::Boxes => svg!("boxes"),
            Self::Check => svg!("check"),
            Self::ChevronDown => svg!("chevron-down"),
            Self::CircleAlert => svg!("circle-alert"),
            Self::CircleCheck => svg!("circle-check"),
            Self::Copy => svg!("copy"),
            Self::Download => svg!("download"),
            Self::History => svg!("history"),
            Self::Info => svg!("info"),
            Self::Keyboard => svg!("keyboard"),
            Self::LoaderCircle => svg!("loader-circle"),
            Self::Mic => svg!("mic"),
            Self::Pencil => svg!("pencil"),
            Self::Plus => svg!("plus"),
            Self::RefreshCw => svg!("refresh-cw"),
            Self::RotateCcw => svg!("rotate-ccw"),
            Self::Search => svg!("search"),
            Self::Settings2 => svg!("settings-2"),
            Self::SpellCheck => svg!("spell-check"),
            Self::Square => svg!("square"),
            Self::Trash2 => svg!("trash-2"),
            Self::TriangleAlert => svg!("triangle-alert"),
            Self::Wrench => svg!("wrench"),
            Self::X => svg!("x"),
        }
    }

    /// The flattened polylines, in view-box units.
    pub fn polylines(self) -> &'static [Polyline] {
        static PARSED: OnceLock<HashMap<Icon, Vec<Polyline>>> = OnceLock::new();
        PARSED
            .get_or_init(|| {
                Icon::ALL
                    .iter()
                    .map(|&icon| (icon, parse_svg(icon.source())))
                    .collect()
            })
            .get(&self)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// One stroked run of points in the 24×24 view box.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Pos2>,
    pub closed: bool,
}

// ── Drawing ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    icon: Icon,
    px: u32,
    stroke_milli: u32,
}

type Cache = Arc<Mutex<HashMap<Key, TextureHandle>>>;

fn texture(ctx: &Context, icon: Icon, size: f32, stroke_width: f32) -> (TextureHandle, f32) {
    let ppp = ctx.pixels_per_point();
    let px = (size * ppp).round().max(1.0) as u32;
    let key = Key {
        icon,
        px,
        stroke_milli: (stroke_width * 1000.0).round() as u32,
    };
    let cache: Cache = ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Cache>(egui::Id::unique("flow-icon-cache"))
            .clone()
    });
    let mut cache = cache.lock();
    let handle = cache
        .entry(key)
        .or_insert_with(|| {
            let image = rasterize(icon.polylines(), px, stroke_width);
            ctx.load_texture(
                format!("icon-{icon:?}-{px}-{}", key.stroke_milli),
                image,
                TextureOptions::LINEAR,
            )
        })
        .clone();
    (handle, px as f32 / ppp)
}

/// Coverage of the stroked polylines, as white with alpha.
fn rasterize(lines: &[Polyline], px: u32, stroke_width: f32) -> ColorImage {
    let scale = px as f32 / 24.0;
    let half = stroke_width * scale / 2.0;
    let mut segments: Vec<(Pos2, Pos2)> = Vec::new();
    for line in lines {
        let pts: Vec<Pos2> = line
            .points
            .iter()
            .map(|p| pos2(p.x * scale, p.y * scale))
            .collect();
        for pair in pts.windows(2) {
            segments.push((pair[0], pair[1]));
        }
        if line.closed && pts.len() > 2 {
            segments.push((pts[pts.len() - 1], pts[0]));
        }
        if pts.len() == 1 {
            segments.push((pts[0], pts[0]));
        }
    }
    let n = px as usize;
    let mut pixels = vec![Color32::TRANSPARENT; n * n];
    // Supersample 3×3 with a narrow ramp: close to the analytic coverage browsers use.
    const SS: usize = 3;
    for y in 0..n {
        for x in 0..n {
            let mut cover = 0.0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let p = pos2(
                        x as f32 + (sx as f32 + 0.5) / SS as f32,
                        y as f32 + (sy as f32 + 0.5) / SS as f32,
                    );
                    let d = segments
                        .iter()
                        .map(|&(a, b)| distance_to_segment(p, a, b))
                        .fold(f32::INFINITY, f32::min);
                    cover += (half - d + 0.5 / SS as f32).clamp(0.0, 1.0 / SS as f32) * SS as f32;
                }
            }
            let alpha = (cover / (SS * SS) as f32).clamp(0.0, 1.0);
            pixels[y * n + x] = Color32::from_white_alpha((alpha * 255.0).round() as u8);
        }
    }
    ColorImage::new([n, n], pixels)
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// Paints an icon centred in `rect`. `stroke_width` is in view-box units (lucide's `strokeWidth`).
pub fn paint(ui: &Ui, icon: Icon, rect: Rect, size: f32, stroke_width: f32, color: Color32) {
    paint_rotated(ui, icon, rect, size, stroke_width, color, 0.0);
}

/// Like [`paint`], turned by `angle` radians about its centre (the `.spin` loader).
pub fn paint_rotated(
    ui: &Ui,
    icon: Icon,
    rect: Rect,
    size: f32,
    stroke_width: f32,
    color: Color32,
    angle: f32,
) {
    let (handle, drawn) = texture(ui.ctx(), icon, size, stroke_width);
    let ppp = ui.ctx().pixels_per_point();
    let min = (rect.center() - Vec2::splat(drawn / 2.0)).round_to_pixels(ppp);
    let target = Rect::from_min_size(min, Vec2::splat(drawn));
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    if angle == 0.0 {
        ui.painter().image(handle.id(), target, uv, color);
        return;
    }
    let mut mesh = Mesh::with_texture(handle.id());
    mesh.add_rect_with_uv(target, uv, color);
    let rot = egui::emath::Rot2::from_angle(angle);
    let center = target.center();
    for v in &mut mesh.vertices {
        v.pos = center + rot * (v.pos - center);
    }
    ui.painter().add(Shape::mesh(mesh));
}

/// Allocates `size`×`size` at the cursor and paints the icon there.
pub fn show(ui: &mut Ui, icon: Icon, size: f32, stroke_width: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint(ui, icon, rect, size, stroke_width, color);
    response
}

// ── Parsing ─────────────────────────────────────────────────────────────────

/// The value of `name="…"` inside one element's attribute text.
fn attr<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = element;
    while let Some(at) = rest.find(name) {
        let before_ok = at == 0 || rest.as_bytes()[at - 1].is_ascii_whitespace();
        let after = &rest[at + name.len()..];
        if before_ok && let Some(value) = after.trim_start().strip_prefix('=') {
            let value = value.trim_start().strip_prefix('"')?;
            return value.find('"').map(|end| &value[..end]);
        }
        rest = &rest[at + name.len()..];
    }
    None
}

fn num(element: &str, name: &str) -> f32 {
    attr(element, name)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0)
}

/// Parses the shapes of a lucide SVG into polylines.
pub fn parse_svg(source: &str) -> Vec<Polyline> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let element = &rest[..end];
        rest = &rest[end..];
        let tag = element
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        match tag {
            "path" => {
                if let Some(d) = attr(element, "d") {
                    out.extend(parse_path(d));
                }
            }
            "circle" => {
                let (cx, cy, r) = (num(element, "cx"), num(element, "cy"), num(element, "r"));
                out.push(ellipse(pos2(cx, cy), r, r));
            }
            "rect" => {
                let (x, y, w, h) = (
                    num(element, "x"),
                    num(element, "y"),
                    num(element, "width"),
                    num(element, "height"),
                );
                let rx = attr(element, "rx").map(|_| num(element, "rx"));
                let ry = attr(element, "ry").map(|_| num(element, "ry"));
                let (rx, ry) = match (rx, ry) {
                    (Some(rx), Some(ry)) => (rx, ry),
                    (Some(r), None) | (None, Some(r)) => (r, r),
                    (None, None) => (0.0, 0.0),
                };
                out.push(rounded_rect(x, y, w, h, rx.min(w / 2.0), ry.min(h / 2.0)));
            }
            "line" => out.push(Polyline {
                points: vec![
                    pos2(num(element, "x1"), num(element, "y1")),
                    pos2(num(element, "x2"), num(element, "y2")),
                ],
                closed: false,
            }),
            "polyline" | "polygon" => {
                let values: Vec<f32> = attr(element, "points")
                    .unwrap_or("")
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter_map(|v| v.parse().ok())
                    .collect();
                let points = values
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[x, y]| pos2(*x, *y))
                    .collect();
                out.push(Polyline {
                    points,
                    closed: tag == "polygon",
                });
            }
            _ => {}
        }
    }
    out
}

fn ellipse(center: Pos2, rx: f32, ry: f32) -> Polyline {
    let steps = 48;
    let points = (0..steps)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / steps as f32;
            pos2(center.x + rx * a.cos(), center.y + ry * a.sin())
        })
        .collect();
    Polyline {
        points,
        closed: true,
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32) -> Polyline {
    if rx <= 0.0 || ry <= 0.0 {
        return Polyline {
            points: vec![
                pos2(x, y),
                pos2(x + w, y),
                pos2(x + w, y + h),
                pos2(x, y + h),
            ],
            closed: true,
        };
    }
    let mut points = Vec::new();
    let corners = [
        (pos2(x + w - rx, y + ry), -std::f32::consts::FRAC_PI_2),
        (pos2(x + w - rx, y + h - ry), 0.0),
        (pos2(x + rx, y + h - ry), std::f32::consts::FRAC_PI_2),
        (pos2(x + rx, y + ry), std::f32::consts::PI),
    ];
    for (c, start) in corners {
        for i in 0..=8 {
            let a = start + std::f32::consts::FRAC_PI_2 * i as f32 / 8.0;
            points.push(pos2(c.x + rx * a.cos(), c.y + ry * a.sin()));
        }
    }
    Polyline {
        points,
        closed: true,
    }
}

/// Path data tokens: command letters and numbers (`-4.74-2.85`, `.97.5`).
struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn skip_separators(&mut self) {
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',')
        {
            self.i += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip_separators();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn has_number(&mut self) -> bool {
        self.skip_separators();
        self.s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_separators();
        let start = self.i;
        let s = self.s;
        if self.i < s.len() && matches!(s[self.i], b'-' | b'+') {
            self.i += 1;
        }
        let mut seen_dot = false;
        let mut seen_exp = false;
        while self.i < s.len() {
            let c = s[self.i];
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !seen_dot && !seen_exp {
                seen_dot = true;
                self.i += 1;
            } else if (c == b'e' || c == b'E') && !seen_exp {
                seen_exp = true;
                self.i += 1;
                if self.i < s.len() && matches!(s[self.i], b'-' | b'+') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
        std::str::from_utf8(&s[start..self.i]).ok()?.parse().ok()
    }

    /// Arc flags may be written without separators (`a2 2 0 011 1`).
    fn flag(&mut self) -> Option<bool> {
        self.skip_separators();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Parses SVG path data into polylines, flattening curves and arcs.
pub fn parse_path(d: &str) -> Vec<Polyline> {
    let mut lines: Vec<Polyline> = Vec::new();
    let mut current: Vec<Pos2> = Vec::new();
    let mut t = Tokens {
        s: d.as_bytes(),
        i: 0,
    };
    let mut pos = Pos2::ZERO;
    let mut start = Pos2::ZERO;
    let mut last_control: Option<Pos2> = None;
    let mut last_quad: Option<Pos2> = None;
    let mut cmd = b'M';

    let flush = |current: &mut Vec<Pos2>, lines: &mut Vec<Polyline>, closed: bool| {
        if current.len() > 1 {
            lines.push(Polyline {
                points: std::mem::take(current),
                closed,
            });
        }
        current.clear();
    };

    loop {
        if let Some(c) = t.command() {
            cmd = c;
        } else if !t.has_number() {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { pos.to_vec2() } else { Vec2::ZERO };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(x), Some(y)) = (t.number(), t.number()) else {
                    break;
                };
                flush(&mut current, &mut lines, false);
                pos = pos2(x, y) + base;
                start = pos;
                current.push(pos);
                // Further pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
                last_control = None;
                last_quad = None;
            }
            b'L' => {
                let (Some(x), Some(y)) = (t.number(), t.number()) else {
                    break;
                };
                pos = pos2(x, y) + base;
                current.push(pos);
                last_control = None;
                last_quad = None;
            }
            b'H' => {
                let Some(x) = t.number() else { break };
                pos = pos2(if rel { pos.x + x } else { x }, pos.y);
                current.push(pos);
                last_control = None;
                last_quad = None;
            }
            b'V' => {
                let Some(y) = t.number() else { break };
                pos = pos2(pos.x, if rel { pos.y + y } else { y });
                current.push(pos);
                last_control = None;
                last_quad = None;
            }
            b'C' | b'S' => {
                let c1 = if cmd.eq_ignore_ascii_case(&b'C') {
                    let (Some(x), Some(y)) = (t.number(), t.number()) else {
                        break;
                    };
                    pos2(x, y) + base
                } else {
                    last_control.map(|c| pos + (pos - c)).unwrap_or(pos)
                };
                let (Some(x2), Some(y2), Some(x), Some(y)) =
                    (t.number(), t.number(), t.number(), t.number())
                else {
                    break;
                };
                let c2 = pos2(x2, y2) + base;
                let end = pos2(x, y) + base;
                if current.is_empty() {
                    current.push(pos);
                }
                cubic(&mut current, pos, c1, c2, end);
                last_control = Some(c2);
                last_quad = None;
                pos = end;
            }
            b'Q' | b'T' => {
                let c = if cmd.eq_ignore_ascii_case(&b'Q') {
                    let (Some(x), Some(y)) = (t.number(), t.number()) else {
                        break;
                    };
                    pos2(x, y) + base
                } else {
                    last_quad.map(|c| pos + (pos - c)).unwrap_or(pos)
                };
                let (Some(x), Some(y)) = (t.number(), t.number()) else {
                    break;
                };
                let end = pos2(x, y) + base;
                if current.is_empty() {
                    current.push(pos);
                }
                // Elevate to a cubic.
                let c1 = pos + (c - pos) * (2.0 / 3.0);
                let c2 = end + (c - end) * (2.0 / 3.0);
                cubic(&mut current, pos, c1, c2, end);
                last_quad = Some(c);
                last_control = None;
                pos = end;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rotation)) = (t.number(), t.number(), t.number())
                else {
                    break;
                };
                let (Some(large), Some(sweep)) = (t.flag(), t.flag()) else {
                    break;
                };
                let (Some(x), Some(y)) = (t.number(), t.number()) else {
                    break;
                };
                let end = pos2(x, y) + base;
                if current.is_empty() {
                    current.push(pos);
                }
                arc(&mut current, pos, rx, ry, rotation, large, sweep, end);
                pos = end;
                last_control = None;
                last_quad = None;
            }
            b'Z' => {
                pos = start;
                flush(&mut current, &mut lines, true);
                current.push(pos);
                last_control = None;
                last_quad = None;
                // `Z` takes no numbers: only a command may follow.
                t.skip_separators();
                if !t.s.get(t.i).is_some_and(|c| c.is_ascii_alphabetic()) {
                    break;
                }
            }
            _ => break,
        }
    }
    flush(&mut current, &mut lines, false);
    lines
}

fn cubic(out: &mut Vec<Pos2>, p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2) {
    let length = (p1 - p0).length() + (p2 - p1).length() + (p3 - p2).length();
    let steps = ((length * 2.0).ceil() as usize).clamp(4, 32);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let u = 1.0 - t;
        let p = p0.to_vec2() * (u * u * u)
            + p1.to_vec2() * (3.0 * u * u * t)
            + p2.to_vec2() * (3.0 * u * t * t)
            + p3.to_vec2() * (t * t * t);
        out.push(p.to_pos2());
    }
}

/// SVG arc (endpoint parameterisation, F.6.5 of the SVG spec).
#[allow(clippy::too_many_arguments)]
fn arc(
    out: &mut Vec<Pos2>,
    from: Pos2,
    rx: f32,
    ry: f32,
    rotation_deg: f32,
    large: bool,
    sweep: bool,
    to: Pos2,
) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 || from == to {
        out.push(to);
        return;
    }
    let phi = rotation_deg.to_radians();
    let (sin, cos) = phi.sin_cos();
    let d = (from - to) / 2.0;
    let x1 = cos * d.x + sin * d.y;
    let y1 = -sin * d.x + cos * d.y;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cx1 = coef * rx * y1 / ry;
    let cy1 = -coef * ry * x1 / rx;
    let mid = (from.to_vec2() + to.to_vec2()) / 2.0;
    let center = pos2(cos * cx1 - sin * cy1 + mid.x, sin * cx1 + cos * cy1 + mid.y);
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 { -a } else { a }
    };
    let theta1 = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut delta = angle(
        (x1 - cx1) / rx,
        (y1 - cy1) / ry,
        (-x1 - cx1) / rx,
        (-y1 - cy1) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= std::f32::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f32::consts::TAU;
    }
    let steps = ((delta.abs() / std::f32::consts::TAU * 64.0).ceil() as usize).max(2);
    for i in 1..=steps {
        let a = theta1 + delta * i as f32 / steps as f32;
        let (s, c) = a.sin_cos();
        out.push(pos2(
            center.x + cos * rx * c - sin * ry * s,
            center.y + sin * rx * c + cos * ry * s,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_icon() {
        for icon in Icon::ALL {
            assert!(!icon.polylines().is_empty(), "{icon:?} has no shapes");
            for line in icon.polylines() {
                for p in &line.points {
                    assert!(
                        p.x > -1.0 && p.x < 25.0 && p.y > -1.0 && p.y < 25.0,
                        "{icon:?} point {p:?} is outside the box"
                    );
                }
            }
        }
    }

    #[test]
    fn parses_relative_commands_and_packed_numbers() {
        let lines = parse_path("m7 16.5-4.74-2.85");
        assert_eq!(lines.len(), 1);
        let pts = &lines[0].points;
        assert_eq!(pts[0], pos2(7.0, 16.5));
        assert!((pts[1].x - 2.26).abs() < 1e-4 && (pts[1].y - 13.65).abs() < 1e-4);
        let closed = parse_path("M2 2h4v4Z");
        assert!(closed[0].closed);
        assert_eq!(closed[0].points.len(), 3);
    }

    #[test]
    fn arcs_end_where_they_should() {
        // The loader: three quarters of a circle of radius 9 around (12, 12).
        let lines = parse_path("M21 12a9 9 0 1 1-6.219-8.56");
        let last = *lines[0].points.last().unwrap();
        assert!((last.x - 14.781).abs() < 1e-3 && (last.y - 3.44).abs() < 1e-3);
        for p in &lines[0].points {
            assert!(((*p - pos2(12.0, 12.0)).length() - 9.0).abs() < 0.05);
        }
    }

    #[test]
    fn rasterizes_a_crisp_line() {
        let image = rasterize(
            &[Polyline {
                points: vec![pos2(0.0, 12.0), pos2(24.0, 12.0)],
                closed: false,
            }],
            24,
            2.0,
        );
        let alpha = |x: usize, y: usize| image.pixels[y * 24 + x].a();
        assert_eq!(alpha(12, 11), 255);
        assert_eq!(alpha(12, 12), 255);
        assert_eq!(alpha(12, 9), 0);
    }
}
