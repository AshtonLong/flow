//! `EngineClient` driving a real engine process (`flow-engine-host`). None of
//! these need the native speech library: voice detection is pure Rust.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flow_core::protocol::engine::VadKind;
use flow_engine::{EngineClient, EngineError, EngineOptions};

fn options() -> EngineOptions {
    EngineOptions {
        exe: PathBuf::from(env!("CARGO_BIN_EXE_flow-engine-host")),
        args: vec!["--engine".into()],
        // Never pick up a library from the environment or the target dir.
        library_path: Some(PathBuf::from("C:\\flow-test\\missing\\transcribe.dll")),
        cuda_runtime_dir: None,
        debug: false,
    }
}

async fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Stops one of our own engine processes, by PID.
fn kill(pid: u32) {
    let status = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output().unwrap();
    assert!(status.status.success(), "{status:?}");
}

/// Ninety seconds of noise: keeps voice detection busy for a few seconds in a
/// debug build (well past the 1.5 s idle delay).
fn long_noise() -> Vec<f32> {
    let mut seed = 0x1234_5678_u32;
    (0..16_000 * 90)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1 << 24) as f32 - 0.5) * 0.2
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn trims_jfk_through_the_engine_process() {
    let client = EngineClient::new(options());
    assert!(!client.running());
    let audio = common::jfk();
    let started = Instant::now();
    let trimmed = client.trim(&audio).await.unwrap();
    eprintln!("trim (including engine start): {:?}", started.elapsed());
    assert_eq!(trimmed.detector, VadKind::Silero);
    assert_eq!(trimmed.speech_ms, 9456.0);
    let speech = trimmed.audio.unwrap();
    assert_eq!(speech.len(), 151_296);
    // The first kept sample is 152 ms into the recording.
    assert_eq!(speech[..100], audio[2432..2532]);
    assert!(client.running());
    assert!(client.pid().is_some());

    let silence = client.trim(&vec![0.0; 16_000]).await.unwrap();
    assert_eq!(silence.audio, None);
    assert_eq!(silence.speech_ms, 0.0);
    client.shutdown();
    assert!(!client.running());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_probe_without_the_library_says_why_and_the_engine_keeps_serving() {
    let client = EngineClient::new(options());
    let err = client.probe().await.unwrap_err();
    assert_eq!(
        err,
        EngineError::new("FLOW_TRANSCRIBE_LIBRARY points at a missing file: C:\\flow-test\\missing\\transcribe.dll")
    );
    assert!(client.devices().is_empty());
    assert!(client.trim(&common::jfk()).await.unwrap().audio.is_some());
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn exits_right_after_use_when_models_should_not_stay_loaded() {
    let client = EngineClient::new(options());
    client.set_keep_loaded_minutes(0.0);
    client.trim(&[0.0; 1600]).await.unwrap();
    assert!(client.running());
    let idle_from = Instant::now();
    wait_until("the engine exits", Duration::from_secs(5), || !client.running()).await;
    let waited = idle_from.elapsed();
    eprintln!("idle exit after {waited:?}");
    assert!(waited >= Duration::from_millis(1400), "{waited:?}");
    // It starts again on demand.
    client.trim(&[0.0; 1600]).await.unwrap();
    assert!(client.running());
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn honours_fractional_minutes_and_never() {
    let client = EngineClient::new(options());
    client.set_keep_loaded_minutes(-1.0);
    client.trim(&[0.0; 1600]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(client.running(), "-1 never unloads");
    // 0.04 minutes = 2.4 s, counted from now.
    client.set_keep_loaded_minutes(0.04);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(client.running());
    wait_until("the engine exits", Duration::from_secs(5), || !client.running()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_exit_while_a_request_is_pending() {
    let client = EngineClient::new(options());
    client.set_keep_loaded_minutes(0.0);
    client.trim(&[0.0; 1600]).await.unwrap();
    // A long request keeps it alive past the 1.5 s idle delay.
    let started = Instant::now();
    let long = client.trim(&long_noise()).await;
    eprintln!("90 s of noise trimmed in {:?}", started.elapsed());
    assert!(long.is_ok());
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_rejects_pending_requests_and_the_next_request_restarts_it() {
    let client = EngineClient::new(options());
    client.trim(&[0.0; 1600]).await.unwrap();
    let pid = client.pid().unwrap();
    let busy = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.trim(&long_noise()).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    kill(pid);
    let err = busy.await.unwrap().unwrap_err();
    assert_eq!(err, EngineError::new("The speech engine stopped unexpectedly"));
    wait_until("the exit is noticed", Duration::from_secs(5), || !client.running()).await;
    assert!(client.trim(&[0.0; 1600]).await.is_ok());
    assert_ne!(client.pid(), Some(pid));
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_rejects_what_is_pending() {
    let client = EngineClient::new(options());
    client.trim(&[0.0; 1600]).await.unwrap();
    let busy = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.trim(&long_noise()).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    client.shutdown();
    assert!(!client.running());
    assert_eq!(busy.await.unwrap().unwrap_err(), EngineError::new("The speech engine stopped unexpectedly"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_process_that_exits_before_saying_hello_fails_the_start() {
    let cmd = std::env::var("ComSpec").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".into());
    let client = EngineClient::new(EngineOptions {
        exe: PathBuf::from(cmd),
        args: vec!["/c".into(), "exit 3".into()],
        ..options()
    });
    assert_eq!(client.trim(&[0.0; 16]).await.unwrap_err(), EngineError::new("The speech engine exited (code 3)"));
    assert!(!client.running());
}
