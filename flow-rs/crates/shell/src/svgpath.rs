//! Just enough SVG path syntax for the overlay's icons (index.html): M, L, H,
//! V, A and Z, absolute and relative. Arcs become cubic Béziers.

use tiny_skia::{Path, PathBuilder};

/// The icons, in their 24 × 24 viewBox.
pub mod icons {
    /// Audio is leaving the PC.
    pub const CLOUD: &str = "M7 18a4 4 0 0 1-.6-7.96A5.5 5.5 0 0 1 17 9.5a4.25 4.25 0 0 1 .25 8.5z";
    pub const OK: &str = "M5 12.5l4.5 4.5L19 7.5";
    pub const BANG: &str = "M12 6v7M12 17.5v.5";
    pub const LOCK: &str = "M7 11V8a5 5 0 0 1 10 0v3M6 11h12v9H6z";
}

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn skip_separators(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
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

    fn at_number(&mut self) -> bool {
        self.skip_separators();
        self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }

    fn number(&mut self) -> Option<f32> {
        self.skip_separators();
        let start = self.i;
        let s = self.s;
        if matches!(s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut dot = false;
        while let Some(&c) = s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        if matches!(s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(s.get(self.i), Some(b'-' | b'+')) {
                self.i += 1;
            }
            while s.get(self.i).is_some_and(u8::is_ascii_digit) {
                self.i += 1;
            }
        }
        std::str::from_utf8(&s[start..self.i]).ok()?.parse().ok()
    }

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

/// Parses `d` into a path. Returns `None` for malformed data.
pub fn parse(d: &str) -> Option<Path> {
    let mut t = Tokens { s: d.as_bytes(), i: 0 };
    let mut pb = PathBuilder::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    let mut cmd = t.command()?;
    loop {
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                x = ox + t.number()?;
                y = oy + t.number()?;
                pb.move_to(x, y);
                (sx, sy) = (x, y);
                // Further pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
                if !t.at_number() {
                    cmd = t.command().unwrap_or(0);
                }
                if cmd == 0 {
                    break;
                }
                continue;
            }
            b'L' => {
                x = ox + t.number()?;
                y = oy + t.number()?;
                pb.line_to(x, y);
            }
            b'H' => {
                x = ox + t.number()?;
                pb.line_to(x, y);
            }
            b'V' => {
                y = oy + t.number()?;
                pb.line_to(x, y);
            }
            b'A' => {
                let rx = t.number()?;
                let ry = t.number()?;
                let rotation = t.number()?;
                let large = t.flag()?;
                let sweep = t.flag()?;
                let ex = ox + t.number()?;
                let ey = oy + t.number()?;
                arc_to(&mut pb, (x, y), (rx, ry), rotation, large, sweep, (ex, ey));
                (x, y) = (ex, ey);
            }
            b'Z' => {
                pb.close();
                (x, y) = (sx, sy);
            }
            _ => return None,
        }
        if !cmd.eq_ignore_ascii_case(&b'Z') && t.at_number() {
            continue;
        }
        match t.command() {
            Some(next) => cmd = next,
            None => break,
        }
    }
    pb.finish()
}

/// SVG endpoint arc to cubic Béziers (SVG 1.1 implementation notes, F.6.5).
fn arc_to(
    pb: &mut PathBuilder,
    from: (f32, f32),
    radii: (f32, f32),
    rotation_deg: f32,
    large: bool,
    sweep: bool,
    to: (f32, f32),
) {
    let (x1, y1) = (from.0 as f64, from.1 as f64);
    let (x2, y2) = (to.0 as f64, to.1 as f64);
    let (mut rx, mut ry) = (radii.0.abs() as f64, radii.1.abs() as f64);
    if (x1 - x2).abs() < 1e-9 && (y1 - y2).abs() < 1e-9 {
        return;
    }
    if rx < 1e-9 || ry < 1e-9 {
        pb.line_to(to.0, to.1);
        return;
    }
    let phi = (rotation_deg as f64).to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos_phi * dx + sin_phi * dy;
    let y1p = -sin_phi * dx + cos_phi * dy;
    // Scale radii up if they cannot reach.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos_phi * cxp - sin_phi * cyp + (x1 + x2) / 2.0;
    let cy = sin_phi * cxp + cos_phi * cyp + (y1 + y2) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        if a.is_nan() { 0.0 } else { a }
    };
    let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    let segments = (delta.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / segments as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f64| {
        let (s, c) = t.sin_cos();
        (cx + rx * c * cos_phi - ry * s * sin_phi, cy + rx * c * sin_phi + ry * s * cos_phi)
    };
    let deriv = |t: f64| {
        let (s, c) = t.sin_cos();
        (-rx * s * cos_phi - ry * c * sin_phi, -rx * s * sin_phi + ry * c * cos_phi)
    };
    let mut t = theta1;
    for i in 0..segments {
        let t2 = t + step;
        let p1 = point(t);
        let d1 = deriv(t);
        let p2 = if i + 1 == segments { (x2, y2) } else { point(t2) };
        let d2 = deriv(t2);
        pb.cubic_to(
            (p1.0 + k * d1.0) as f32,
            (p1.1 + k * d1.1) as f32,
            (p2.0 - k * d2.0) as f32,
            (p2.1 - k * d2.1) as f32,
            p2.0 as f32,
            p2.1 as f32,
        );
        t = t2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_icon() {
        for d in [icons::CLOUD, icons::OK, icons::BANG, icons::LOCK] {
            let path = parse(d).unwrap_or_else(|| panic!("{d}"));
            let b = path.compute_tight_bounds().unwrap();
            assert!(b.left() >= 0.0 && b.right() <= 24.0 && b.top() >= 0.0 && b.bottom() <= 24.0, "{d}: {b:?}");
        }
    }

    #[test]
    fn cloud_spans_its_drawing() {
        let b = parse(icons::CLOUD).unwrap().compute_tight_bounds().unwrap();
        // The big arc (r 5.5 from 6.4,10.04 to 17,9.5) is centred near 11.7,11.2.
        assert!((b.top() - 5.71).abs() < 0.02, "{b:?}");
        assert!((b.left() - 2.96).abs() < 0.02, "{b:?}");
        assert!((b.right() - 21.38).abs() < 0.02, "{b:?}");
        assert!((b.bottom() - 18.0).abs() < 0.01, "{b:?}");
    }

    #[test]
    fn lock_has_its_shackle_and_body() {
        let b = parse(icons::LOCK).unwrap().compute_tight_bounds().unwrap();
        assert_eq!((b.left(), b.top(), b.right(), b.bottom()), (6.0, 3.0, 18.0, 20.0));
    }
}
