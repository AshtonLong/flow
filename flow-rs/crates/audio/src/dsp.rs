//! The processing Chromium applied to `getUserMedia({ noiseSuppression: true,
//! autoGainControl: true, echoCancellation: false })` before the AudioContext
//! resampled to 16 kHz, rebuilt natively:
//!
//! 1. down-mix to mono at the device rate;
//! 2. resample to 48 kHz when the device runs at another rate;
//! 3. an 80 Hz high-pass (WebRTC's capture high-pass filter);
//! 4. noise suppression with RNNoise (nnnoiseless), limited to about 15 dB of
//!    attenuation like WebRTC's "high" level, so it never eats quiet speech;
//! 5. a gentle digital automatic gain that lifts quiet microphones towards a
//!    speech level of -20 dBFS (at most +24 dB, never below unity), with a peak
//!    limiter;
//! 6. resample to 16 kHz.
//!
//! The output is aligned with the input: the processing delay is dropped at the
//! start and [`Pipeline::drain`] pushes the tail out, so N seconds of microphone
//! audio become exactly N seconds at 16 kHz.

use nnnoiseless::DenoiseState;
use rubato::{FftFixedIn, Resampler};

use crate::CAPTURE_SAMPLE_RATE;

/// The rate RNNoise works at.
pub const DSP_RATE: u32 = 48_000;
/// RNNoise processes 10 ms frames.
const FRAME: usize = nnnoiseless::FRAME_SIZE;

/// Share of the unprocessed signal mixed back in: a floor of -15 dB on noise
/// suppression.
const NS_FLOOR: f32 = 0.177_827_94;

/// Speech level the gain aims for (RMS, dBFS).
const AGC_TARGET_DB: f32 = -20.0;
/// The most the gain will lift a quiet microphone.
const AGC_MAX_GAIN_DB: f32 = 24.0;
/// How fast the gain may rise and fall while someone speaks, per second.
const AGC_RISE_DB_PER_S: f32 = 10.0;
const AGC_FALL_DB_PER_S: f32 = 20.0;
/// RNNoise's voice probability above which a frame may count as speech.
const AGC_SPEECH_PROB: f32 = 0.8;
/// A speech frame must also stand this far above the noise floor (power ratio,
/// 6 dB): RNNoise's voice detector alone sometimes takes steady noise for speech.
const AGC_SPEECH_OVER_FLOOR: f64 = 4.0;
/// The noise floor is the quietest frame of the last second, kept as the minima
/// of five 200 ms blocks.
const FLOOR_BLOCKS: usize = 5;
const FLOOR_BLOCK_FRAMES: u32 = 20;
/// Peaks are held below this after gain.
const LIMIT_CEILING: f32 = 0.89;

#[derive(Debug, thiserror::Error)]
#[error("unsupported audio format: {0}")]
pub struct PipelineError(String);

/// A fixed-ratio resampler that takes any number of samples at a time.
struct Stage {
    resampler: FftFixedIn<f32>,
    chunk: usize,
    pending: Vec<f32>,
    scratch: Vec<f32>,
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl Stage {
    fn new(from: u32, to: u32) -> Result<Self, PipelineError> {
        if from == 0 || to == 0 {
            return Err(PipelineError(format!("{from} Hz to {to} Hz")));
        }
        // About 10 ms per call, in whole resampler units, so each call yields a
        // fixed number of samples.
        let unit = (from / gcd(from, to)) as usize;
        let chunk = unit * (from as usize / 100).div_ceil(unit).max(1);
        let resampler = FftFixedIn::<f32>::new(from as usize, to as usize, chunk, 1, 1)
            .map_err(|e| PipelineError(e.to_string()))?;
        let scratch = vec![0.0; resampler.output_frames_max()];
        Ok(Self { resampler, chunk, pending: Vec::with_capacity(chunk * 2), scratch })
    }

    /// Output samples of delay.
    fn delay(&self) -> usize {
        self.resampler.output_delay()
    }

    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(input);
        let mut start = 0;
        while self.pending.len() - start >= self.chunk {
            let block = &self.pending[start..start + self.chunk];
            match self.resampler.process_into_buffer(&[block], &mut [&mut self.scratch[..]], None) {
                Ok((_, produced)) => out.extend_from_slice(&self.scratch[..produced]),
                Err(err) => log::warn!(target: "capture", "resampler: {err}"),
            }
            start += self.chunk;
        }
        self.pending.drain(..start);
    }
}

/// Second-order Butterworth high-pass (RBJ cookbook), transposed direct form II.
struct HighPass {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl HighPass {
    fn new(cutoff: f64, rate: f64) -> Self {
        let w0 = std::f64::consts::TAU * cutoff / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = [(1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0, (1.0 + cos) / 2.0 / a0];
        let a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
        Self { b, a, z: [0.0; 2] }
    }

    fn process(&mut self, x: &mut [f32]) {
        for s in x {
            let input = *s as f64;
            let y = self.b[0] * input + self.z[0];
            self.z[0] = self.b[1] * input - self.a[0] * y + self.z[1];
            self.z[1] = self.b[2] * input - self.a[1] * y;
            *s = y as f32;
        }
    }
}

/// Gentle automatic gain: a speech-gated level estimate drives a slowly moving
/// gain, and a fast limiter keeps peaks under the ceiling.
#[derive(Debug, Clone)]
pub struct Agc {
    gain_db: f32,
    speech_power: f64,
    speech_frames: u32,
    /// Minimum frame power of each finished block (minimum statistics).
    floor_blocks: [f64; FLOOR_BLOCKS],
    floor_next: usize,
    /// Minimum of the block being filled, and its length.
    block_min: f64,
    block_frames: u32,
    /// Extra attenuation from the limiter (≤ 1), recovering over ~50 ms.
    limit: f32,
    /// The gain applied at the end of the previous frame, ramped from.
    applied: f32,
}

impl Agc {
    pub fn new(initial_gain_db: f32) -> Self {
        let gain_db = initial_gain_db.clamp(0.0, AGC_MAX_GAIN_DB);
        Self {
            gain_db,
            speech_power: 0.0,
            speech_frames: 0,
            floor_blocks: [f64::INFINITY; FLOOR_BLOCKS],
            floor_next: 0,
            block_min: f64::INFINITY,
            block_frames: 0,
            limit: 1.0,
            applied: db_to_gain(gain_db),
        }
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// The quietest frame power of about the last second (infinite before any
    /// audio arrived).
    fn noise_floor(&self) -> f64 {
        self.floor_blocks.iter().fold(self.block_min, |m, &b| m.min(b))
    }

    fn track_floor(&mut self, raw_power: f64) {
        // Digital silence (a stream starting up) says nothing about the room.
        if raw_power <= 1e-12 {
            return;
        }
        self.block_min = self.block_min.min(raw_power);
        self.block_frames += 1;
        if self.block_frames == FLOOR_BLOCK_FRAMES {
            self.floor_blocks[self.floor_next] = self.block_min;
            self.floor_next = (self.floor_next + 1) % FLOOR_BLOCKS;
            self.block_min = f64::INFINITY;
            self.block_frames = 0;
        }
    }

    /// Processes one 10 ms frame in place. `voice` is RNNoise's speech
    /// probability and `raw_power` the frame's power before noise suppression
    /// (steady there, so the noise floor is measured on it).
    fn process(&mut self, frame: &mut [f32], voice: f32, raw_power: f64) {
        let power = mean_power(frame);
        self.track_floor(raw_power);
        let speaking =
            voice >= AGC_SPEECH_PROB && power > 1e-7 && raw_power > self.noise_floor() * AGC_SPEECH_OVER_FLOOR;
        if speaking {
            self.speech_frames = self.speech_frames.saturating_add(1);
            // A running mean over the first half second of speech, then a 0.5 s average.
            let k = 1.0 / self.speech_frames.min(50) as f64;
            self.speech_power += (power - self.speech_power) * k;
            let level_db = 10.0 * self.speech_power.log10() as f32;
            let desired = (AGC_TARGET_DB - level_db).clamp(0.0, AGC_MAX_GAIN_DB);
            let frame_s = frame.len() as f32 / DSP_RATE as f32;
            let step = if desired > self.gain_db { AGC_RISE_DB_PER_S } else { AGC_FALL_DB_PER_S } * frame_s;
            self.gain_db += (desired - self.gain_db).clamp(-step, step);
        }

        let gain = db_to_gain(self.gain_db);
        let peak = frame.iter().fold(0.0f32, |m, &s| m.max(s.abs())) * gain;
        // Release: back towards no limiting with a ~50 ms time constant.
        self.limit = (self.limit + (1.0 - self.limit) * 0.2).min(1.0);
        if peak * self.limit > LIMIT_CEILING {
            self.limit = LIMIT_CEILING / peak;
        }
        let target = gain * self.limit;
        let n = frame.len() as f32;
        let from = self.applied;
        for (i, s) in frame.iter_mut().enumerate() {
            let g = from + (target - from) * (i + 1) as f32 / n;
            *s = soft_clip(*s * g);
        }
        self.applied = target;
    }
}

fn mean_power(x: &[f32]) -> f64 {
    x.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / x.len().max(1) as f64
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Transparent below the ceiling, then a smooth knee that never exceeds 1.
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= LIMIT_CEILING {
        x
    } else {
        let room = 1.0 - LIMIT_CEILING;
        (LIMIT_CEILING + room * ((a - LIMIT_CEILING) / room).tanh()).copysign(x)
    }
}

/// Device audio in, 16 kHz processed mono out.
pub struct Pipeline {
    rate: u32,
    channels: usize,
    mono: Vec<f32>,
    up: Option<Stage>,
    buf48: Vec<f32>,
    frame: Vec<f32>,
    /// The previous frame after the high-pass, aligned with RNNoise's output
    /// (which lags its input by one frame).
    dry: Vec<f32>,
    hpf: HighPass,
    ns: Box<DenoiseState<'static>>,
    agc: Agc,
    down: Stage,
    out16: Vec<f32>,
    /// Leading samples still to drop (the processing delay).
    skip: usize,
    /// Device frames taken in since the last reset.
    frames_in: u64,
    /// 16 kHz samples handed out since the last reset.
    samples_out: u64,
}

impl Pipeline {
    pub fn new(rate: u32, channels: usize, initial_gain_db: f32) -> Result<Self, PipelineError> {
        if channels == 0 {
            return Err(PipelineError("no channels".into()));
        }
        let up = if rate == DSP_RATE { None } else { Some(Stage::new(rate, DSP_RATE)?) };
        let down = Stage::new(DSP_RATE, CAPTURE_SAMPLE_RATE)?;
        let ratio = DSP_RATE / CAPTURE_SAMPLE_RATE;
        let up_delay = up.as_ref().map_or(0, |s| s.delay());
        let skip = FRAME / ratio as usize + down.delay() + (up_delay + 1) / ratio as usize;
        Ok(Self {
            rate,
            channels,
            mono: Vec::new(),
            up,
            buf48: Vec::with_capacity(FRAME * 2),
            frame: vec![0.0; FRAME],
            dry: vec![0.0; FRAME],
            hpf: HighPass::new(80.0, DSP_RATE as f64),
            ns: DenoiseState::new(),
            agc: Agc::new(initial_gain_db),
            down,
            out16: Vec::new(),
            skip,
            frames_in: 0,
            samples_out: 0,
        })
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn agc(&self) -> &Agc {
        &self.agc
    }

    /// Starts a new recording on the same device: clears every buffer and filter
    /// but keeps the gain the AGC has settled on.
    pub fn reset(&mut self) -> Result<(), PipelineError> {
        let agc = Agc::new(self.agc.gain_db);
        *self = Self::new(self.rate, self.channels, 0.0)?;
        self.agc = agc;
        Ok(())
    }

    /// Processes interleaved device samples; appends 16 kHz output to `out`.
    pub fn process(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        let channels = self.channels;
        let frames = interleaved.len() / channels;
        self.mono.clear();
        if channels == 1 {
            self.mono.extend_from_slice(&interleaved[..frames]);
        } else {
            let scale = 1.0 / channels as f32;
            self.mono.extend(interleaved.chunks_exact(channels).map(|f| f.iter().sum::<f32>() * scale));
        }
        self.frames_in += frames as u64;
        let mono = std::mem::take(&mut self.mono);
        self.process_mono(&mono, out);
        self.mono = mono;
    }

    fn process_mono(&mut self, mono: &[f32], out: &mut Vec<f32>) {
        match self.up.as_mut() {
            Some(up) => up.push(mono, &mut self.buf48),
            None => self.buf48.extend_from_slice(mono),
        }
        let mut used = 0;
        while self.buf48.len() - used >= FRAME {
            self.frame.copy_from_slice(&self.buf48[used..used + FRAME]);
            used += FRAME;
            self.process_frame();
        }
        self.buf48.drain(..used);

        let produced = &self.out16[..];
        let dropped = self.skip.min(produced.len());
        self.skip -= dropped;
        out.extend_from_slice(&produced[dropped..]);
        self.samples_out += (produced.len() - dropped) as u64;
        self.out16.clear();
    }

    fn process_frame(&mut self) {
        self.hpf.process(&mut self.frame);
        // RNNoise expects 16-bit sample values.
        let mut input = [0.0f32; FRAME];
        for (dst, &src) in input.iter_mut().zip(&self.frame) {
            *dst = src * 32768.0;
        }
        let mut output = [0.0f32; FRAME];
        let voice = self.ns.process_frame(&mut output, &input);
        for (o, &dry) in output.iter_mut().zip(&self.dry) {
            *o = *o / 32768.0 * (1.0 - NS_FLOOR) + dry * NS_FLOOR;
        }
        // The power of the input frame this output belongs to (one frame back).
        let raw_power = mean_power(&self.dry);
        self.dry.copy_from_slice(&self.frame);
        self.agc.process(&mut output, voice, raw_power);
        self.down.push(&output, &mut self.out16);
    }

    /// Pushes the audio still inside the filters out: feeds silence until the
    /// output has caught up with every device frame taken in, then stops exactly
    /// there.
    pub fn drain(&mut self, out: &mut Vec<f32>) {
        let expected = self.frames_in * CAPTURE_SAMPLE_RATE as u64 / self.rate as u64;
        if self.samples_out >= expected {
            return;
        }
        let start = out.len();
        let silence = vec![0.0f32; (self.rate as usize / 100).max(1)];
        // The delay is a few tens of milliseconds; the bound is a safety net.
        for _ in 0..50 {
            if self.samples_out >= expected {
                break;
            }
            self.process_mono(&silence, out);
        }
        let excess = self.samples_out.saturating_sub(expected) as usize;
        let keep = (out.len() - start).saturating_sub(excess);
        out.truncate(start + keep);
        self.samples_out = expected;
    }
}

/// Runs a whole mono clip through a fresh pipeline (tests and tools).
pub fn process_clip(mono: &[f32], rate: u32) -> Result<Vec<f32>, PipelineError> {
    let mut pipeline = Pipeline::new(rate, 1, 0.0)?;
    let mut out = Vec::with_capacity(mono.len() * CAPTURE_SAMPLE_RATE as usize / rate as usize + 1600);
    for block in mono.chunks((rate as usize / 100).max(1)) {
        pipeline.process(block, &mut out);
    }
    pipeline.drain(&mut out);
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 16-bit PCM mono WAV reader for the test fixture.
    pub(crate) fn read_wav(path: &std::path::Path) -> (Vec<f32>, u32) {
        let bytes = std::fs::read(path).expect("fixture");
        let mut pos = 12;
        let mut rate = 0;
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
            if id == b"fmt " {
                rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            } else if id == b"data" {
                let pcm = body.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect();
                return (pcm, rate);
            }
            pos += 8 + len + (len & 1);
        }
        panic!("no data chunk");
    }

    pub(crate) fn jfk() -> Vec<f32> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/jfk.wav");
        let (pcm, rate) = read_wav(&path);
        assert_eq!(rate, 16_000);
        pcm
    }

    /// Plain linear interpolation, good enough to make test input at other rates.
    fn to_rate(pcm: &[f32], rate: u32) -> Vec<f32> {
        let n = pcm.len() as u64 * rate as u64 / 16_000;
        (0..n)
            .map(|i| {
                let t = i as f64 * 16_000.0 / rate as f64;
                let i0 = t.floor() as usize;
                let f = (t - i0 as f64) as f32;
                let a = pcm[i0.min(pcm.len() - 1)];
                let b = pcm[(i0 + 1).min(pcm.len() - 1)];
                a + (b - a) * f
            })
            .collect()
    }

    fn rms_db(x: &[f32]) -> f32 {
        let p = x.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64;
        10.0 * (p.max(1e-20)).log10() as f32
    }

    /// Normalised cross-correlation of `a` against `b` shifted by `lag`.
    fn correlation(a: &[f32], b: &[f32], lag: isize) -> f64 {
        let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
        for (i, &x) in a.iter().enumerate() {
            let j = i as isize + lag;
            if j < 0 || j as usize >= b.len() {
                continue;
            }
            let y = b[j as usize] as f64;
            ab += x as f64 * y;
            aa += (x as f64).powi(2);
            bb += y * y;
        }
        ab / (aa.sqrt() * bb.sqrt()).max(1e-12)
    }

    #[test]
    fn output_length_matches_input_duration() {
        let speech = jfk();
        for rate in [16_000, 44_100, 48_000, 96_000, 22_050] {
            let input = if rate == 16_000 { speech.clone() } else { to_rate(&speech, rate) };
            let out = process_clip(&input, rate).unwrap();
            let expected = input.len() as u64 * 16_000 / rate as u64;
            assert_eq!(out.len() as u64, expected, "rate {rate}");
        }
    }

    #[test]
    fn stereo_is_mixed_down() {
        let speech = &jfk()[..16_000];
        let input = to_rate(speech, 48_000);
        let stereo: Vec<f32> = input.iter().flat_map(|&s| [s, s]).collect();
        let mut pipeline = Pipeline::new(48_000, 2, 0.0).unwrap();
        let mut out = Vec::new();
        for block in stereo.chunks(960) {
            pipeline.process(block, &mut out);
        }
        pipeline.drain(&mut out);
        let mono = process_clip(&input, 48_000).unwrap();
        assert_eq!(out.len(), mono.len());
        let diff = out.iter().zip(&mono).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(diff < 1e-4, "max diff {diff}");
    }

    #[test]
    fn output_is_aligned_with_the_input() {
        let speech = jfk();
        for rate in [16_000, 44_100, 48_000] {
            let input = if rate == 16_000 { speech.clone() } else { to_rate(&speech, rate) };
            let out = process_clip(&input, rate).unwrap();
            let best = (-40..=40)
                .max_by(|&a, &b| correlation(&speech, &out, a).total_cmp(&correlation(&speech, &out, b)))
                .unwrap();
            let c = correlation(&speech, &out, best);
            assert!(best.abs() <= 1, "rate {rate}: lag {best} (corr {c:.3})");
            // Noise suppression and gain change the signal, but it stays the same speech.
            assert!(c > 0.8, "rate {rate}: correlation {c:.3}");
        }
    }

    fn noise(seconds: usize, cutoff: Option<f32>, amplitude: f32) -> Vec<f32> {
        let mut seed = 0x1234_5678u32;
        let a = cutoff.map_or(0.0, |c| (-std::f32::consts::TAU * c / 48_000.0).exp());
        let mut lp = 0.0f32;
        (0..48_000 * seconds)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let white = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
                lp = a * lp + (1.0 - a) * white;
                lp * amplitude
            })
            .collect()
    }

    #[test]
    fn steady_noise_is_suppressed() {
        // A low rumble, like a fan or traffic.
        let rumble = noise(3, Some(500.0), 0.1);
        let out = process_clip(&rumble, 48_000).unwrap();
        let before = rms_db(&rumble[48_000..]);
        let after = rms_db(&out[16_000..]);
        assert!(before - after > 10.0, "noise {before:.1} dB -> {after:.1} dB");
    }

    #[test]
    fn steady_noise_is_never_lifted() {
        // RNNoise's voice detector sometimes takes these for speech; the gain must not follow.
        for (cutoff, amplitude) in [(None, 0.017), (Some(2000.0), 0.1), (Some(6000.0), 0.05)] {
            let hiss = noise(4, cutoff, amplitude);
            let out = process_clip(&hiss, 48_000).unwrap();
            let before = rms_db(&hiss[48_000..]);
            let after = rms_db(&out[16_000..]);
            assert!(after <= before + 0.5, "{cutoff:?}: noise {before:.1} dB -> {after:.1} dB");
        }
    }

    #[test]
    fn quiet_speech_is_lifted_and_loud_speech_is_not_clipped() {
        let speech = jfk();
        let quiet: Vec<f32> = speech.iter().map(|s| s * 0.08).collect();
        let out = process_clip(&to_rate(&quiet, 48_000), 48_000).unwrap();
        let lift = rms_db(&out[out.len() / 2..]) - rms_db(&quiet[quiet.len() / 2..]);
        assert!(lift > 6.0, "lift {lift:.1} dB");
        assert!(lift <= AGC_MAX_GAIN_DB + 1.0);

        let loud: Vec<f32> = speech.iter().map(|s| (s * 4.0).clamp(-1.0, 1.0)).collect();
        let out = process_clip(&to_rate(&loud, 48_000), 48_000).unwrap();
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 1.0, "peak {peak}");
        // Never turned down on purpose: the level stays close.
        let change = rms_db(&out) - rms_db(&loud);
        assert!(change > -3.0 && change < 1.0, "change {change:.1} dB");
    }

    #[test]
    fn reset_keeps_the_gain() {
        let mut pipeline = Pipeline::new(48_000, 1, 12.0).unwrap();
        assert_eq!(pipeline.agc().gain_db(), 12.0);
        pipeline.reset().unwrap();
        assert_eq!(pipeline.agc().gain_db(), 12.0);
        assert_eq!(Agc::new(99.0).gain_db(), AGC_MAX_GAIN_DB);
        assert_eq!(Agc::new(-5.0).gain_db(), 0.0);
    }

    #[test]
    fn drain_on_an_empty_pipeline_yields_nothing() {
        let mut pipeline = Pipeline::new(44_100, 1, 0.0).unwrap();
        let mut out = Vec::new();
        pipeline.drain(&mut out);
        assert!(out.is_empty());
        // 5 ms in, 5 ms out (220 frames at 44.1 kHz are 79.8 samples at 16 kHz).
        pipeline.process(&vec![0.1; 220], &mut out);
        pipeline.drain(&mut out);
        assert_eq!(out.len(), 79);
    }

    #[test]
    #[ignore = "diagnostic"]
    fn print_noise_gain() {
        let hiss = noise(4, None, 0.017);
        let mut pipeline = Pipeline::new(48_000, 1, 0.0).unwrap();
        let mut out = Vec::new();
        for (i, block) in hiss.chunks(480).enumerate() {
            pipeline.process(block, &mut out);
            if i % 20 == 0 {
                let a = &pipeline.agc;
                eprintln!(
                    "{i}: gain {:.1} floor {:.1} dB speech {:.1} dB frames {}",
                    a.gain_db,
                    10.0 * a.noise_floor().log10(),
                    10.0 * a.speech_power.max(1e-20).log10(),
                    a.speech_frames
                );
            }
        }
    }

    #[test]
    #[ignore = "diagnostic"]
    fn print_noise_vad() {
        let mut seed = 0x1234_5678u32;
        let mut ns = DenoiseState::new();
        let mut sum_in = 0.0f64;
        let mut sum_out = 0.0f64;
        let mut vads = Vec::new();
        for _ in 0..300 {
            let input: Vec<f32> = (0..FRAME)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.017 * 32768.0
                })
                .collect();
            let mut out = [0.0f32; FRAME];
            let vad = ns.process_frame(&mut out, &input);
            vads.push(vad);
            sum_in += input.iter().map(|&s| (s as f64).powi(2)).sum::<f64>();
            sum_out += out.iter().map(|&s| (s as f64).powi(2)).sum::<f64>();
        }
        eprintln!("ns attenuation {:.1} dB", 10.0 * (sum_out / sum_in).log10());
        eprintln!("vad {:?}", &vads[..60]);
        for cutoff in [500.0f32, 2000.0, 6000.0] {
            let mut ns = DenoiseState::new();
            let a = (-std::f32::consts::TAU * cutoff / 48_000.0).exp();
            let mut lp = 0.0f32;
            let (mut si, mut so) = (0.0f64, 0.0f64);
            let mut vmax = 0.0f32;
            for k in 0..300 {
                let input: Vec<f32> = (0..FRAME)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        let w = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
                        lp = a * lp + (1.0 - a) * w;
                        lp * 0.1 * 32768.0
                    })
                    .collect();
                let mut out = [0.0f32; FRAME];
                let vad = ns.process_frame(&mut out, &input);
                if k > 50 {
                    vmax = vmax.max(vad);
                    si += input.iter().map(|&s| (s as f64).powi(2)).sum::<f64>();
                    so += out.iter().map(|&s| (s as f64).powi(2)).sum::<f64>();
                }
            }
            eprintln!(
                "lowpass {cutoff} Hz noise at {:.1} dBFS: ns attenuation {:.1} dB, max vad {vmax:.2}",
                10.0 * (si / 250.0 / 480.0 / 32768.0f64.powi(2)).log10(),
                10.0 * (so / si).log10()
            );
        }
        let speech = to_rate(&jfk(), 48_000);
        let mut ns = DenoiseState::new();
        let mut v = Vec::new();
        for frame in speech.as_chunks::<FRAME>().0 {
            let input: Vec<f32> = frame.iter().map(|s| s * 32768.0).collect();
            let mut out = [0.0f32; FRAME];
            v.push((ns.process_frame(&mut out, &input) * 100.0) as u8);
        }
        eprintln!("speech vad {:?}", &v[..200]);
    }

    #[test]
    fn processing_is_much_faster_than_real_time() {
        let input = to_rate(&jfk(), 48_000);
        let started = std::time::Instant::now();
        let out = process_clip(&input, 48_000).unwrap();
        let elapsed = started.elapsed().as_secs_f64();
        let audio = out.len() as f64 / 16_000.0;
        eprintln!("processed {audio:.1} s of audio in {:.1} ms ({:.0}x real time)", elapsed * 1000.0, audio / elapsed);
        assert!(audio / elapsed > 10.0);
    }
}
