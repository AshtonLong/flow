//! CSS transitions as egui animations. Durations and timing functions come
//! from `styles.css`; under reduced motion every duration is zero (the
//! `[data-motion='reduced']` rule), so nothing animates and nothing repaints.

use egui::{Color32, Id, Ui};

/// Solves `cubic-bezier(x1, y1, x2, y2)` for the progress `x` in 0..1.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let bez = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    // Bisection on x(t): monotonic for valid CSS curves.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut t = x;
    for _ in 0..24 {
        let value = bez(x1, x2, t);
        if (value - x).abs() < 1e-5 {
            break;
        }
        if value < x {
            lo = t;
        } else {
            hi = t;
        }
        t = (lo + hi) / 2.0;
    }
    bez(y1, y2, t)
}

/// `cubic-bezier(0.16, 1, 0.3, 1)`: the switch thumb and the key press.
pub fn snappy(t: f32) -> f32 {
    cubic_bezier(0.16, 1.0, 0.3, 1.0, t)
}

/// CSS `ease`, the default timing function.
pub fn ease(t: f32) -> f32 {
    cubic_bezier(0.25, 0.1, 0.25, 1.0, t)
}

/// CSS `ease-in-out`.
pub fn ease_in_out(t: f32) -> f32 {
    cubic_bezier(0.42, 0.0, 0.58, 1.0, t)
}

pub fn linear(t: f32) -> f32 {
    t
}

/// Whether animations run (`reduced_motion` turns them off).
pub fn enabled(ui: &Ui) -> bool {
    !super::tokens(ui.ctx()).reduced_motion
}

/// 0 → 1 as `value` turns on, over `ms` milliseconds with `easing`.
pub fn toggle(ui: &Ui, id: Id, value: bool, ms: f32, easing: fn(f32) -> f32) -> f32 {
    let time = if enabled(ui) { ms / 1000.0 } else { 0.0 };
    ui.ctx()
        .animate_bool_with_time_and_easing(id, value, time, easing)
}

/// A number that glides to `target` over `ms` (linear).
pub fn value(ui: &Ui, id: Id, target: f32, ms: f32) -> f32 {
    let time = if enabled(ui) { ms / 1000.0 } else { 0.0 };
    ui.ctx().animate_value_with_time(id, target, time)
}

/// Mixes two colours in gamma space, as CSS transitions do for `background-color`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, ba] = b.to_array();
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(lerp(ar, br), lerp(ag, bg), lerp(ab, bb), lerp(aa, ba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_hit_their_ends_and_overshoot_early() {
        assert_eq!(snappy(0.0), 0.0);
        assert_eq!(snappy(1.0), 1.0);
        // The thumb's curve is most of the way there a quarter of the way in.
        assert!(snappy(0.25) > 0.75);
        assert!((ease(0.5) - 0.8024).abs() < 0.01);
        assert!((ease_in_out(0.5) - 0.5).abs() < 0.01);
    }
}
