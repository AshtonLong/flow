//! Cycles the overlay pill through every state on screen (about 30 s), with
//! the waveform driven by `tests/fixtures/jfk.wav` (no microphone), and prints
//! the process's CPU time and memory while the pill is idle and animating.
//!
//! `cargo run -p flow-shell --example overlay_demo [-- --sounds] [--position top-right]`

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use flow_audio::Analyser;
use flow_core::config::{OverlayPosition, OverlaySize, OverlayStyle};
use flow_core::types::{OverlayAppearance, OverlayPhase, OverlayState, ThemeInfo};
use flow_shell::Overlay;

fn read_wav(path: &std::path::Path) -> Vec<f32> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            let end = (pos + 8 + len).min(bytes.len());
            return bytes[pos + 8..end].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b) as f32 / 32768.0).collect();
        }
        pos += 8 + len + (len & 1);
    }
    Vec::new()
}

/// Cycles this process has used (exact, unlike the 15.6 ms tick of
/// GetProcessTimes), and its working set.
#[cfg(windows)]
fn usage() -> (u64, usize) {
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::Win32::System::WindowsProgramming::QueryProcessCycleTime;
    // SAFETY: queries about this process with valid out-pointers.
    unsafe {
        let process = GetCurrentProcess();
        let mut cycles = 0u64;
        let _ = QueryProcessCycleTime(process, &mut cycles);
        let mut mem =
            PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        let _ = K32GetProcessMemoryInfo(process, &mut mem, mem.cb);
        (cycles, mem.WorkingSetSize)
    }
}

#[cfg(not(windows))]
fn usage() -> (u64, usize) {
    (0, 0)
}

/// Cycles per second of one busy core, measured by spinning.
fn cycles_per_second() -> f64 {
    let (c0, _) = usage();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(300) {
        std::hint::spin_loop();
    }
    let (c1, _) = usage();
    (c1 - c0) as f64 / t0.elapsed().as_secs_f64()
}

static RATE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();

/// Waits `wait` and reports the process's CPU use and working set over it.
fn measure(label: &str, wait: Duration) {
    let rate = *RATE.get_or_init(cycles_per_second);
    let (c0, _) = usage();
    let t0 = Instant::now();
    std::thread::sleep(wait);
    let (c1, ws) = usage();
    let cpu = (c1 - c0) as f64 / rate / t0.elapsed().as_secs_f64() * 100.0;
    println!("{label:<46} CPU {cpu:5.2} % of one core, working set {:.1} MB", ws as f64 / 1_048_576.0);
}

/// Tells a watching script which state is about to show.
fn announce(state: &OverlayState) {
    use std::io::Write;
    println!("show {:?}", state.phase);
    let _ = std::io::stdout().flush();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sounds = args.iter().any(|a| a == "--sounds");
    let position = args
        .iter()
        .position(|a| a == "--position")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| OverlayPosition::ALL.into_iter().find(|v| v.as_str() == p))
        .unwrap_or_default();

    let speech = read_wav(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/jfk.wav"));
    let analyser = Arc::new(Analyser::new());
    let feeding = Arc::new(AtomicBool::new(true));
    {
        // Feed the speech in real time, 10 ms at a time, looping.
        let analyser = analyser.clone();
        let feeding = feeding.clone();
        std::thread::spawn(move || {
            let mut pos = 0;
            while feeding.load(Ordering::Relaxed) && !speech.is_empty() {
                let end = (pos + 160).min(speech.len());
                analyser.push(&speech[pos..end]);
                pos = if end >= speech.len() { 0 } else { end };
                std::thread::sleep(Duration::from_millis(10));
            }
        });
    }

    measure("before the overlay (demo's audio feeder only)", Duration::from_secs(3));
    let overlay = Overlay::create();
    // Let the window thread load the font before measuring idle.
    std::thread::sleep(Duration::from_millis(300));
    let appearance = |style, size, high_contrast| OverlayAppearance {
        style,
        size,
        show_waveform: true,
        sounds,
        theme: ThemeInfo { dark: true, high_contrast, ..ThemeInfo::default() },
    };
    overlay.configure(appearance(OverlayStyle::Pill, OverlaySize::Medium, false), position);
    overlay.set_analyser(Some(analyser.clone()));
    measure("created, hidden (idle)", Duration::from_secs(4));

    let state = |phase| OverlayState { phase, ..OverlayState::default() };
    let show = |s: OverlayState, secs: f64| {
        announce(&s);
        overlay.set_state(s);
        std::thread::sleep(Duration::from_secs_f64(secs));
    };

    announce(&state(OverlayPhase::Listening));
    overlay.set_state(state(OverlayPhase::Listening));
    measure("listening (waveform animating)", Duration::from_secs(4));
    show(OverlayState { locked: true, queued: 2, ..state(OverlayPhase::Listening) }, 1.5);
    announce(&state(OverlayPhase::Transcribing));
    overlay.set_state(OverlayState { cloud: true, ..state(OverlayPhase::Transcribing) });
    measure("transcribing (dots animating)", Duration::from_secs(4));
    announce(&state(OverlayPhase::Done));
    overlay.set_state(state(OverlayPhase::Done));
    measure("done (static, visible)", Duration::from_secs(3));
    show(OverlayState::hidden(), 0.8);
    show(
        OverlayState {
            phase: OverlayPhase::Error,
            message: Some("Groq rejected the API key".into()),
            ..OverlayState::default()
        },
        1.6,
    );
    show(
        OverlayState {
            phase: OverlayPhase::Notice,
            message: Some("No text field focused — copied to clipboard".into()),
            ..OverlayState::default()
        },
        1.6,
    );
    show(OverlayState::hidden(), 0.6);
    measure("hidden again (idle)", Duration::from_secs(3));

    for (style, size, contrast, label) in [
        (OverlayStyle::Pill, OverlaySize::Small, false, "small"),
        (OverlayStyle::Pill, OverlaySize::Large, false, "large"),
        (OverlayStyle::Minimal, OverlaySize::Medium, false, "minimal"),
        (OverlayStyle::Pill, OverlaySize::Medium, true, "high contrast"),
    ] {
        println!("{label}");
        overlay.configure(appearance(style, size, contrast), position);
        show(state(OverlayPhase::Listening), 1.6);
        show(OverlayState { cloud: true, ..state(OverlayPhase::Transcribing) }, 1.2);
        show(state(OverlayPhase::Done), 0.8);
        show(OverlayState::hidden(), 0.5);
    }
    // Reduced motion: no entrance, still dots.
    overlay.configure(
        OverlayAppearance {
            theme: ThemeInfo { reduced_motion: true, ..ThemeInfo::default() },
            ..appearance(OverlayStyle::Pill, OverlaySize::Medium, false)
        },
        position,
    );
    println!("reduced motion");
    show(state(OverlayPhase::Transcribing), 1.2);
    show(OverlayState::hidden(), 0.5);
    // style = none never shows.
    overlay.configure(appearance(OverlayStyle::None, OverlaySize::Medium, false), position);
    println!("style none (nothing on screen)");
    show(state(OverlayPhase::Listening), 1.0);
    show(OverlayState::hidden(), 0.3);

    overlay.destroy();
    feeding.store(false, Ordering::Relaxed);
    println!("done");
}
