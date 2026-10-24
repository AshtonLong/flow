//! Voice detection. Silero VAD scores 32 ms frames; the scores are turned into
//! speech segments, and the segments are stitched together so leading silence,
//! trailing silence and long pauses are dropped. Port of the pure parts of
//! `src/engine/vad.ts`; the Silero model itself runs in the engine process.

use crate::types::SAMPLE_RATE;

/// Samples per scored frame (32 ms at 16 kHz).
pub const VAD_FRAME: usize = 512;
/// Samples of the previous frame prefixed to each frame before scoring.
pub const VAD_CONTEXT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentOptions {
    /// Probability at which speech starts.
    pub threshold: f64,
    /// Probability below which speech may end (hysteresis).
    pub neg_threshold: f64,
    /// Silence this long ends a segment.
    pub min_silence_ms: f64,
    /// Segments shorter than this are noise.
    pub min_speech_ms: f64,
    /// Audio kept on both sides of each segment.
    pub pad_ms: f64,
    pub frame_ms: f64,
}

pub const DEFAULT_SEGMENT_OPTIONS: SegmentOptions = SegmentOptions {
    threshold: 0.5,
    neg_threshold: 0.35,
    min_silence_ms: 400.0,
    min_speech_ms: 90.0,
    pad_ms: 200.0,
    frame_ms: VAD_FRAME as f64 / SAMPLE_RATE as f64 * 1000.0,
};

impl Default for SegmentOptions {
    fn default() -> Self {
        DEFAULT_SEGMENT_OPTIONS
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub start_ms: f64,
    pub end_ms: f64,
}

/// Turns per-frame speech probabilities into padded, merged speech segments.
pub fn segments_from_probs(probs: &[f32], total_ms: f64, o: &SegmentOptions) -> Vec<Segment> {
    let mut raw: Vec<Segment> = Vec::new();
    let mut start = -1.0_f64;
    let mut silence_start = -1.0_f64;
    for (i, &p) in probs.iter().enumerate() {
        let p = f64::from(p);
        let t = i as f64 * o.frame_ms;
        if start < 0.0 {
            if p >= o.threshold {
                start = t;
                silence_start = -1.0;
            }
            continue;
        }
        if p >= o.threshold {
            silence_start = -1.0;
        } else if p < o.neg_threshold {
            if silence_start < 0.0 {
                silence_start = t;
            }
            if t + o.frame_ms - silence_start >= o.min_silence_ms {
                raw.push(Segment { start_ms: start, end_ms: silence_start });
                start = -1.0;
                silence_start = -1.0;
            }
        }
    }
    if start >= 0.0 {
        raw.push(Segment { start_ms: start, end_ms: if silence_start >= 0.0 { silence_start } else { total_ms } });
    }

    let padded = raw.into_iter().filter(|s| s.end_ms - s.start_ms >= o.min_speech_ms).map(|s| Segment {
        start_ms: (s.start_ms - o.pad_ms).max(0.0),
        end_ms: (s.end_ms + o.pad_ms).min(total_ms),
    });

    let mut merged: Vec<Segment> = Vec::new();
    for seg in padded {
        match merged.last_mut() {
            Some(last) if seg.start_ms <= last.end_ms => last.end_ms = last.end_ms.max(seg.end_ms),
            _ => merged.push(seg),
        }
    }
    merged
}

/// Concatenates the audio of the given segments.
pub fn extract_segments(audio: &[f32], segments: &[Segment]) -> Vec<f32> {
    let rate = f64::from(SAMPLE_RATE);
    let ranges: Vec<(usize, usize)> = segments
        .iter()
        .map(|s| {
            let from = js_round(s.start_ms / 1000.0 * rate).max(0.0) as usize;
            let to = (js_round(s.end_ms / 1000.0 * rate).max(0.0) as usize).min(audio.len());
            // A segment past the end of the audio contributes nothing.
            let from = from.min(audio.len());
            (from, to.max(from))
        })
        .collect();
    let total = ranges.iter().map(|(from, to)| to - from).sum();
    let mut out = Vec::with_capacity(total);
    for (from, to) in ranges {
        out.extend_from_slice(&audio[from..to]);
    }
    out
}

/// Fallback detector for when the Silero model cannot run: frame energy against
/// an adaptive noise floor. Returns pseudo-probabilities on the Silero frame grid.
pub fn energy_probs(audio: &[f32]) -> Vec<f32> {
    let frames = audio.len() / VAD_FRAME;
    let rms: Vec<f32> = audio
        .as_chunks::<VAD_FRAME>()
        .0
        .iter()
        .map(|frame| {
            let sum: f64 = frame.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
            (sum / VAD_FRAME as f64).sqrt() as f32
        })
        .collect();
    if frames == 0 {
        return rms;
    }
    // Noise floor: the 10th percentile frame, with an absolute minimum.
    let mut sorted = rms.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = f64::from(sorted[frames / 10]).max(0.0015);
    let speech_level = (floor * 3.5).max(0.008);
    rms.iter().map(|&r| (f64::from(r) / (speech_level * 2.0)).min(1.0) as f32).collect()
}

/// `Math.round`: halves round towards positive infinity.
fn js_round(x: f64) -> f64 {
    let r = x.round();
    if x - r == 0.5 { r + 1.0 } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_MS: f64 = DEFAULT_SEGMENT_OPTIONS.frame_ms;

    /// Builds a probability track from `(duration_ms, probability)` runs.
    fn track(runs: &[(f64, f32)]) -> (Vec<f32>, f64) {
        let mut probs = Vec::new();
        for &(ms, p) in runs {
            for _ in 0..js_round(ms / FRAME_MS) as usize {
                probs.push(p);
            }
        }
        let total_ms = probs.len() as f64 * FRAME_MS;
        (probs, total_ms)
    }

    fn segments(probs: &[f32], total_ms: f64) -> Vec<Segment> {
        segments_from_probs(probs, total_ms, &SegmentOptions::default())
    }

    // segmentsFromProbs

    #[test]
    fn returns_nothing_for_silence() {
        let (probs, total_ms) = track(&[(3000.0, 0.02)]);
        assert_eq!(segments(&probs, total_ms), vec![]);
    }

    #[test]
    fn trims_leading_and_trailing_silence_keeping_padding() {
        let (probs, total_ms) = track(&[(1000.0, 0.01), (2000.0, 0.95), (1500.0, 0.01)]);
        let segs = segments(&probs, total_ms);
        assert_eq!(segs.len(), 1);
        let seg = segs[0];
        assert!(seg.start_ms > 700.0);
        assert!(seg.start_ms <= 1000.0);
        assert!(seg.end_ms >= 3000.0);
        assert!(seg.end_ms < 3400.0);
    }

    #[test]
    fn drops_a_long_pause_between_two_utterances() {
        let (probs, total_ms) = track(&[(1000.0, 0.9), (3000.0, 0.01), (1000.0, 0.9)]);
        let segs = segments(&probs, total_ms);
        assert_eq!(segs.len(), 2);
        let kept: f64 = segs.iter().map(|s| s.end_ms - s.start_ms).sum();
        assert!(kept < 3000.0);
        assert!(kept >= 2000.0);
    }

    #[test]
    fn keeps_a_short_pause_inside_one_segment() {
        let (probs, total_ms) = track(&[(800.0, 0.9), (200.0, 0.05), (800.0, 0.9)]);
        assert_eq!(segments(&probs, total_ms).len(), 1);
    }

    #[test]
    fn ignores_a_click_shorter_than_the_minimum_speech_length() {
        let (probs, total_ms) = track(&[(1000.0, 0.01), (FRAME_MS, 0.9), (1000.0, 0.01)]);
        assert_eq!(segments(&probs, total_ms), vec![]);
    }

    #[test]
    fn keeps_speech_that_runs_to_the_end_of_the_recording() {
        let (probs, total_ms) = track(&[(500.0, 0.01), (1000.0, 0.9)]);
        let segs = segments(&probs, total_ms);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].end_ms, total_ms);
    }

    #[test]
    fn uses_hysteresis_so_a_dip_between_thresholds_does_not_end_speech() {
        let (probs, total_ms) = track(&[(600.0, 0.9), (600.0, 0.42), (600.0, 0.9)]);
        assert_eq!(segments(&probs, total_ms).len(), 1);
    }

    // extractSegments

    #[test]
    fn concatenates_the_audio_of_each_segment() {
        let audio: Vec<f32> = (0..16000).map(|i| i as f32).collect();
        let out = extract_segments(
            &audio,
            &[Segment { start_ms: 0.0, end_ms: 100.0 }, Segment { start_ms: 500.0, end_ms: 600.0 }],
        );
        assert_eq!(out.len(), 3200);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1600], 8000.0);
    }

    #[test]
    fn clamps_segments_to_the_audio_length() {
        let audio = vec![0.0_f32; 1600];
        assert_eq!(extract_segments(&audio, &[Segment { start_ms: 50.0, end_ms: 5000.0 }]).len(), 800);
    }

    #[test]
    fn ignores_a_segment_past_the_end_of_the_audio() {
        let audio = vec![0.0_f32; 1600];
        assert!(extract_segments(&audio, &[Segment { start_ms: 500.0, end_ms: 900.0 }]).is_empty());
    }

    // energyProbs

    #[test]
    fn scores_loud_frames_above_quiet_ones() {
        let audio: Vec<f32> = (0..VAD_FRAME * 40)
            .map(|i| {
                let loud = (VAD_FRAME * 15..VAD_FRAME * 25).contains(&i);
                ((i as f64 / 5.0).sin() * if loud { 0.3 } else { 0.001 }) as f32
            })
            .collect();
        let probs = energy_probs(&audio);
        assert_eq!(probs.len(), 40);
        assert!(probs[20] > 0.8);
        assert!(probs[5] < 0.3);
        let segs = segments(&probs, 40.0 * FRAME_MS);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn finds_no_speech_in_a_silent_recording() {
        let probs = energy_probs(&vec![0.0_f32; VAD_FRAME * 30]);
        assert_eq!(segments(&probs, 30.0 * FRAME_MS), vec![]);
    }

    #[test]
    fn handles_audio_shorter_than_one_frame() {
        assert!(energy_probs(&[0.5; 100]).is_empty());
    }

    #[test]
    fn rounds_halves_up_like_javascript() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(62.5), 63.0);
        assert_eq!(js_round(6.25), 6.0);
    }
}
