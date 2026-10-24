//! Capture tests. Those marked `#[ignore]` open the real default microphone for
//! a moment (no audio is written anywhere); run them with
//! `cargo test -p flow-audio --test capture -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use flow_audio::{CaptureEvent, ClipRecorder, DictationCapture, LevelMonitor, list_microphones};
use flow_core::types::{CaptureErrorReason, CaptureStart};

fn capture() -> (DictationCapture, Receiver<(Instant, CaptureEvent)>) {
    let (tx, rx) = unbounded();
    let capture = DictationCapture::new(Arc::new(move |event| {
        let _ = tx.send((Instant::now(), event));
    }));
    (capture, rx)
}

fn next(rx: &Receiver<(Instant, CaptureEvent)>) -> (Instant, CaptureEvent) {
    rx.recv_timeout(Duration::from_secs(5)).expect("capture event")
}

fn start(device: &str, keep_warm: bool) -> CaptureStart {
    CaptureStart { device_id: device.into(), keep_warm }
}

#[test]
fn stop_without_a_recording_still_reports_stopped() {
    let (capture, rx) = capture();
    capture.stop();
    assert_eq!(next(&rx).1, CaptureEvent::Stopped);
}

#[test]
fn a_missing_microphone_is_reported_by_name() {
    let (capture, rx) = capture();
    capture.start(&start("Microphone (Not Plugged In) (dead:beef)", false));
    match next(&rx).1 {
        CaptureEvent::Error { reason, message } => {
            assert_eq!(reason, CaptureErrorReason::DeviceMissing);
            assert_eq!(message, "Microphone \"Microphone (Not Plugged In) (dead:beef)\" is not connected");
        }
        other => panic!("unexpected {other:?}"),
    }
    // The failed start left nothing recording.
    capture.stop();
    assert_eq!(next(&rx).1, CaptureEvent::Stopped);
}

/// Records for `hold`, returns (start→Started, start→first chunk, samples).
fn record_once(
    capture: &DictationCapture,
    rx: &Receiver<(Instant, CaptureEvent)>,
    opts: &CaptureStart,
    hold: Duration,
) -> (Duration, Duration, usize) {
    let began = Instant::now();
    capture.start(opts);
    let (at, first) = next(rx);
    assert_eq!(first, CaptureEvent::Started, "first event");
    let started = at - began;
    let mut first_chunk = None;
    let mut samples = 0;
    let stop_at = began + hold;
    loop {
        let wait = stop_at.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok((at, CaptureEvent::Chunk(pcm))) => {
                first_chunk.get_or_insert(at - began);
                assert!(pcm.len() <= 1600);
                samples += pcm.len();
            }
            Ok((_, other)) => panic!("unexpected {other:?}"),
            Err(_) => break,
        }
    }
    capture.stop();
    loop {
        match next(rx) {
            (_, CaptureEvent::Chunk(pcm)) => samples += pcm.len(),
            (_, CaptureEvent::Stopped) => break,
            (_, other) => panic!("unexpected {other:?}"),
        }
    }
    (started, first_chunk.unwrap_or_default(), samples)
}

#[test]
#[ignore = "opens the real default microphone"]
fn dictation_on_the_default_microphone() {
    let (capture, rx) = capture();
    let opts = start("default", true);
    let hold = Duration::from_millis(700);
    let (started, first, samples) = record_once(&capture, &rx, &opts, hold);
    eprintln!("cold: Started after {started:?}, first chunk after {first:?}, {samples} samples");
    // The recording covers the time between audio flowing and the stop.
    let expected = (hold - started).as_secs_f64() * 16_000.0;
    assert!((samples as f64 - expected).abs() < 3_200.0, "{samples} samples, expected about {expected:.0}");

    for round in 0..3 {
        let (started, first, samples) = record_once(&capture, &rx, &opts, hold);
        eprintln!("warm {round}: Started after {started:?}, first chunk after {first:?}, {samples} samples");
        assert!(started < Duration::from_millis(100), "warm start took {started:?}");
    }

    // A device change drops the warm stream: the next start opens the device again.
    capture.device_changed();
    let (started, first, _) = record_once(&capture, &rx, &opts, Duration::from_millis(400));
    eprintln!("after device change: Started after {started:?}, first chunk after {first:?}");

    // Without keep-warm every start is cold.
    let cold = start("default", false);
    for round in 0..2 {
        let (started, first, _) = record_once(&capture, &rx, &cold, Duration::from_millis(400));
        eprintln!("cold {round}: Started after {started:?}, first chunk after {first:?}");
    }
}

#[test]
#[ignore = "opens the real microphones"]
fn each_listed_microphone_opens_by_label() {
    let (capture, rx) = capture();
    for mic in list_microphones() {
        let (started, first, samples) =
            record_once(&capture, &rx, &start(&mic.label, false), Duration::from_millis(400));
        eprintln!("{}: Started after {started:?}, first chunk after {first:?}, {samples} samples", mic.label);
        // The Electron spelling, with a USB suffix, opens the same device.
        let legacy = format!("{} (1234:abcd)", mic.label);
        let (started, ..) = record_once(&capture, &rx, &start(&legacy, false), Duration::from_millis(300));
        eprintln!("  as {legacy:?}: Started after {started:?}");
    }
}

#[test]
#[ignore = "opens the real default microphone"]
fn quick_release_while_opening_still_stops_cleanly() {
    let (capture, rx) = capture();
    capture.start(&start("default", true));
    capture.stop();
    // Stopped comes at once; the late open is kept warm, not announced.
    assert_eq!(next(&rx).1, CaptureEvent::Stopped);
    assert!(rx.recv_timeout(Duration::from_millis(600)).is_err(), "nothing after Stopped");
    // The warm stream from the stale open serves the next start.
    let (started, _, _) = record_once(&capture, &rx, &start("default", true), Duration::from_millis(300));
    eprintln!("start after a stale open: Started after {started:?}");
}

#[test]
#[ignore = "opens the real default microphone"]
fn level_monitor_and_clip_recorder() {
    let (tx, rx) = unbounded();
    let monitor = LevelMonitor::open(
        "default",
        Arc::new(move |level| {
            let _ = tx.send(level);
        }),
        Arc::new(|| {}),
    )
    .expect("monitor");
    std::thread::sleep(Duration::from_millis(500));
    drop(monitor);
    let levels: Vec<f32> = rx.try_iter().collect();
    eprintln!("{} level reports in 500 ms", levels.len());
    assert!(levels.len() >= 20 && levels.len() <= 40, "{} reports", levels.len());
    assert!(levels.iter().all(|l| (0.0..=1.0).contains(l)));

    let recorder = ClipRecorder::start("default", None, None).expect("recorder");
    std::thread::sleep(Duration::from_millis(600));
    let clip = recorder.stop();
    eprintln!("clip of {} samples ({:.0} ms)", clip.len(), clip.len() as f64 / 16.0);
    assert!(clip.len() > 6_400 && clip.len() < 11_000, "{} samples", clip.len());
}
