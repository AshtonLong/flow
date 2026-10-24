//! WAV encoding for cloud uploads. Port of `src/main/transcribe/wav.ts`.

const HEADER_BYTES: usize = 44;

/// `Math.round`: halves round towards positive infinity (`-2.5` → `-2`).
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// 16-bit PCM mono WAV. Pass [`SAMPLE_RATE`](crate::types::SAMPLE_RATE) for Flow's own audio.
pub fn encode_wav(pcm: &[f32], sample_rate: u32) -> Vec<u8> {
    // The header fields are 32-bit; like the DataView writes in the TS, they wrap.
    let data_bytes = (pcm.len() as u32).wrapping_mul(2);
    let mut out = Vec::with_capacity(HEADER_BYTES + pcm.len() * 2);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&36u32.wrapping_add(data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&sample_rate.wrapping_mul(2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());

    for &sample in pcm {
        let sample = f64::from(sample);
        // NaN fails both comparisons and becomes silence.
        let clamped = if sample >= 1.0 {
            1.0
        } else if sample <= -1.0 {
            -1.0
        } else if sample > -1.0 {
            sample
        } else {
            0.0
        };
        let scaled = js_round(if clamped < 0.0 { clamped * 32768.0 } else { clamped * 32767.0 });
        // Always within i16 after the clamp.
        out.extend_from_slice(&(scaled as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SAMPLE_RATE;

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
    }
    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }
    fn i16_at(bytes: &[u8], offset: usize) -> i16 {
        i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }

    #[test]
    fn writes_a_16_bit_pcm_mono_header_at_16_khz_by_default() {
        let wav = encode_wav(&[0.0; 160], SAMPLE_RATE);
        assert_eq!(wav.len(), 44 + 320);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32_at(&wav, 4), 36 + 320);
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32_at(&wav, 16), 16);
        assert_eq!(u16_at(&wav, 20), 1); // PCM
        assert_eq!(u16_at(&wav, 22), 1); // mono
        assert_eq!(u32_at(&wav, 24), 16000);
        assert_eq!(u32_at(&wav, 28), 32000);
        assert_eq!(u16_at(&wav, 32), 2);
        assert_eq!(u16_at(&wav, 34), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32_at(&wav, 40), 320);
    }

    #[test]
    fn honours_another_sample_rate() {
        let wav = encode_wav(&[0.0; 10], 48000);
        assert_eq!(u32_at(&wav, 24), 48000);
        assert_eq!(u32_at(&wav, 28), 96000);
    }

    #[test]
    fn scales_clamps_and_silences_bad_samples() {
        let wav = encode_wav(&[0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, f32::NAN, f32::INFINITY], SAMPLE_RATE);
        let samples: Vec<i16> = (0..9).map(|i| i16_at(&wav, 44 + i * 2)).collect();
        assert_eq!(samples, [0, 32767, -32768, 16384, -16384, 32767, -32768, 0, 32767]);
    }

    #[test]
    fn encodes_empty_audio_as_a_bare_header() {
        let wav = encode_wav(&[], SAMPLE_RATE);
        assert_eq!(wav.len(), 44);
        assert_eq!(u32_at(&wav, 40), 0);
    }

    #[test]
    fn rounds_halves_like_math_round() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(-2.6), -3.0);
        // -1/65536 scales to exactly -0.5, which Math.round takes to 0.
        let wav = encode_wav(&[-1.0 / 65536.0, f32::NEG_INFINITY], SAMPLE_RATE);
        assert_eq!(i16_at(&wav, 44), 0);
        assert_eq!(i16_at(&wav, 46), -32768);
    }
}
