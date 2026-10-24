//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The repository root (`flow-rs/..`).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// `tests/fixtures/jfk.wav`: 11 s of speech, 16 kHz mono.
pub fn jfk() -> Vec<f32> {
    read_wav(&repo_root().join("tests/fixtures/jfk.wav"))
}

/// Reads a 16-bit PCM mono WAV (walking the RIFF chunks; jfk.wav has a LIST chunk).
pub fn read_wav(path: &Path) -> Vec<f32> {
    let data = std::fs::read(path).expect("read wav");
    assert_eq!(&data[0..4], b"RIFF");
    assert_eq!(&data[8..12], b"WAVE");
    let mut off = 12;
    let mut format = None;
    while off + 8 <= data.len() {
        let id = &data[off..off + 4];
        let size = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
        let body = &data[off + 8..off + 8 + size];
        match id {
            b"fmt " => {
                let tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes([body[14], body[15]]);
                format = Some((tag, channels, rate, bits));
            }
            b"data" => {
                assert_eq!(format, Some((1, 1, 16_000, 16)), "expected 16 kHz mono 16-bit PCM");
                return body.as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0).collect();
            }
            _ => {}
        }
        off += 8 + size + (size & 1);
    }
    panic!("no data chunk in {}", path.display());
}
