//! A CPU renderer for egui's tessellated meshes. It replaces the glow
//! painter: an OpenGL context costs the window 60–90 MB of driver memory,
//! while the settings UI is a few thousand triangles that a CPU fills in a
//! couple of milliseconds. The output matches `egui_glow` (see [`raster`]).
//!
//! The frame is split into bands of rows painted in parallel; each band
//! clears itself and draws every primitive clipped to its rows, so the result
//! does not depend on the thread count. A band whose triangles, textures and
//! clip rects hash the same as last frame is left as it is, so a blinking
//! caret or a level meter repaints a strip, not the window.

mod raster;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use egui::epaint::{ImageData, Primitive};
use egui::{ClippedPrimitive, Color32, TextureId, TextureOptions, TexturesDelta};

use raster::{Band, Clip, Prepared};

/// Rows per band.
const BAND_ROWS: usize = 32;

pub(crate) struct Texture {
    width: usize,
    height: usize,
    /// Premultiplied texels as `0xAARRGGBB`.
    texels: Vec<u32>,
    options: TextureOptions,
    /// Bumped on every change, for the band hashes.
    version: u64,
}

/// Textures and the painting of frames into a pixel buffer.
pub(crate) struct Renderer {
    textures: HashMap<TextureId, Texture>,
    threads: usize,
    /// Each band's hash as last painted, for the frame size and clear colour in `last`.
    bands: Vec<u64>,
    last: (usize, usize, u32),
    versions: u64,
}

/// A `width`×`height` frame of `0xAARRGGBB` premultiplied pixels, top row first.
pub(crate) struct Target<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

fn pack(c: Color32) -> u32 {
    let [r, g, b, a] = c.to_array();
    (u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// A packed pixel back to egui's premultiplied colour.
pub(crate) fn unpack(p: u32) -> Color32 {
    Color32::from_rgba_premultiplied((p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8)
}

impl Renderer {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        // Physical cores at most; past four the bands are too thin to matter.
        let threads = std::env::var("FLOW_RENDER_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or((cores / 2).clamp(1, 4));
        Self::with_threads(threads)
    }

    fn with_threads(threads: usize) -> Self {
        Self { textures: HashMap::new(), threads, bands: Vec::new(), last: (0, 0, 0), versions: 0 }
    }

    /// Forgets what was painted, so the next frame paints every band (the
    /// buffer was replaced).
    pub fn invalidate(&mut self) {
        self.bands.clear();
    }

    /// Applies the new and changed textures of a frame (call before [`Self::paint`]).
    pub fn set_textures(&mut self, delta: &TexturesDelta) {
        for (id, deltas) in &delta.set {
            for image_delta in deltas {
                let ImageData::Color(image) = &image_delta.image;
                let [w, h] = image.size;
                let texels = image.pixels.iter().map(|&c| pack(c));
                self.versions += 1;
                let version = self.versions;
                match image_delta.pos {
                    None => {
                        let texels = texels.collect();
                        self.textures
                            .insert(*id, Texture { width: w, height: h, texels, options: image_delta.options, version });
                    }
                    Some([x, y]) => {
                        let Some(tex) = self.textures.get_mut(id) else { continue };
                        tex.options = image_delta.options;
                        tex.version = version;
                        let texels: Vec<u32> = texels.collect();
                        for row in 0..h {
                            if y + row >= tex.height {
                                break;
                            }
                            let n = w.min(tex.width.saturating_sub(x));
                            let start = (y + row) * tex.width + x;
                            tex.texels[start..start + n].copy_from_slice(&texels[row * w..row * w + n]);
                        }
                    }
                }
            }
        }
    }

    /// Drops the textures a frame freed (call after [`Self::paint`]).
    pub fn free_textures(&mut self, delta: &TexturesDelta) {
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// Clears `target` to `clear` and paints `primitives` (positions in points)
    /// over it, skipping the bands that would come out as they already are.
    /// `target` must hold the previous frame unless [`Self::invalidate`] was called.
    /// Returns how many rows were painted.
    pub fn paint(&mut self, target: Target<'_>, clear: Color32, primitives: &[ClippedPrimitive], ppp: f32) -> usize {
        let Target { pixels, width, height } = target;
        if width == 0 || height == 0 {
            return 0;
        }
        let pixels = &mut pixels[..width * height];
        let clear = pack(clear);
        let textures = &self.textures;
        let meshes: Vec<Prepared<'_>> = primitives
            .iter()
            .filter_map(|prim| {
                let Primitive::Mesh(mesh) = &prim.primitive else { return None };
                let tex = textures.get(&mesh.texture_id)?;
                let clip = scissor(prim.clip_rect, ppp, width, height);
                let key = raster::mix(texture_key(mesh.texture_id), tex.version);
                (clip.x0 < clip.x1 && clip.y0 < clip.y1)
                    .then(|| Prepared::new(mesh, tex, mesh.texture_id == TextureId::default(), clip, ppp, key))
            })
            .collect();
        let count = height.div_ceil(BAND_ROWS);
        if self.last != (width, height, clear) || self.bands.len() != count {
            self.bands = vec![0; count];
            self.last = (width, height, clear);
            // A hash of 0 never matches below.
            self.bands.iter_mut().for_each(|h| *h = 0);
        }
        let mut painted = 0;
        let mut bands: Vec<Option<Band<'_>>> = pixels
            .chunks_mut(width * BAND_ROWS)
            .enumerate()
            .map(|(i, px)| {
                let y0 = i * BAND_ROWS;
                let rows = px.len() / width;
                let mut hash = 1;
                for mesh in &meshes {
                    mesh.hash_rows(y0 as i64, (y0 + rows) as i64, &mut hash);
                }
                // Never 0, which marks a band as unknown.
                let hash = hash | 1;
                if self.bands[i] == hash {
                    return None;
                }
                self.bands[i] = hash;
                painted += rows;
                Some(Band { px, width, height, y0 })
            })
            .collect();
        let job = |band: &mut Band<'_>| {
            band.px.fill(clear);
            for mesh in &meshes {
                band.mesh(mesh);
            }
        };
        let threads = self.threads.min(bands.iter().flatten().count());
        if threads <= 1 {
            for band in bands.iter_mut().flatten() {
                job(band);
            }
            return painted;
        }
        let slots: Vec<parking_lot::Mutex<Option<Band<'_>>>> =
            bands.iter_mut().filter(|b| b.is_some()).map(|b| parking_lot::Mutex::new(b.take())).collect();
        let next = AtomicUsize::new(0);
        let work = || {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(slot) = slots.get(i) else { break };
                if let Some(band) = slot.lock().as_mut() {
                    job(band);
                }
            }
        };
        std::thread::scope(|s| {
            for _ in 1..threads {
                s.spawn(work);
            }
            work();
        });
        painted
    }
}

fn texture_key(id: TextureId) -> u64 {
    match id {
        TextureId::Managed(n) => n << 1,
        TextureId::User(n) => (n << 1) | 1,
    }
}

/// The clip rect in pixels, rounded and clamped as `egui_glow` sets its scissor.
fn scissor(rect: egui::Rect, ppp: f32, width: usize, height: usize) -> Clip {
    let (w, h) = (width as i64, height as i64);
    let x0 = ((rect.min.x * ppp).round() as i64).clamp(0, w);
    let y0 = ((rect.min.y * ppp).round() as i64).clamp(0, h);
    let x1 = ((rect.max.x * ppp).round() as i64).clamp(x0, w);
    let y1 = ((rect.max.y * ppp).round() as i64).clamp(y0, h);
    Clip { x0, y0, x1, y1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    use egui::{Rect, pos2};

    fn render(meshes: Vec<Mesh>, w: usize, h: usize, threads: usize) -> Vec<u32> {
        let mut r = Renderer::with_threads(threads);
        let mut delta = TexturesDelta::default();
        let atlas = egui::ColorImage::new([4, 4], vec![Color32::WHITE; 16]);
        delta.set.insert(TextureId::default(), [egui::epaint::ImageDelta::full(atlas, TextureOptions::LINEAR)].into());
        r.set_textures(&delta);
        let prims: Vec<ClippedPrimitive> = meshes
            .into_iter()
            .map(|m| ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(m) })
            .collect();
        let mut px = vec![0u32; w * h];
        r.paint(Target { pixels: &mut px, width: w, height: h }, Color32::BLACK, &prims, 1.0);
        px
    }

    /// A light-theme frame (cards with drop shadows and text, like the
    /// General page), painted single-threaded:
    /// `cargo test --release -p flow-ui render::tests::frame_time -- --ignored --nocapture`.
    #[test]
    #[ignore = "a benchmark"]
    fn frame_time() {
        use crate::design::{self, Tokens, text};
        use crate::stores::Appearance;
        let ctx = egui::Context::default();
        design::fonts::install(&ctx);
        for dark in [false, true] {
            let appearance = Appearance { dark, high_contrast: false, reduced_motion: true };
            design::apply(&ctx, Tokens::new(appearance, &crate::platform::system_colors()));
            let input = || egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 700.0))),
                ..Default::default()
            };
            let mut paint = |ui: &mut egui::Ui| {
                let t = design::tokens(ui.ctx());
                ui.painter().rect_filled(ui.max_rect(), 0.0, t.mica);
                for i in 0..9 {
                    let rect = Rect::from_min_size(pos2(256.0, 60.0 + i as f32 * 72.0), egui::vec2(696.0, 64.0));
                    design::shadow::paint_surface(ui.painter(), rect, 8.0, t.card, &t.shadow_card, None);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(16.0)));
                    text::label(&mut child, "Launch at login", text::TextStyle::BODY, t.fg);
                    text::label(&mut child, "Start Flow in the system tray when you sign in.", text::TextStyle::CAPTION, t.fg_2);
                }
            };
            let mut out = ctx.run_ui(input(), &mut paint);
            for _ in 0..3 {
                out = ctx.run_ui(input(), &mut paint);
            }
            let mut r = Renderer::with_threads(1);
            let mut all = ctx.run_ui(input(), &mut paint);
            all.textures_delta.append(out.textures_delta);
            r.set_textures(&ctx.fonts(|f| {
                let mut d = TexturesDelta::default();
                d.set.insert(TextureId::default(), [egui::epaint::ImageDelta::full(f.image(), TextureOptions::LINEAR)].into());
                d
            }));
            let prims = ctx.tessellate(all.shapes, 1.0);
            let mut px = vec![0u32; 1000 * 700];
            let start = std::time::Instant::now();
            let n = 100;
            for _ in 0..n {
                r.invalidate();
                r.paint(Target { pixels: &mut px, width: 1000, height: 700 }, Color32::BLACK, &prims, 1.0);
            }
            let tris: usize = prims
                .iter()
                .map(|p| if let Primitive::Mesh(m) = &p.primitive { m.indices.len() / 3 } else { 0 })
                .sum();
            eprintln!("dark={dark}: {tris} triangles, {:.2} ms per frame", start.elapsed().as_secs_f64() * 1000.0 / n as f64);
        }
    }

    #[test]
    fn the_blend_rounds_exactly() {
        for a in 0..=255u32 {
            for d in 0..=255u32 {
                let src = a << 24;
                let dst = (d << 24) | (d << 16) | (d << 8) | d;
                let out = raster::blend(dst, src);
                let want = (f64::from(d) * f64::from(255 - a) / 255.0).round() as u32;
                assert_eq!(out & 0xff, want, "a={a} d={d}");
                assert_eq!(out >> 24, a + want, "alpha a={a} d={d}");
            }
        }
    }

    #[test]
    fn a_rect_covers_the_pixels_whose_centres_it_holds() {
        let mut mesh = Mesh::default();
        mesh.add_colored_rect(Rect::from_min_max(pos2(1.0, 1.0), pos2(3.0, 4.0)), Color32::WHITE);
        let px = render(vec![mesh], 5, 5, 1);
        for y in 0..5 {
            for x in 0..5 {
                let inside = (1..3).contains(&x) && (1..4).contains(&y);
                assert_eq!(px[y * 5 + x] == 0xffff_ffff, inside, "({x}, {y})");
            }
        }
    }

    #[test]
    fn triangles_sharing_an_edge_blend_each_pixel_once() {
        // A translucent fan of slivers: every pixel must be blended exactly once.
        let color = Color32::from_rgba_premultiplied(60, 60, 60, 60);
        let mut mesh = Mesh::default();
        let center = pos2(8.0, 8.0);
        mesh.vertices.push(Vertex { pos: center, uv: WHITE_UV, color });
        let n = 24;
        for i in 0..=n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            mesh.vertices.push(Vertex { pos: center + egui::vec2(a.cos(), a.sin()) * 7.3, uv: WHITE_UV, color });
        }
        for i in 1..=n as u32 {
            mesh.add_triangle(0, i, i + 1);
        }
        let px = render(vec![mesh], 16, 16, 1);
        let once = raster::blend(0xff00_0000, 0x3c3c_3c3c);
        assert!(px.iter().all(|&p| p == 0xff00_0000 || p == once), "{px:x?}");
        assert!(px.iter().filter(|&&p| p == once).count() > 100);
    }

    #[test]
    fn bands_and_threads_do_not_change_the_result() {
        let mut mesh = Mesh::default();
        for i in 0..40 {
            let f = i as f32;
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(f * 3.3, f * 7.1), pos2(f * 3.3 + 50.5, f * 7.1 + 33.2)),
                Color32::from_rgba_premultiplied(20, 40, 60, 100),
            );
        }
        let one = render(vec![mesh.clone()], 200, 300, 1);
        let four = render(vec![mesh], 200, 300, 4);
        assert_eq!(one, four);
    }
}
