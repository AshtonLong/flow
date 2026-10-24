//! Renders the pill in every state offscreen and writes PNGs (transparent, the
//! full 460 × 84 window at the given scale).
//!
//! `cargo run -p flow-shell --example render_states -- [out-dir] [scale]`
//! (default `target/overlay-renders`, scale 1).

use flow_core::config::{OverlaySize, OverlayStyle};
use flow_core::types::{OverlayAppearance, OverlayPhase, OverlayState, ThemeInfo};
use flow_shell::render_pill;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().unwrap_or_else(|| "target/overlay-renders".into()));
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create {}: {err}", dir.display());
        std::process::exit(1);
    }
    let base = OverlayAppearance {
        style: OverlayStyle::Pill,
        size: OverlaySize::Medium,
        show_waveform: true,
        sounds: false,
        theme: ThemeInfo { dark: true, ..ThemeInfo::default() },
    };
    let speaking = [0.40, 0.34, 0.30, 0.18, 0.04, 0.0, 0.0];
    let state = |phase| OverlayState { phase, ..OverlayState::default() };
    let error = OverlayState {
        phase: OverlayPhase::Error,
        message: Some("Groq rejected the API key".into()),
        ..OverlayState::default()
    };
    let notice = OverlayState {
        phase: OverlayPhase::Notice,
        message: Some("No text field focused — copied to clipboard".into()),
        ..OverlayState::default()
    };
    let transcribing = OverlayState { cloud: true, ..state(OverlayPhase::Transcribing) };
    let locked = OverlayState { locked: true, queued: 2, cloud: true, ..state(OverlayPhase::Listening) };
    let small = OverlayAppearance { size: OverlaySize::Small, ..base.clone() };
    let large = OverlayAppearance { size: OverlaySize::Large, ..base.clone() };
    let minimal = OverlayAppearance { style: OverlayStyle::Minimal, ..base.clone() };
    let contrast = OverlayAppearance { theme: ThemeInfo { high_contrast: true, ..base.theme.clone() }, ..base.clone() };
    let long = OverlayState {
        phase: OverlayPhase::Error,
        message: Some("The model file is damaged and could not be loaded — download it again from Settings".into()),
        ..OverlayState::default()
    };

    let listening = state(OverlayPhase::Listening);
    let done = state(OverlayPhase::Done);
    let renders: Vec<(&str, &OverlayState, &OverlayAppearance, f32, &[f32])> = vec![
        ("listening", &listening, &base, 0.0, &speaking),
        ("transcribing", &transcribing, &base, 1.17, &[]),
        ("done", &done, &base, 0.0, &[]),
        ("error", &error, &base, 0.0, &[]),
        ("notice", &notice, &base, 0.0, &[]),
        ("listening-locked-queued", &locked, &base, 0.0, &speaking),
        ("listening-quiet", &listening, &base, 0.0, &[]),
        ("small-done", &done, &small, 0.0, &[]),
        ("large-listening", &listening, &large, 0.0, &speaking),
        ("minimal-listening", &listening, &minimal, 0.0, &speaking),
        ("minimal-transcribing", &transcribing, &minimal, 1.17, &[]),
        ("minimal-error", &error, &minimal, 0.0, &[]),
        ("contrast-listening", &listening, &contrast, 0.0, &speaking),
        ("contrast-done", &done, &contrast, 0.0, &[]),
        ("ellipsis", &long, &base, 0.0, &[]),
    ];
    for (name, s, a, time, levels) in renders {
        let pixmap = render_pill(s, a, scale, time, levels);
        let path = dir.join(format!("{name}.png"));
        match pixmap.save_png(&path) {
            Ok(()) => println!("{}", path.display()),
            Err(err) => eprintln!("{}: {err}", path.display()),
        }
    }
}
