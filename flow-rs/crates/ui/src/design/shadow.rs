//! CSS `box-shadow`, painted. Rings (`0 0 0 1px`) become strokes outside the
//! shape; blurred layers become a mesh of concentric rounded rectangles whose
//! alpha follows the Gaussian edge profile CSS specifies (σ = blur / 2).

use egui::epaint::{Mesh, RectShape, Shape, StrokeKind};
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Stroke, pos2, vec2};

use super::tokens::BoxShadow;

/// Points per quarter circle; every ring uses the same count so rings can be stitched.
const CORNER_STEPS: usize = 8;
/// Rings across the blurred edge.
const RINGS: usize = 14;

/// Paints `shadows` for a rounded rect, last layer first (CSS paints the first on top).
pub fn paint_box_shadows(painter: &Painter, rect: Rect, radius: f32, shadows: &[BoxShadow]) {
    let mut shapes = Vec::new();
    box_shadow_shapes(&mut shapes, rect, radius, shadows);
    painter.extend(shapes);
}

/// The shapes of [`paint_box_shadows`], for painting into a reserved slot.
pub fn box_shadow_shapes(out: &mut Vec<Shape>, rect: Rect, radius: f32, shadows: &[BoxShadow]) {
    for shadow in shadows.iter().rev() {
        shadow_shape(out, rect, radius, shadow);
    }
}

/// A raised surface: shadows, then the fill, then (high contrast) a border.
pub fn paint_surface(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    fill: Color32,
    shadows: &[BoxShadow],
    border: Option<Color32>,
) {
    painter.add(surface(rect, radius, fill, shadows, border));
}

/// The shape of [`paint_surface`]. Reserve a slot with `painter.add(Shape::Noop)`
/// before drawing the content, then `painter.set(slot, surface(...))` once the
/// size is known.
pub fn surface(
    rect: Rect,
    radius: f32,
    fill: Color32,
    shadows: &[BoxShadow],
    border: Option<Color32>,
) -> Shape {
    let mut shapes = Vec::new();
    box_shadow_shapes(&mut shapes, rect, radius, shadows);
    shapes.push(Shape::Rect(RectShape::filled(
        rect,
        CornerRadius::from(radius),
        fill,
    )));
    if let Some(border) = border {
        shapes.push(Shape::Rect(RectShape::stroke(
            rect,
            CornerRadius::from(radius),
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        )));
    }
    Shape::Vec(shapes)
}

fn shadow_shape(out: &mut Vec<Shape>, rect: Rect, radius: f32, shadow: &BoxShadow) {
    if shadow.color.a() == 0 {
        return;
    }
    if shadow.blur <= 0.0 {
        if shadow.offset == egui::Vec2::ZERO && shadow.spread > 0.0 {
            // A ring: exactly the CSS result even over translucent fills.
            out.push(Shape::Rect(RectShape::stroke(
                rect,
                CornerRadius::from(radius),
                Stroke::new(shadow.spread, shadow.color),
                StrokeKind::Outside,
            )));
        } else {
            let shape = rect.translate(shadow.offset).expand(shadow.spread);
            if shape.is_positive() {
                out.push(Shape::Rect(RectShape::filled(
                    shape,
                    CornerRadius::from((radius + shadow.spread).max(0.0)),
                    shadow.color,
                )));
            }
        }
        return;
    }
    let shape = rect.translate(shadow.offset).expand(shadow.spread);
    let base_radius = (radius + shadow.spread).max(0.0);
    let sigma = shadow.blur / 2.0;
    let half = vec2(shape.width().max(0.0), shape.height().max(0.0)) / 2.0;
    // Peak opacity of a blurred box smaller than the blur.
    let peak = erf(half.x / (sigma * std::f32::consts::SQRT_2))
        * erf(half.y / (sigma * std::f32::consts::SQRT_2));
    let inner_limit = half.x.min(half.y);
    let reach = 3.0 * sigma;
    let start = -reach.min(inner_limit);

    let mut mesh = Mesh::default();
    let per_ring = 4 * (CORNER_STEPS + 1);
    let center = shape.center();
    // The innermost ring is filled as a fan from the centre.
    let peak = peak.clamp(0.0, 1.0);
    mesh.colored_vertex(center, alpha(shadow.color, profile(start, sigma) * peak));
    for ring in 0..RINGS {
        let d = start + (reach - start) * ring as f32 / (RINGS - 1) as f32;
        let color = alpha(shadow.color, (profile(d, sigma) * peak).clamp(0.0, 1.0));
        let ring_rect = shape.expand(d);
        let r = (base_radius + d)
            .max(0.0)
            .min(ring_rect.width().min(ring_rect.height()).max(0.0) / 2.0);
        for p in rounded_points(ring_rect, r) {
            mesh.colored_vertex(p, color);
        }
    }
    // Fan: centre to the first ring.
    for i in 0..per_ring {
        let a = 1 + i as u32;
        let b = 1 + ((i + 1) % per_ring) as u32;
        mesh.add_triangle(0, a, b);
    }
    // Strips between rings.
    for ring in 0..RINGS - 1 {
        let inner = 1 + (ring * per_ring) as u32;
        let outer = inner + per_ring as u32;
        for i in 0..per_ring {
            let j = (i + 1) % per_ring;
            let (i, j) = (i as u32, j as u32);
            mesh.add_triangle(inner + i, outer + i, outer + j);
            mesh.add_triangle(inner + i, outer + j, inner + j);
        }
    }
    out.push(Shape::mesh(mesh));
}

fn alpha(color: Color32, factor: f32) -> Color32 {
    color.gamma_multiply(factor)
}

/// Opacity at signed distance `d` outside the shape's edge, for a Gaussian of σ.
fn profile(d: f32, sigma: f32) -> f32 {
    0.5 * (1.0 - erf(d / (sigma * std::f32::consts::SQRT_2)))
}

/// Abramowitz–Stegun 7.1.26; plenty for 8-bit alpha.
fn erf(x: f32) -> f32 {
    let sign = x.signum();
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_4 * t - 1.453_152_1) * t) + 1.421_413_7) * t - 0.284_496_74) * t
            + 0.254_829_6)
            * t
            * (-x * x).exp();
    sign * y
}

/// The outline of a rounded rect, clockwise from the top-left corner's start,
/// with a fixed number of points so rings line up.
fn rounded_points(rect: Rect, r: f32) -> impl Iterator<Item = Pos2> {
    let corners = [
        (pos2(rect.left() + r, rect.top() + r), std::f32::consts::PI),
        (
            pos2(rect.right() - r, rect.top() + r),
            1.5 * std::f32::consts::PI,
        ),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (
            pos2(rect.left() + r, rect.bottom() - r),
            0.5 * std::f32::consts::PI,
        ),
    ];
    corners.into_iter().flat_map(move |(c, start)| {
        (0..=CORNER_STEPS).map(move |i| {
            let angle = start + std::f32::consts::FRAC_PI_2 * i as f32 / CORNER_STEPS as f32;
            pos2(c.x + r * angle.cos(), c.y + r * angle.sin())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edge_is_half_opaque() {
        assert!((profile(0.0, 4.0) - 0.5).abs() < 1e-3);
        assert!(profile(-12.0, 4.0) > 0.99);
        assert!(profile(12.0, 4.0) < 0.01);
        assert!((erf(1.0) - 0.8427).abs() < 1e-3);
    }

    #[test]
    fn rings_have_a_fixed_point_count() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 40.0));
        assert_eq!(rounded_points(rect, 10.0).count(), 4 * (CORNER_STEPS + 1));
        assert_eq!(rounded_points(rect, 0.0).count(), 4 * (CORNER_STEPS + 1));
    }
}
