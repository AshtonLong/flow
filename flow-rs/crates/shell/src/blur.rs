//! Gaussian blur of an alpha mask, for `box-shadow`. Three box blurs in each
//! direction approximate the Gaussian closely (as Skia does for large sigmas).

/// Box widths whose three passes approximate a Gaussian of `sigma`.
fn boxes(sigma: f32) -> [usize; 3] {
    let n = 3.0f32;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let l = lower as f32;
    let m = ((12.0 * sigma * sigma - n * l * l - 4.0 * n * l - 3.0 * n) / (-4.0 * l - 4.0)).round();
    let mut out = [0usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        *o = if (i as f32) < m { lower as usize } else { upper as usize };
    }
    out
}

/// One running-sum box blur of radius `r` along rows (`stride` 1) or columns.
fn box_pass(src: &[f32], dst: &mut [f32], len: usize, lines: usize, step: usize, line_step: usize, r: usize) {
    let scale = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let base = line * line_step;
        let at = |i: isize| -> f32 {
            if i < 0 || i as usize >= len { 0.0 } else { src[base + i as usize * step] }
        };
        let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for i in 0..len {
            dst[base + i * step] = sum * scale;
            sum += at(i as isize + r as isize + 1) - at(i as isize - r as isize);
        }
    }
}

/// Blurs `mask` (`width` × `height`, values 0..1) in place.
pub fn gaussian(mask: &mut [f32], width: usize, height: usize, sigma: f32) {
    if sigma < 0.3 || width == 0 || height == 0 {
        return;
    }
    let mut tmp = vec![0.0f32; mask.len()];
    for b in boxes(sigma) {
        let r = (b - 1) / 2;
        box_pass(mask, &mut tmp, width, height, 1, width, r);
        box_pass(&tmp, mask, height, width, width, 1, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_mass_and_spreads_like_a_gaussian() {
        let (w, h) = (121, 1);
        let mut m = vec![0.0f32; w * h];
        m[60] = 1.0;
        // A 1-D impulse: only the horizontal pass matters when h == 1... use a tall strip instead.
        let (w2, h2) = (121, 121);
        let mut strip = vec![0.0f32; w2 * h2];
        for y in 0..h2 {
            strip[y * w2 + 60] = 1.0;
        }
        gaussian(&mut strip, w2, h2, 8.0);
        let row: Vec<f32> = strip[60 * w2..61 * w2].to_vec();
        let total: f32 = row.iter().sum();
        assert!((total - 1.0).abs() < 0.02, "mass {total}");
        let var: f32 = row.iter().enumerate().map(|(i, v)| v * (i as f32 - 60.0).powi(2)).sum::<f32>() / total;
        assert!((var.sqrt() - 8.0).abs() < 0.3, "sigma {}", var.sqrt());
        gaussian(&mut m, w, h, 0.1);
        assert_eq!(m[60], 1.0);
    }
}
