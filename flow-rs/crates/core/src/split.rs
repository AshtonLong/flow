//! Splits long recordings at pauses so each piece fits a model's window.
//! Port of `src/main/transcribe/split.ts`.

use std::ops::Range;

use crate::types::SAMPLE_RATE;

/// Energy is measured in 10 ms frames.
const FRAME_SECONDS: f64 = 0.01;
/// A cut goes in the middle of the quietest stretch this long.
const PAUSE_SECONDS: f64 = 0.3;
/// Avoid leaving a final piece shorter than this.
const MIN_TAIL_SECONDS: f64 = 1.0;
/// Piece length when `max_seconds` is not a positive number.
const DEFAULT_MAX_SECONDS: f64 = 30.0;

/// `Math.round`: halves round towards positive infinity.
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// Splits long audio at the quietest points so each piece is ≤ `max_seconds`
/// (30 when it is not positive; `sample_rate` 0 means 16 kHz).
/// Pieces concatenate back to the input exactly; they are views of it, not copies.
pub fn split_at_pauses(audio: &[f32], max_seconds: f64, sample_rate: u32) -> Vec<&[f32]> {
    split_ranges(audio, max_seconds, sample_rate).into_iter().map(|range| &audio[range]).collect()
}

/// [`split_at_pauses`] as index ranges into `audio`.
pub fn split_ranges(audio: &[f32], max_seconds: f64, sample_rate: u32) -> Vec<Range<usize>> {
    let sample_rate = if sample_rate > 0 { sample_rate } else { SAMPLE_RATE };
    let rate = f64::from(sample_rate);
    // NaN is not > 0 either, so it falls back too.
    let max_seconds = if max_seconds > 0.0 { max_seconds } else { DEFAULT_MAX_SECONDS };
    let max_samples = ((max_seconds * rate).floor() as usize).max(1);
    let len = audio.len();
    if len <= max_samples {
        // The whole input as one piece.
        return std::iter::once(0..len).collect();
    }

    let frame = (js_round(FRAME_SECONDS * rate) as usize).max(1);
    let frame_count = len.div_ceil(frame);
    // prefix[i] is the energy of frames 0..i-1, so any window sum is one subtraction.
    let mut prefix = vec![0.0f64; frame_count + 1];
    for f in 0..frame_count {
        let end = len.min((f + 1) * frame);
        let energy: f64 = audio[f * frame..end].iter().map(|&s| f64::from(s) * f64::from(s)).sum();
        prefix[f + 1] = prefix[f] + energy;
    }
    let window = (js_round(PAUSE_SECONDS / FRAME_SECONDS) as usize).max(1);
    let min_tail = ((MIN_TAIL_SECONDS * rate).floor() as usize).min(max_samples / 2);

    let mut pieces = Vec::new();
    let mut start = 0;
    while len - start > max_samples {
        // Search the second half of the allowed span, so pieces stay reasonably long.
        let low = start + max_samples.div_ceil(2);
        let mut high = start + max_samples;
        if len - high < min_tail {
            high = low.max(len - min_tail);
        }
        let cut = quietest_point(&prefix, frame, window, low, high);
        pieces.push(start..cut);
        start = cut;
    }
    pieces.push(start..len);
    pieces
}

/// Sample index in [low, high] at the centre of the quietest window; the latest one on a tie.
fn quietest_point(prefix: &[f64], frame: usize, window: usize, low: usize, high: usize) -> usize {
    let frames = prefix.len() - 1;
    let half = window / 2;
    let first_frame = low.div_ceil(frame);
    let last_frame = high / frame;
    let mut best: Option<usize> = None;
    let mut best_energy = f64::INFINITY;
    for f in first_frame..=last_frame {
        let from = f.saturating_sub(half);
        let to = frames.min(f + (window - half));
        if to <= from {
            // An empty window: 0/0 in the TS, which never compares as quieter.
            continue;
        }
        let energy = (prefix[to] - prefix[from]) / (to - from) as f64;
        if energy <= best_energy {
            best_energy = energy;
            best = Some(f);
        }
    }
    match best {
        // No frame boundary inside the range (a tiny max_seconds): cut at the limit.
        None => high,
        Some(best) => high.min(low.max(best * frame)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16000;

    /// Loud noise with silent gaps at the given [start_seconds, end_seconds] ranges.
    fn speech(seconds: f64, pauses: &[(f64, f64)], rate: u32) -> Vec<f32> {
        let rate = f64::from(rate);
        let len = js_round(seconds * rate) as usize;
        let mut audio = vec![0.0f32; len];
        let mut seed: u64 = 1;
        for sample in audio.iter_mut() {
            seed = (seed.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7fff_ffff;
            *sample = ((seed as f64 / 0x7fff_ffff as f64 - 0.5) * 0.8) as f32;
        }
        for &(from, to) in pauses {
            let from = js_round(from * rate) as usize;
            let to = js_round(to * rate) as usize;
            audio[from..to].fill(0.0);
        }
        audio
    }

    /// True if the pieces, end to end, are exactly `audio`, as views of it.
    fn rebuilds(pieces: &[&[f32]], audio: &[f32]) -> bool {
        let mut offset = 0;
        for piece in pieces {
            if piece.as_ptr() != audio[offset..].as_ptr() {
                return false;
            }
            offset += piece.len();
        }
        offset == audio.len()
    }

    #[test]
    fn returns_short_audio_as_one_piece() {
        let audio = speech(12.0, &[], RATE);
        let pieces = split_at_pauses(&audio, 30.0, RATE);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].as_ptr(), audio.as_ptr());
        assert_eq!(pieces[0].len(), audio.len());
    }

    #[test]
    fn does_not_split_audio_exactly_at_the_limit() {
        assert_eq!(split_at_pauses(&speech(30.0, &[], RATE), 30.0, RATE).len(), 1);
    }

    #[test]
    fn returns_empty_audio_as_one_empty_piece() {
        let pieces = split_at_pauses(&[], 30.0, RATE);
        assert_eq!(pieces.len(), 1);
        assert!(pieces[0].is_empty());
    }

    #[test]
    fn cuts_inside_the_pause() {
        let audio = speech(45.0, &[(22.0, 22.6)], RATE);
        let pieces = split_at_pauses(&audio, 30.0, RATE);
        assert_eq!(pieces.len(), 2);
        let cut = pieces[0].len() as f64 / f64::from(RATE);
        assert!(cut > 22.0 && cut < 22.6, "{cut}");
    }

    #[test]
    fn picks_the_quietest_pause_within_reach_not_the_first() {
        let mut audio = speech(45.0, &[(26.0, 26.5)], RATE);
        // A quieter-but-not-silent stretch earlier on.
        for sample in &mut audio[18 * RATE as usize..(18.5 * f64::from(RATE)) as usize] {
            *sample *= 0.3;
        }
        let cut = split_at_pauses(&audio, 30.0, RATE)[0].len() as f64 / f64::from(RATE);
        assert!(cut > 26.0 && cut < 26.5, "{cut}");
    }

    #[test]
    fn keeps_every_piece_within_the_limit_and_loses_nothing() {
        let cases: Vec<(&str, Vec<f32>, f64)> = vec![
            (
                "long speech with pauses",
                speech(100.0, &[(20.0, 20.4), (47.0, 47.5), (70.0, 70.3), (95.0, 95.4)], RATE),
                30.0,
            ),
            ("speech with no pause at all", speech(95.0, &[], RATE), 30.0),
            ("pure silence", vec![0.0; 100 * RATE as usize], 30.0),
            ("a shorter limit", speech(31.0, &[(7.0, 7.5)], RATE), 10.0),
            ("a length just over the limit", speech(30.01, &[], RATE), 30.0),
        ];
        for (name, audio, max_seconds) in cases {
            let pieces = split_at_pauses(&audio, max_seconds, RATE);
            assert!(pieces.len() > 1, "{name}");
            for piece in &pieces {
                assert!(!piece.is_empty(), "{name}");
                assert!(piece.len() as f64 <= max_seconds * f64::from(RATE), "{name}");
            }
            assert!(rebuilds(&pieces, &audio), "{name}");
        }
    }

    #[test]
    fn returns_views_of_the_input_rather_than_copies() {
        let audio = speech(40.0, &[(25.0, 25.5)], RATE);
        let range = audio.as_ptr_range();
        for piece in split_at_pauses(&audio, 30.0, RATE) {
            assert!(range.start <= piece.as_ptr() && piece.as_ptr_range().end <= range.end);
        }
    }

    #[test]
    fn avoids_a_sliver_at_the_end() {
        let audio = speech(30.2, &[], RATE);
        let pieces = split_at_pauses(&audio, 30.0, RATE);
        assert_eq!(pieces.len(), 2);
        assert!(pieces[1].len() >= RATE as usize);
    }

    #[test]
    fn honours_the_sample_rate() {
        let audio = speech(45.0, &[(22.0, 22.6)], 8000);
        let pieces = split_at_pauses(&audio, 30.0, 8000);
        assert_eq!(pieces.len(), 2);
        let cut = pieces[0].len() as f64 / 8000.0;
        assert!(cut > 22.0 && cut < 22.6, "{cut}");
    }

    #[test]
    fn falls_back_to_defaults_for_nonsense_options() {
        let audio = speech(45.0, &[], RATE);
        for (max_seconds, sample_rate) in [(0.0, 0), (-5.0, 0), (f64::NAN, 0)] {
            for piece in split_at_pauses(&audio, max_seconds, sample_rate) {
                assert!(piece.len() <= 30 * RATE as usize);
            }
        }
    }

    #[test]
    fn ranges_match_the_slices() {
        let audio = speech(65.0, &[(22.0, 22.6), (50.0, 50.5)], RATE);
        let ranges = split_ranges(&audio, 30.0, RATE);
        let pieces = split_at_pauses(&audio, 30.0, RATE);
        assert_eq!(ranges.len(), pieces.len());
        for (range, piece) in ranges.iter().zip(&pieces) {
            assert_eq!(audio[range.clone()].as_ptr(), piece.as_ptr());
            assert_eq!(range.len(), piece.len());
        }
    }
}
