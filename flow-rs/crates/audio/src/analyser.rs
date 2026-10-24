//! A stand-in for the Web Audio `AnalyserNode` the overlay's waveform and the
//! settings level meter read: `fftSize` 512 on the 16 kHz stream,
//! `smoothingTimeConstant` 0.6, `minDecibels` -100, `maxDecibels` -30. The math
//! follows Chromium's `RealtimeAnalyser` so the bars move as they did.
//!
//! The capture thread feeds it ([`Analyser::push`]); any thread may read it. Both
//! sides take a short uncontended lock.

use std::sync::Arc;

use parking_lot::Mutex;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// `analyser.fftSize`.
pub const FFT_SIZE: usize = 512;
/// `analyser.frequencyBinCount`.
pub const FREQUENCY_BIN_COUNT: usize = FFT_SIZE / 2;

const SMOOTHING_TIME_CONSTANT: f64 = 0.6;
const MIN_DECIBELS: f64 = -100.0;
const MAX_DECIBELS: f64 = -30.0;

/// The most recent `FFT_SIZE` samples.
struct Ring {
    buf: [f32; FFT_SIZE],
    /// Next write position.
    pos: usize,
    /// Samples written since the last reset; tells readers whether time moved.
    written: u64,
}

struct Spectrum {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Smoothed magnitudes, kept between calls like the node's internal state.
    magnitudes: Vec<f64>,
    /// `written` at the last analysis; a call with no new audio reuses the result.
    analysed_at: Option<u64>,
}

pub struct Analyser {
    ring: Mutex<Ring>,
    spectrum: Mutex<Spectrum>,
}

impl Default for Analyser {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Analyser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyser").field("written", &self.ring.lock().written).finish()
    }
}

impl Analyser {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        // Chromium's Blackman window: alpha 0.16, over N (not N - 1).
        let window = (0..FFT_SIZE)
            .map(|i| {
                let x = i as f64 / FFT_SIZE as f64;
                let (a0, a1, a2) = (0.42, 0.5, 0.08);
                (a0 - a1 * (std::f64::consts::TAU * x).cos() + a2 * (2.0 * std::f64::consts::TAU * x).cos()) as f32
            })
            .collect();
        let input = fft.make_input_vec();
        let output = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        Self {
            ring: Mutex::new(Ring { buf: [0.0; FFT_SIZE], pos: 0, written: 0 }),
            spectrum: Mutex::new(Spectrum {
                fft,
                window,
                input,
                output,
                scratch,
                magnitudes: vec![0.0; FREQUENCY_BIN_COUNT],
                analysed_at: None,
            }),
        }
    }

    /// Appends 16 kHz samples (called by the capture thread).
    pub fn push(&self, samples: &[f32]) {
        let mut ring = self.ring.lock();
        let tail = if samples.len() > FFT_SIZE { &samples[samples.len() - FFT_SIZE..] } else { samples };
        for &s in tail {
            let pos = ring.pos;
            ring.buf[pos] = s;
            ring.pos = (pos + 1) % FFT_SIZE;
        }
        ring.written += samples.len() as u64;
    }

    /// Forgets all audio and smoothing state, as a freshly created node.
    pub fn reset(&self) {
        {
            let mut ring = self.ring.lock();
            ring.buf = [0.0; FFT_SIZE];
            ring.pos = 0;
            ring.written = 0;
        }
        let mut spectrum = self.spectrum.lock();
        spectrum.magnitudes.iter_mut().for_each(|m| *m = 0.0);
        spectrum.analysed_at = None;
    }

    /// `getFloatTimeDomainData`: the most recent samples, oldest first. Fills
    /// up to `FFT_SIZE` values.
    pub fn float_time_domain_data(&self, out: &mut [f32]) {
        let ring = self.ring.lock();
        let n = out.len().min(FFT_SIZE);
        let start = (ring.pos + FFT_SIZE - n) % FFT_SIZE;
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = ring.buf[(start + i) % FFT_SIZE];
        }
    }

    /// The level the settings meter shows: RMS of the time-domain window times
    /// four, at most 1 (capture.ts `tick`).
    pub fn level(&self) -> f32 {
        let mut buf = [0.0f32; FFT_SIZE];
        self.float_time_domain_data(&mut buf);
        let sum: f32 = buf.iter().map(|v| v * v).sum();
        ((sum / FFT_SIZE as f32).sqrt() * 4.0).min(1.0)
    }

    /// `getByteFrequencyData`: smoothed magnitudes in dB mapped from
    /// [-100, -30] to [0, 255]. Fills up to `FREQUENCY_BIN_COUNT` values.
    pub fn byte_frequency_data(&self, out: &mut [u8]) {
        let range_scale = 1.0 / (MAX_DECIBELS - MIN_DECIBELS);
        self.with_magnitudes(|mags| {
            for (o, &m) in out.iter_mut().zip(mags) {
                let db = linear_to_decibels(m);
                let scaled = 255.0 * (db - MIN_DECIBELS) * range_scale;
                *o = scaled.clamp(0.0, 255.0) as u8;
            }
        });
    }

    /// `getFloatFrequencyData`: smoothed magnitudes in dB.
    pub fn float_frequency_data(&self, out: &mut [f32]) {
        self.with_magnitudes(|mags| {
            for (o, &m) in out.iter_mut().zip(mags) {
                *o = linear_to_decibels(m) as f32;
            }
        });
    }

    /// Runs the analysis when new audio arrived since the previous call (a call
    /// within the same "render quantum" returns the previous data, as in
    /// Chromium), then hands out the smoothed magnitudes.
    fn with_magnitudes(&self, f: impl FnOnce(&[f64])) {
        let mut spectrum = self.spectrum.lock();
        let written = {
            let ring = self.ring.lock();
            if spectrum.analysed_at != Some(ring.written) {
                // Oldest first.
                for i in 0..FFT_SIZE {
                    spectrum.input[i] = ring.buf[(ring.pos + i) % FFT_SIZE];
                }
            }
            ring.written
        };
        if spectrum.analysed_at != Some(written) {
            spectrum.analysed_at = Some(written);
            let s = &mut *spectrum;
            for (x, w) in s.input.iter_mut().zip(&s.window) {
                *x *= w;
            }
            if s.fft.process_with_scratch(&mut s.input, &mut s.output, &mut s.scratch).is_ok() {
                // Chromium's FFT frames are scaled by 2 (to match vecLib), then
                // normalised by 1/N; the packed Nyquist term is dropped.
                let scale = 2.0 / FFT_SIZE as f64;
                let k = SMOOTHING_TIME_CONSTANT;
                for (i, m) in s.magnitudes.iter_mut().enumerate() {
                    let c = s.output[i];
                    let im = if i == 0 { 0.0 } else { c.im as f64 };
                    let magnitude = (c.re as f64).hypot(im) * scale;
                    let next = k * *m + (1.0 - k) * magnitude;
                    *m = if next.is_finite() { next } else { 0.0 };
                }
            }
        }
        f(&spectrum.magnitudes);
    }
}

fn linear_to_decibels(v: f64) -> f64 {
    // Chromium returns -1000 for silence instead of -Infinity.
    if v > 0.0 { 20.0 * v.log10() } else { -1000.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / 16_000.0).sin()).collect()
    }

    #[test]
    fn silence_reads_as_zero() {
        let a = Analyser::new();
        let mut bytes = [7u8; FREQUENCY_BIN_COUNT];
        a.byte_frequency_data(&mut bytes);
        assert!(bytes.iter().all(|&b| b == 0));
        assert_eq!(a.level(), 0.0);
    }

    #[test]
    fn a_tone_peaks_in_its_bin() {
        let a = Analyser::new();
        // 1 kHz at 16 kHz with N = 512 lands in bin 32.
        a.push(&sine(1000.0, 0.5, 2048));
        let mut db = [0f32; FREQUENCY_BIN_COUNT];
        a.float_frequency_data(&mut db);
        let peak = db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
        assert_eq!(peak, Some(32));
        // Chromium's scaling: amplitude * 0.42 (Blackman mean), smoothed once by 0.4.
        let expected = 20.0 * (0.5f32 * 0.42 * 0.4).log10();
        assert!((db[32] - expected).abs() < 0.2, "{} vs {expected}", db[32]);
        // Loud bins saturate at 255; far from the tone the window keeps leakage low.
        let mut bytes = [0u8; FREQUENCY_BIN_COUNT];
        a.byte_frequency_data(&mut bytes);
        assert_eq!(bytes[32], 255);
        assert!(bytes[100] < 60, "leak {}", bytes[100]);
    }

    #[test]
    fn smoothing_applies_once_per_new_audio() {
        let a = Analyser::new();
        a.push(&sine(500.0, 0.5, 512));
        let mut first = [0f32; FREQUENCY_BIN_COUNT];
        a.float_frequency_data(&mut first);
        // Same audio, no new samples: identical result (no second smoothing step).
        let mut again = [0f32; FREQUENCY_BIN_COUNT];
        a.float_frequency_data(&mut again);
        assert_eq!(first, again);
        // New audio: the smoothed value moves towards the steady state.
        a.push(&sine(500.0, 0.5, 512));
        let mut third = [0f32; FREQUENCY_BIN_COUNT];
        a.float_frequency_data(&mut third);
        assert!(third[16] > first[16]);
        // First analysis from zero state: (1 - 0.6) of the magnitude, i.e. about -8 dB lower.
        let steady = {
            let b = Analyser::new();
            b.push(&sine(500.0, 0.5, 512));
            for _ in 0..60 {
                b.push(&sine(500.0, 0.5, 512));
                let mut tmp = [0f32; FREQUENCY_BIN_COUNT];
                b.float_frequency_data(&mut tmp);
            }
            let mut tmp = [0f32; FREQUENCY_BIN_COUNT];
            b.float_frequency_data(&mut tmp);
            tmp[16]
        };
        assert!((steady - first[16] - 7.96).abs() < 0.1, "steady {steady} first {}", first[16]);
    }

    #[test]
    fn level_is_rms_times_four() {
        let a = Analyser::new();
        a.push(&vec![0.1; FFT_SIZE]);
        assert!((a.level() - 0.4).abs() < 1e-5);
        a.push(&vec![0.5; FFT_SIZE]);
        assert_eq!(a.level(), 1.0);
        a.reset();
        assert_eq!(a.level(), 0.0);
    }

    #[test]
    fn time_domain_data_is_oldest_first() {
        let a = Analyser::new();
        let ramp: Vec<f32> = (0..600).map(|i| i as f32).collect();
        a.push(&ramp[..300]);
        a.push(&ramp[300..]);
        let mut out = [0f32; FFT_SIZE];
        a.float_time_domain_data(&mut out);
        assert_eq!(out[0], 88.0);
        assert_eq!(out[FFT_SIZE - 1], 599.0);
        let mut last4 = [0f32; 4];
        a.float_time_domain_data(&mut last4);
        assert_eq!(last4, [596.0, 597.0, 598.0, 599.0]);
    }
}
