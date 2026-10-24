//! The pure-Rust Silero against what the Electron engine computed with
//! onnxruntime on the same model file (`fixtures/jfk_silero_ts.json`).

mod common;

use flow_core::types::samples_to_ms;
use flow_core::vad::{DEFAULT_SEGMENT_OPTIONS, Segment, extract_segments, segments_from_probs};
use flow_engine::silero::SileroVad;

fn reference() -> Vec<f32> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jfk_silero_ts.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["probs"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect()
}

#[test]
fn matches_the_onnxruntime_scores_on_jfk() {
    let audio = common::jfk();
    let expected = reference();
    let probs = SileroVad::new().unwrap().probabilities(&audio);
    assert_eq!(probs.len(), expected.len());
    let max_diff = probs.iter().zip(&expected).map(|(a, b)| (a - b).abs()).fold(0.0_f32, f32::max);
    eprintln!("{} frames, max difference from onnxruntime {max_diff:e}", probs.len());
    assert!(max_diff < 1e-4, "max difference {max_diff}");
}

#[test]
fn trims_jfk_like_the_electron_engine() {
    let audio = common::jfk();
    let total_ms = samples_to_ms(audio.len());
    let probs = SileroVad::new().unwrap().probabilities(&audio);
    let segments = segments_from_probs(&probs, total_ms, &DEFAULT_SEGMENT_OPTIONS);
    let expected = segments_from_probs(&reference(), total_ms, &DEFAULT_SEGMENT_OPTIONS);
    assert_eq!(segments, expected);
    // Speech starts after ~0.35 s of silence; the pauses between phrases go.
    let ms = |s: f64, e: f64| Segment { start_ms: s, end_ms: e };
    assert_eq!(
        segments,
        [ms(152.0, 2440.0), ms(3096.0, 4616.0), ms(5208.0, 7848.0), ms(7992.0, 11_000.0)]
    );
    assert_eq!(samples_to_ms(extract_segments(&audio, &segments).len()), 9456.0);
}

#[test]
fn state_resets_between_recordings() {
    let audio = common::jfk();
    let mut vad = SileroVad::new().unwrap();
    let first = vad.probabilities(&audio[..48_000]);
    vad.probabilities(&audio[48_000..]);
    assert_eq!(vad.probabilities(&audio[..48_000]), first);
}
