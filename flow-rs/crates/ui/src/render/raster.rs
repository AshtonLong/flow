//! Rasterises one horizontal band of the frame. The rules follow what the
//! glow painter asks of the GPU, so the output matches it:
//!
//! - positions snap to 1/256 px; a pixel is covered when its centre is inside
//!   the triangle, with the top-left rule for centres on an edge, so triangles
//!   that share an edge never blend a pixel twice;
//! - colours are sRGB bytes, multiplied in gamma space by the bilinearly
//!   filtered texel (8-bit filter weights, like the hardware);
//! - the fragment is dithered with the interleaved gradient noise of
//!   `egui_glow`'s fragment shader, rounded to 8 bits, then blended as
//!   premultiplied alpha (`ONE, ONE_MINUS_SRC_ALPHA`) in 8 bits.
//!
//! Pixels are `0xAARRGGBB` (BGRA in memory, what a 32-bit DIB holds).

use egui::epaint::{Mesh, Vertex};
use egui::{Pos2, TextureFilter, TextureWrapMode};

use super::Texture;

/// Fixed-point fraction bits of vertex positions.
const SUB: i64 = 256;
const HALF: i64 = SUB / 2;
/// Vertex positions are clamped to ±2²¹ px so edge functions fit in i64.
const LIMIT: f32 = (1 << 21) as f32;

/// A band of rows `y0..y0 + rows` of a `width`×`height` frame.
pub(super) struct Band<'a> {
    pub px: &'a mut [u32],
    pub width: usize,
    pub height: usize,
    pub y0: usize,
}

/// A pixel rectangle `x0..x1` × `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Clip {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl Clip {
    fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

/// Triangles per chunk of a prepared mesh (an even number, so quads stay whole).
const CHUNK: usize = 32;

/// A run of a mesh's indices, the pixel rows its triangles touch, and a hash
/// of what it draws.
pub(super) struct Chunk {
    start: usize,
    end: usize,
    rows: (i64, i64),
    hash: u64,
}

/// One step of FxHash: each step is a bijection of the running hash, so a
/// single changed word always changes the result.
#[inline]
pub(super) fn mix(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95)
}

/// A mesh with its positions in fixed-point pixels, done once per frame and
/// shared by the bands.
pub(super) struct Prepared<'a> {
    mesh: &'a Mesh,
    tex: &'a Texture,
    white: bool,
    clip: Clip,
    pos: Vec<(i64, i64)>,
    chunks: Vec<Chunk>,
    /// Texture identity and version, and the clip.
    meta: u64,
}

impl<'a> Prepared<'a> {
    pub fn new(mesh: &'a Mesh, tex: &'a Texture, white: bool, clip: Clip, ppp: f32, texture_key: u64) -> Self {
        let pos: Vec<(i64, i64)> = mesh.vertices.iter().map(|v| fixed(v.pos, ppp)).collect();
        let idx = &mesh.indices;
        let mut chunks = Vec::with_capacity(idx.len() / (3 * CHUNK) + 1);
        let mut start = 0;
        while start < idx.len() {
            let end = (start + 3 * CHUNK).min(idx.len());
            let (mut lo, mut hi) = (i64::MAX, i64::MIN);
            let mut hash = 0;
            for &k in &idx[start..end] {
                if let (Some(&(x, y)), Some(v)) = (pos.get(k as usize), mesh.vertices.get(k as usize)) {
                    lo = lo.min(y);
                    hi = hi.max(y);
                    hash = mix(hash, ((x as u64) << 32) ^ (y as u64));
                    hash = mix(hash, (u64::from(v.uv.x.to_bits()) << 32) | u64::from(v.uv.y.to_bits()));
                    hash = mix(hash, u64::from(u32::from_ne_bytes(v.color.to_array())));
                } else {
                    hash = mix(hash, u64::MAX);
                }
            }
            if lo <= hi {
                let rows = (first_center_at_or_after(lo), first_center_at_or_after(hi));
                chunks.push(Chunk { start, end, rows, hash });
            }
            start = end;
        }
        let mut meta = mix(texture_key, u64::from(white));
        for v in [clip.x0, clip.y0, clip.x1, clip.y1] {
            meta = mix(meta, v as u64);
        }
        Self { mesh, tex, white, clip, pos, chunks, meta }
    }

    /// Folds what this mesh draws in rows `y0..y1` into `hash`.
    pub fn hash_rows(&self, y0: i64, y1: i64, hash: &mut u64) {
        if self.clip.y1 <= y0 || self.clip.y0 >= y1 {
            return;
        }
        *hash = mix(*hash, self.meta);
        for chunk in &self.chunks {
            if chunk.rows.1 > y0.max(self.clip.y0) && chunk.rows.0 < y1.min(self.clip.y1) {
                *hash = mix(*hash, chunk.hash);
            }
        }
    }
}

/// `ceil(n / SUB)` for any sign.
#[inline]
fn ceil_div(n: i64) -> i64 {
    n.div_euclid(SUB) + i64::from(n.rem_euclid(SUB) != 0)
}

/// The first pixel whose centre is at or after the fixed-point coordinate `v`.
#[inline]
fn first_center_at_or_after(v: i64) -> i64 {
    ceil_div(v - HALF)
}

#[inline]
fn fixed(p: Pos2, ppp: f32) -> (i64, i64) {
    let x = (p.x * ppp).clamp(-LIMIT, LIMIT);
    let y = (p.y * ppp).clamp(-LIMIT, LIMIT);
    ((x * SUB as f32).round() as i64, (y * SUB as f32).round() as i64)
}

#[inline]
fn pack(c: egui::Color32) -> u32 {
    let [r, g, b, a] = c.to_array();
    (u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// `[r, g, b, a]` in 0..1, as the vertex shader hands it on (`a_srgba / 255`).
#[inline]
fn unit(c: egui::Color32) -> [f32; 4] {
    let [r, g, b, a] = c.to_array();
    [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0, f32::from(a) / 255.0]
}

/// The fragment shader's dither: `(interleaved_gradient_noise(gl_FragCoord.xy) - 0.5) * 0.95`,
/// in 8-bit units. `gl_FragCoord` counts rows from the bottom.
#[inline]
fn noise(x: i64, y: i64, height: usize) -> f32 {
    let fx = x as f32 + 0.5;
    let fy = (height as i64 - 1 - y) as f32 + 0.5;
    let f = 0.067_110_56 * fx + 0.005_837_15 * fy;
    let f = f - floor(f);
    let n = 52.982_918_9 * f;
    let n = n - floor(n);
    (n - 0.5) * 0.95
}

/// `f32::floor` without the libm call (the baseline x86-64 target has no
/// rounding instruction); exact for |x| < 2³¹.
#[inline]
fn floor(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i > x { i - 1.0 } else { i }
}

#[inline]
fn quantize(v: f32) -> u32 {
    // `as` saturates: negative values become 0.
    ((v + 0.5) as u32).min(255)
}

/// A shaded fragment (`rgb` and `a` in 8-bit units, before rounding) to its
/// premultiplied 8-bit colour.
#[inline]
fn finish(rgb: [f32; 3], a: f32, n: f32) -> u32 {
    (quantize(a) << 24) | (quantize(rgb[0] + n) << 16) | (quantize(rgb[1] + n) << 8) | quantize(rgb[2] + n)
}

/// `src + dst * (255 - src.a) / 255` per channel, rounded, in 8 bits.
#[inline]
pub(super) fn blend(dst: u32, src: u32) -> u32 {
    let a = src >> 24;
    if a == 255 {
        return src;
    }
    if src == 0 {
        return dst;
    }
    let ia = 255 - a;
    let rb = (dst & 0x00ff_00ff) * ia + 0x0080_0080;
    let rb = ((rb + ((rb >> 8) & 0x00ff_00ff)) >> 8) & 0x00ff_00ff;
    let ag = ((dst >> 8) & 0x00ff_00ff) * ia + 0x0080_0080;
    let ag = (ag + ((ag >> 8) & 0x00ff_00ff)) & 0xff00_ff00;
    let under = rb | ag;
    let (r, g, b) = ((src >> 16) & 0xff, (src >> 8) & 0xff, src & 0xff);
    if r <= a && g <= a && b <= a {
        // A valid premultiplied colour cannot overflow.
        src + under
    } else {
        let add = |shift: u32| (((src >> shift) & 0xff) + ((under >> shift) & 0xff)).min(255) << shift;
        add(24) | add(16) | add(8) | add(0)
    }
}

fn blend_span(row: &mut [u32], src: u32) {
    if src >> 24 == 255 {
        row.fill(src);
    } else if src != 0 {
        for d in row {
            *d = blend(*d, src);
        }
    }
}

/// Texture lookups as the GPU does them for one triangle or quad.
struct Sampler<'a> {
    tex: &'a Texture,
    w: f32,
    h: f32,
    linear: bool,
}

impl<'a> Sampler<'a> {
    /// `rho` is texels per pixel; above 1 the minification filter applies.
    fn new(tex: &'a Texture, rho: f32) -> Self {
        let filter = if rho > 1.0 { tex.options.minification } else { tex.options.magnification };
        Self { tex, w: tex.width as f32, h: tex.height as f32, linear: filter == TextureFilter::Linear }
    }

    #[inline]
    fn wrap(&self, i: i64, n: usize) -> usize {
        let n = n as i64;
        let i = match self.tex.options.wrap_mode {
            TextureWrapMode::ClampToEdge => i.clamp(0, n - 1),
            TextureWrapMode::Repeat => i.rem_euclid(n),
            TextureWrapMode::MirroredRepeat => {
                let m = i.rem_euclid(2 * n);
                if m < n { m } else { 2 * n - 1 - m }
            }
        };
        i as usize
    }

    #[inline]
    fn texel(&self, x: usize, y: usize) -> u32 {
        self.tex.texels[y * self.tex.width + x]
    }

    /// The premultiplied texel at `uv`, channels `[r, g, b, a]` in 8-bit units.
    #[inline]
    fn sample(&self, u: f32, v: f32) -> Option<[f32; 4]> {
        if !self.linear {
            let x = self.wrap(floor(u * self.w) as i64, self.tex.width);
            let y = self.wrap(floor(v * self.h) as i64, self.tex.height);
            let t = self.texel(x, y);
            if t == 0 {
                return None;
            }
            return Some(channels(t).map(|c| c as f32));
        }
        let (x0, wx) = split(u * self.w - 0.5);
        let (y0, wy) = split(v * self.h - 0.5);
        let xa = self.wrap(x0, self.tex.width);
        let ya = self.wrap(y0, self.tex.height);
        if wx == 0 && wy == 0 {
            let t = self.texel(xa, ya);
            if t == 0 {
                return None;
            }
            return Some(channels(t).map(|c| c as f32));
        }
        let xb = self.wrap(x0 + 1, self.tex.width);
        let yb = self.wrap(y0 + 1, self.tex.height);
        let (t00, t10, t01, t11) = (self.texel(xa, ya), self.texel(xb, ya), self.texel(xa, yb), self.texel(xb, yb));
        if t00 | t10 | t01 | t11 == 0 {
            return None;
        }
        let (c00, c10, c01, c11) = (channels(t00), channels(t10), channels(t01), channels(t11));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = c00[i] * (256 - wx) + c10[i] * wx;
            let bottom = c01[i] * (256 - wx) + c11[i] * wx;
            out[i] = (top * (256 - wy) + bottom * wy) as f32 / 65_536.0;
        }
        Some(out)
    }
}

/// A texel coordinate to its integer part and an 8-bit filter weight.
#[inline]
fn split(t: f32) -> (i64, u32) {
    let floor = floor(t);
    let w = ((t - floor) * 256.0 + 0.5) as u32;
    if w >= 256 { (floor as i64 + 1, 0) } else { (floor as i64, w) }
}

#[inline]
fn channels(t: u32) -> [u32; 4] {
    [(t >> 16) & 0xff, (t >> 8) & 0xff, t & 0xff, t >> 24]
}

impl Band<'_> {
    fn rows(&self) -> (i64, i64) {
        let rows = self.px.len() / self.width;
        (self.y0 as i64, (self.y0 + rows) as i64)
    }

    #[inline]
    fn row(&mut self, y: i64) -> &mut [u32] {
        let start = (y as usize - self.y0) * self.width;
        &mut self.px[start..start + self.width]
    }

    /// Draws a prepared mesh.
    pub fn mesh(&mut self, prep: &Prepared<'_>) {
        let (y0, y1) = self.rows();
        let clip = Clip { y0: prep.clip.y0.max(y0), y1: prep.clip.y1.min(y1), ..prep.clip };
        if clip.is_empty() {
            return;
        }
        let idx = &prep.mesh.indices;
        let v = &prep.mesh.vertices;
        let pos = &prep.pos;
        let n = v.len();
        for chunk in &prep.chunks {
            if chunk.rows.1 <= clip.y0 || chunk.rows.0 >= clip.y1 {
                continue;
            }
            let end = chunk.end;
            let mut i = chunk.start;
            while i + 3 <= end {
                let t = [idx[i] as usize, idx[i + 1] as usize, idx[i + 2] as usize];
                if t.iter().any(|&k| k >= n) {
                    i += 3;
                    continue;
                }
                if i + 6 <= end {
                    let q = [idx[i + 3] as usize, idx[i + 4] as usize, idx[i + 5] as usize];
                    if q.iter().all(|&k| k < n) && self.quad(v, pos, t, q, prep, clip) {
                        i += 6;
                        continue;
                    }
                }
                self.triangle([&v[t[0]], &v[t[1]], &v[t[2]]], [pos[t[0]], pos[t[1]], pos[t[2]]], prep, clip);
                i += 3;
            }
        }
    }

    /// Two triangles that make an axis-aligned rectangle with one colour and
    /// an axis-aligned texture mapping: filled row by row. Returns false (and
    /// draws nothing) for anything else.
    fn quad(
        &mut self,
        v: &[Vertex],
        pos: &[(i64, i64)],
        t1: [usize; 3],
        t2: [usize; 3],
        prep: &Prepared<'_>,
        clip: Clip,
    ) -> bool {
        let (tex, white) = (prep.tex, prep.white);
        if t1[0] == t1[1] || t1[1] == t1[2] || t1[0] == t1[2] {
            return false;
        }
        // Exactly two shared vertices: p is t1's other one, q is t2's.
        let mut shared = [0usize; 2];
        let mut count = 0;
        let mut p = usize::MAX;
        for k in t1 {
            if t2.contains(&k) {
                if count < 2 {
                    shared[count] = k;
                }
                count += 1;
            } else {
                p = k;
            }
        }
        if count != 2 {
            return false;
        }
        let Some(q) = t2.iter().copied().find(|k| !shared.contains(k)) else { return false };
        let (s, r) = (shared[0], shared[1]);
        let color = v[s].color;
        if v[r].color != color || v[p].color != color || v[q].color != color {
            return false;
        }
        let (sp, rp, pp, qp) = (pos[s], pos[r], pos[p], pos[q]);
        // The shared edge must be a diagonal and p, q the other two corners.
        if sp.0 == rp.0 || sp.1 == rp.1 {
            return false;
        }
        let c1 = (sp.0, rp.1);
        let c2 = (rp.0, sp.1);
        if !((pp == c1 && qp == c2) || (pp == c2 && qp == c1)) {
            return false;
        }
        let solid = white && [s, r, p, q].iter().all(|&k| v[k].uv == egui::epaint::WHITE_UV);
        // u must follow x only and v follow y only.
        if !solid {
            for &a in &[s, r, p, q] {
                for &b in &[s, r, p, q] {
                    let (fa, fb) = (pos[a], pos[b]);
                    if (fa.0 == fb.0 && v[a].uv.x != v[b].uv.x) || (fa.1 == fb.1 && v[a].uv.y != v[b].uv.y) {
                        return false;
                    }
                }
            }
        }
        let (left, right) = if sp.0 < rp.0 { (s, r) } else { (r, s) };
        let (top, bottom) = if sp.1 < rp.1 { (s, r) } else { (r, s) };
        let (fx0, fx1) = (sp.0.min(rp.0), sp.0.max(rp.0));
        let (fy0, fy1) = (sp.1.min(rp.1), sp.1.max(rp.1));
        let x0 = first_center_at_or_after(fx0).max(clip.x0);
        let x1 = first_center_at_or_after(fx1).min(clip.x1);
        let y0 = first_center_at_or_after(fy0).max(clip.y0);
        let y1 = first_center_at_or_after(fy1).min(clip.y1);
        if x0 >= x1 || y0 >= y1 {
            return true;
        }
        if solid {
            let src = pack(color);
            for y in y0..y1 {
                blend_span(&mut self.row(y)[x0 as usize..x1 as usize], src);
            }
            return true;
        }
        if color.a() == 0 && color.r() == 0 && color.g() == 0 && color.b() == 0 {
            return true;
        }
        let (u0, u1) = (v[left].uv.x, v[right].uv.x);
        let (v0, v1) = (v[top].uv.y, v[bottom].uv.y);
        let (w, h) = ((fx1 - fx0) as f32, (fy1 - fy0) as f32);
        let rho = (((u1 - u0) * tex.width as f32 * SUB as f32 / w).abs())
            .max(((v1 - v0) * tex.height as f32 * SUB as f32 / h).abs());
        let sampler = Sampler::new(tex, rho);
        let tint = unit(color);
        let height = self.height;
        for y in y0..y1 {
            let fy = (y * SUB + HALF - fy0) as f32 / h;
            let tv = v0 + fy * (v1 - v0);
            let row = self.row(y);
            for x in x0..x1 {
                let fx = (x * SUB + HALF - fx0) as f32 / w;
                let tu = u0 + fx * (u1 - u0);
                let Some(t) = sampler.sample(tu, tv) else { continue };
                let rgb = [tint[0] * t[0], tint[1] * t[1], tint[2] * t[2]];
                let src = finish(rgb, tint[3] * t[3], noise(x, y, height));
                let d = &mut row[x as usize];
                *d = blend(*d, src);
            }
        }
        true
    }

    fn triangle(&mut self, v: [&Vertex; 3], p: [(i64, i64); 3], prep: &Prepared<'_>, clip: Clip) {
        let (tex, white) = (prep.tex, prep.white);
        let mut p = p;
        let mut v = v;
        let mut area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
        if area == 0 {
            return;
        }
        if area < 0 {
            p.swap(1, 2);
            v.swap(1, 2);
            area = -area;
        }
        let min_y = p[0].1.min(p[1].1).min(p[2].1);
        let max_y = p[0].1.max(p[1].1).max(p[2].1);
        let min_x = p[0].0.min(p[1].0).min(p[2].0);
        let max_x = p[0].0.max(p[1].0).max(p[2].0);
        let y0 = first_center_at_or_after(min_y).max(clip.y0);
        let y1 = first_center_at_or_after(max_y).min(clip.y1);
        let bx0 = first_center_at_or_after(min_x).max(clip.x0);
        let bx1 = first_center_at_or_after(max_x).min(clip.x1);
        if y0 >= y1 || bx0 >= bx1 {
            return;
        }
        // Edge k runs from p[k] to p[k+1]; E_k(x, y) = a*x + b*y + c is the
        // weight of the opposite vertex p[k+2] (times `area`).
        let mut edges = [(0i64, 0i64, 0i64, 0i64); 3];
        for (k, edge) in edges.iter_mut().enumerate() {
            let (from, to) = (p[k], p[(k + 1) % 3]);
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let a = -dy;
            let b = dx;
            let c = -(a * from.0 + b * from.1);
            // Top-left rule: centres on left and top edges are inside.
            let bias = if dy < 0 || (dy == 0 && dx > 0) { 0 } else { -1 };
            *edge = (a, b, c, bias);
        }
        let colors = [v[0].color, v[1].color, v[2].color];
        let flat = colors[0] == colors[1] && colors[1] == colors[2];
        // Untextured: the white texel of the font atlas, which samples to exactly 1.
        let untextured = white && v.iter().all(|v| v.uv == egui::epaint::WHITE_UV);
        if flat && colors[0] == egui::Color32::TRANSPARENT {
            return;
        }
        let inv_area = 1.0 / area as f32;
        let sampler = if untextured {
            None
        } else {
            // Texels per pixel from the affine uv mapping.
            let du = [v[1].uv.x - v[0].uv.x, v[2].uv.x - v[0].uv.x];
            let dv = [v[1].uv.y - v[0].uv.y, v[2].uv.y - v[0].uv.y];
            let dp = [(p[1].0 - p[0].0) as f32, (p[1].1 - p[0].1) as f32, (p[2].0 - p[0].0) as f32, (p[2].1 - p[0].1) as f32];
            let det = dp[0] * dp[3] - dp[1] * dp[2];
            let s = SUB as f32 / det;
            let dudx = (du[0] * dp[3] - du[1] * dp[1]) * s;
            let dudy = (du[1] * dp[0] - du[0] * dp[2]) * s;
            let dvdx = (dv[0] * dp[3] - dv[1] * dp[1]) * s;
            let dvdy = (dv[1] * dp[0] - dv[0] * dp[2]) * s;
            let rho = (dudx.abs().max(dudy.abs()) * tex.width as f32).max(dvdx.abs().max(dvdy.abs()) * tex.height as f32);
            Some(Sampler::new(tex, rho))
        };
        // Per vertex: colour in 8-bit units, then uv.
        let attr: [[f32; 6]; 3] = std::array::from_fn(|k| {
            let c = unit(colors[k]);
            [c[0] * 255.0, c[1] * 255.0, c[2] * 255.0, c[3] * 255.0, v[k].uv.x, v[k].uv.y]
        });
        let dw2 = edges[0].0 as f32 * SUB as f32 * inv_area;
        let dw1 = edges[2].0 as f32 * SUB as f32 * inv_area;
        let step: [f32; 6] =
            std::array::from_fn(|i| dw1 * (attr[1][i] - attr[0][i]) + dw2 * (attr[2][i] - attr[0][i]));
        let src_flat = pack(colors[0]);
        let height = self.height;
        let inv: [f64; 3] = edges.map(|e| if e.0 == 0 { 0.0 } else { 1.0 / (e.0 * SUB) as f64 });
        for y in y0..y1 {
            let py = y * SUB + HALF;
            // The span from the edges, in floats, widened by a pixel; the exact
            // integer test then trims it.
            let mut lo = bx0;
            let mut hi = bx1 - 1;
            let mut empty = false;
            let mut rowc = [0i64; 3];
            for (k, &(a, b, cc, bias)) in edges.iter().enumerate() {
                let r = b * py + cc;
                rowc[k] = r;
                if a == 0 {
                    if r + bias < 0 {
                        empty = true;
                    }
                    continue;
                }
                // a*x + r >= 0 at the pixel whose centre is at x: index t.
                let t = -(r as f64) * inv[k] - 0.5;
                let i = t as i64 - i64::from((t as i64 as f64) > t);
                if a > 0 {
                    lo = lo.max(i);
                } else {
                    hi = hi.min(i + 1);
                }
            }
            if empty || lo > hi {
                continue;
            }
            let inside = |x: i64| -> bool {
                let px = x * SUB + HALF;
                edges.iter().zip(rowc).all(|(&(a, _, _, bias), r)| a * px + r + bias >= 0)
            };
            while lo <= hi && !inside(lo) {
                lo += 1;
            }
            while hi >= lo && !inside(hi) {
                hi -= 1;
            }
            if lo > hi {
                continue;
            }
            let row = self.row(y);
            if untextured && flat {
                blend_span(&mut row[lo as usize..=hi as usize], src_flat);
                continue;
            }
            // The attributes are affine in x: work them out at the span's start
            // and step them along.
            let px = lo * SUB + HALF;
            let w2 = (edges[0].0 * px + rowc[0]) as f32 * inv_area;
            let w1 = (edges[2].0 * px + rowc[2]) as f32 * inv_area;
            let mut at = [0.0f32; 6];
            for (i, o) in at.iter_mut().enumerate() {
                *o = attr[0][i] + w1 * (attr[1][i] - attr[0][i]) + w2 * (attr[2][i] - attr[0][i]);
            }
            for x in lo..=hi {
                if x > lo {
                    for i in 0..6 {
                        at[i] += step[i];
                    }
                }
                let mut frag = [at[0], at[1], at[2], at[3]];
                if let Some(s) = &sampler {
                    let Some(t) = s.sample(at[4], at[5]) else { continue };
                    for i in 0..4 {
                        frag[i] *= t[i] * (1.0 / 255.0);
                    }
                }
                let src = finish([frag[0], frag[1], frag[2]], frag[3], noise(x, y, height));
                let d = &mut row[x as usize];
                *d = blend(*d, src);
            }
        }
    }
}
