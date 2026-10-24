//! The start and stop sounds (`blip` and `playTransition` in overlay/main.ts):
//! a sine whose pitch glides exponentially, with an 8 ms fade-in and an
//! exponential fade-out. Synthesised once into an in-memory WAV and played
//! asynchronously; sounds are a nicety and never block or fail loudly.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use flow_core::types::OverlayPhase;

const RATE: u32 = 48_000;

/// One blip as samples: Web Audio's oscillator, frequency ramp and gain envelope.
pub fn synthesize(from: f32, to: f32, ms: f32, volume: f32) -> Vec<f32> {
    let rate = RATE as f64;
    let (from, to) = (from.max(1.0) as f64, to.max(1.0) as f64);
    let ramp = (ms as f64 / 1000.0).max(0.009);
    let volume = volume as f64;
    // osc.stop(t + ms / 1000 + 0.02)
    let n = ((ramp + 0.02) * rate).ceil() as usize;
    let mut phase = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / rate;
        // exponentialRampToValueAtTime(to, t + ms / 1000)
        let freq = if t < ramp { from * (to / from).powf(t / ramp) } else { to };
        // setValueAtTime(0) → linearRamp(volume, 8 ms) → exponentialRamp(0.0001, ms)
        let gain = if t < 0.008 {
            volume * t / 0.008
        } else if t < ramp {
            volume * (0.0001 / volume).powf((t - 0.008) / (ramp - 0.008))
        } else {
            0.0001
        };
        out.push((phase.sin() * gain) as f32);
        phase = (phase + std::f64::consts::TAU * freq / rate) % std::f64::consts::TAU;
    }
    out
}

/// 16-bit mono PCM WAV.
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

/// Synthesised sounds by parameters. Playback reads the buffer asynchronously,
/// so each lives for the rest of the process (there are three).
static CACHE: LazyLock<Mutex<HashMap<[u32; 4], &'static [u8]>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Plays a blip gliding from `from` Hz to `to` Hz over `ms`, at `volume`
/// (0.05 in the TS default).
pub fn blip(from: f32, to: f32, ms: f32, volume: f32) {
    let key = [from.to_bits(), to.to_bits(), ms.to_bits(), volume.to_bits()];
    let sound = {
        let Ok(mut cache) = CACHE.lock() else { return };
        *cache.entry(key).or_insert_with(|| Box::leak(wav(&synthesize(from, to, ms, volume)).into_boxed_slice()))
    };
    play(sound);
}

/// The sound for a phase change, if any: up for listening, down for
/// transcribing, low for an error.
pub fn transition_sound(previous: OverlayPhase, next: OverlayPhase) -> Option<(f32, f32, f32, f32)> {
    if next == OverlayPhase::Listening && previous != OverlayPhase::Listening {
        Some((520.0, 780.0, 90.0, 0.05))
    } else if previous == OverlayPhase::Listening && next == OverlayPhase::Transcribing {
        Some((700.0, 480.0, 90.0, 0.05))
    } else if next == OverlayPhase::Error {
        Some((300.0, 200.0, 180.0, 0.06))
    } else {
        None
    }
}

/// `playTransition`
pub fn play_transition(previous: OverlayPhase, next: OverlayPhase) {
    if let Some((from, to, ms, volume)) = transition_sound(previous, next) {
        blip(from, to, ms, volume);
    }
}

#[cfg(windows)]
fn play(sound: &'static [u8]) {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::core::PCWSTR;
    // SAFETY: with SND_MEMORY the "name" is the WAV image, which is 'static.
    let ok = unsafe { PlaySoundW(PCWSTR(sound.as_ptr().cast()), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
    if !ok.as_bool() {
        log::debug!(target: "overlay", "sound did not play");
    }
}

#[cfg(not(windows))]
fn play(_sound: &'static [u8]) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blip_follows_the_envelope() {
        let s = synthesize(520.0, 780.0, 90.0, 0.05);
        assert_eq!(s.len(), (0.11f64 * 48_000.0).ceil() as usize);
        let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 0.05 + 1e-6 && peak > 0.04, "peak {peak}");
        // Silent at the start, fully faded by the end of the ramp.
        assert_eq!(s[0], 0.0);
        assert!(s[(0.095 * 48_000.0) as usize..].iter().all(|v| v.abs() <= 0.0001 + 1e-7));
    }

    #[test]
    fn pitch_glides_up_for_listening() {
        let s = synthesize(520.0, 780.0, 90.0, 0.05);
        // Zero crossings early vs late: the later part is higher.
        let crossings = |a: &[f32]| a.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let early = crossings(&s[480..1920]);
        let late = crossings(&s[2400..3840]);
        assert!(late > early, "{early} vs {late}");
    }

    #[test]
    fn transitions_match_main_ts() {
        use OverlayPhase::*;
        assert_eq!(transition_sound(Hidden, Listening), Some((520.0, 780.0, 90.0, 0.05)));
        assert_eq!(transition_sound(Listening, Listening), None);
        assert_eq!(transition_sound(Listening, Transcribing), Some((700.0, 480.0, 90.0, 0.05)));
        assert_eq!(transition_sound(Hidden, Transcribing), None);
        assert_eq!(transition_sound(Transcribing, Error), Some((300.0, 200.0, 180.0, 0.06)));
        assert_eq!(transition_sound(Listening, Done), None);
    }

    #[test]
    fn wav_header_is_valid() {
        let w = wav(&[0.0, 0.5, -0.5]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(w.len(), 44 + 6);
    }
}
