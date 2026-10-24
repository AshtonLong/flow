//! End to end through the native library: `EngineClient` → `flow-engine-host`
//! → `transcribe.dll`. Ignored by default; run with
//! `cargo test -p flow-engine --test e2e -- --ignored --nocapture --test-threads 1`.
//!
//! The DLLs are copied from `FLOW_TEST_TRANSCRIBE_DIR` (default: the installed
//! Electron app's `@transcribe-cpp\win32-x64-cpu-vulkan`) into an `engine`
//! folder next to the host executable, the layout the app ships with. Models
//! are read from `FLOW_TEST_MODELS_DIR` (default `%LOCALAPPDATA%\Flow\models`).

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use flow_core::protocol::engine::{EngineDevice, EngineModelRef, VadKind};
use flow_engine::{EngineClient, EngineError, EngineOptions};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        eprintln!("{:<5} [{}] {}", record.level(), record.target(), record.args());
    }
    fn flush(&self) {}
}

fn host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_flow-engine-host"))
}

/// Copies the prebuilt library next to the host, once per test run.
fn install_library() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let _ = log::set_logger(&StderrLogger).map(|()| log::set_max_level(log::LevelFilter::Info));
        let source = std::env::var_os("FLOW_TEST_TRANSCRIBE_DIR").map(PathBuf::from).unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
                .join("Programs\\Flow\\resources\\app.asar.unpacked\\node_modules\\@transcribe-cpp\\win32-x64-cpu-vulkan")
        });
        let target = host().parent().unwrap().join("engine");
        std::fs::create_dir_all(&target).unwrap();
        for entry in std::fs::read_dir(&source).unwrap_or_else(|e| panic!("{}: {e}", source.display())) {
            let path = entry.unwrap().path();
            let is_dll = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll"));
            if !is_dll {
                continue;
            }
            let dest = target.join(path.file_name().unwrap());
            let same = dest.metadata().ok().map(|m| m.len()) == path.metadata().ok().map(|m| m.len());
            if !same {
                std::fs::copy(&path, &dest).unwrap();
            }
        }
    });
}

fn client() -> Arc<EngineClient> {
    install_library();
    EngineClient::new(EngineOptions {
        exe: host(),
        args: vec!["--engine".into()],
        library_path: None,
        cuda_runtime_dir: None,
        debug: false,
    })
}

fn models_dir() -> PathBuf {
    std::env::var_os("FLOW_TEST_MODELS_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Flow\\models")
    })
}

fn models() -> Vec<(PathBuf, u64)> {
    let mut found: Vec<(PathBuf, u64)> = std::fs::read_dir(models_dir())
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gguf")))
                .filter_map(|p| Some((p.clone(), p.metadata().ok()?.len())))
                .collect()
        })
        .unwrap_or_default();
    found.sort_by_key(|(_, size)| *size);
    found
}

fn model_ref(path: &Path, device: EngineDevice) -> EngineModelRef {
    let id = path.file_stem().unwrap().to_string_lossy().into_owned();
    EngineModelRef { model_id: id, path: path.to_string_lossy().into_owned(), device }
}

fn smallest() -> Option<PathBuf> {
    models().into_iter().next().map(|(p, _)| p)
}

fn parakeet() -> Option<PathBuf> {
    models().into_iter().map(|(p, _)| p).find(|p| p.to_string_lossy().to_lowercase().contains("parakeet"))
}

fn words(text: &str) -> String {
    text.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn transcribe_jfk(client: &EngineClient, model: &EngineModelRef) {
    let audio = common::jfk();
    let trimmed = client.trim(&audio).await.unwrap();
    let speech = trimmed.audio.unwrap();
    let load = client.load(model, None).await.unwrap();
    let result = client.transcribe(model, &speech, "en", &[], None).await.unwrap();
    let again = client.transcribe(model, &speech, "en", &["Kennedy".to_owned()], None).await.unwrap();
    eprintln!(
        "{} on {:?}: load {:.0} ms on {} | infer {:.0} ms (second run {:.0} ms) on {} | trim kept {:.0} ms ({:?})\n  text: {:?}",
        model.model_id,
        model.device,
        load.load_ms,
        load.backend,
        result.infer_ms,
        again.infer_ms,
        result.backend,
        trimmed.speech_ms,
        trimmed.detector,
        result.text
    );
    assert_eq!(result.load_ms, 0.0, "already loaded");
    assert!(words(&result.text).contains("ask not what your country can do for you"), "{}", result.text);
    assert!(words(&again.text).contains("ask not what your country can do for you"), "{}", again.text);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs transcribe.dll"]
async fn probe_lists_devices() {
    let client = client();
    let started = Instant::now();
    let probe = client.probe().await.unwrap();
    eprintln!("probe in {:?}: transcribe.cpp {}, vad {:?}", started.elapsed(), probe.version, probe.vad);
    for d in &probe.devices {
        eprintln!("  {} kind={} type={} {:?} {} MiB", d.name, d.kind, d.device_type, d.description, d.memory_total >> 20);
    }
    assert_eq!(probe.version, "0.3.0");
    assert_eq!(probe.vad, VadKind::Silero);
    assert!(probe.devices.iter().any(|d| d.device_type == "cpu"));
    assert_eq!(client.devices(), probe.devices);
    assert_eq!(client.vad(), Some(VadKind::Silero));
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs transcribe.dll and a model"]
async fn transcribes_jfk_with_the_smallest_model() {
    let Some(path) = smallest() else { return eprintln!("no model in {}", models_dir().display()) };
    for device in [EngineDevice::Auto, EngineDevice::Cpu] {
        let client = client();
        transcribe_jfk(&client, &model_ref(&path, device)).await;
        client.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs transcribe.dll and the Parakeet model"]
async fn transcribes_jfk_with_parakeet() {
    let Some(path) = parakeet() else { return eprintln!("no Parakeet model in {}", models_dir().display()) };
    for device in [EngineDevice::Auto, EngineDevice::Cpu] {
        let client = client();
        transcribe_jfk(&client, &model_ref(&path, device)).await;
        client.shutdown();
    }
}

/// Transcribes a minute of speech on the CPU, cancelling a quarter of the way
/// through a full run; returns (full run ms, outcome of the cancelled run, its ms).
async fn cancel_mid_way(
    client: &EngineClient,
    model: &EngineModelRef,
) -> (f64, Result<flow_core::protocol::engine::EngineTranscribeResult, EngineError>, f64) {
    client.load(model, None).await.unwrap();
    let audio: Vec<f32> = common::jfk().repeat(6);
    let started = Instant::now();
    client.transcribe(model, &audio, "en", &[], None).await.unwrap();
    let full_ms = started.elapsed().as_secs_f64() * 1000.0;

    let token = CancellationToken::new();
    let cancel_after = Duration::from_millis((full_ms / 4.0) as u64);
    tokio::spawn({
        let token = token.clone();
        async move {
            tokio::time::sleep(cancel_after).await;
            token.cancel();
        }
    });
    let started = Instant::now();
    let outcome = client.transcribe(model, &audio, "en", &[], Some(token)).await;
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "{}: full run {full_ms:.0} ms; cancelled after {cancel_after:?}; the cancelled run ended after {ms:.0} ms with {}",
        model.model_id,
        match &outcome {
            Ok(_) => "a transcript".to_owned(),
            Err(e) => format!("{e:?}"),
        }
    );
    (full_ms, outcome, ms)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs transcribe.dll and a model"]
async fn cancels_a_transcription_mid_way() {
    // Moonshine polls the abort callback during an offline run.
    let Some(path) = smallest() else { return };
    let client = client();
    let model = model_ref(&path, EngineDevice::Cpu);
    let (full_ms, outcome, ms) = cancel_mid_way(&client, &model).await;
    assert_eq!(outcome.unwrap_err(), EngineError::aborted());
    assert!(ms < full_ms * 0.75, "the run was not cut short");
    // The engine and the model are still usable.
    let after = client.transcribe(&model, &common::jfk(), "en", &[], None).await.unwrap();
    assert!(words(&after.text).contains("ask not"));
    assert_eq!(after.load_ms, 0.0);
    client.shutdown();
}

/// transcribe.cpp 0.3.0's Parakeet reports the `cancellation` feature but does
/// not poll the abort callback during an offline CPU run: the run finishes and
/// its transcript comes back. The Electron engine behaved the same (same
/// library, same callback). Records which way it went.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs transcribe.dll and the Parakeet model"]
async fn parakeet_cancellation() {
    let Some(path) = parakeet() else { return };
    let client = client();
    let (_, outcome, _) = cancel_mid_way(&client, &model_ref(&path, EngineDevice::Cpu)).await;
    match outcome {
        Ok(result) => assert!(words(&result.text).contains("ask not")),
        Err(err) => assert_eq!(err, EngineError::aborted()),
    }
    client.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs transcribe.dll and a model"]
async fn exits_when_idle_and_reports_the_loaded_model() {
    let Some(path) = smallest() else { return };
    let client = client();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    client.on_loaded({
        let seen = Arc::clone(&seen);
        move |id| seen.lock().push(id.to_owned())
    });
    let model = model_ref(&path, EngineDevice::Auto);
    client.set_keep_loaded_minutes(-1.0);
    client.transcribe(&model, &common::jfk(), "en", &[], None).await.unwrap();
    assert_eq!(client.loaded_model_id(), Some(model.model_id.clone()));
    client.unload().await.unwrap();
    assert_eq!(client.loaded_model_id(), None);
    assert_eq!(*seen.lock(), [model.model_id.clone(), String::new()]);

    client.set_keep_loaded_minutes(0.0);
    client.transcribe(&model, &common::jfk(), "en", &[], None).await.unwrap();
    let idle_from = Instant::now();
    while client.running() {
        assert!(idle_from.elapsed() < Duration::from_secs(5), "the engine did not exit");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    eprintln!("engine exited {:?} after the last reply", idle_from.elapsed());
    assert_eq!(client.loaded_model_id(), None);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs transcribe.dll"]
async fn a_missing_model_file_is_an_error_not_a_crash() {
    let client = client();
    let model = EngineModelRef {
        model_id: "missing".into(),
        path: "C:\\flow-test\\missing.gguf".into(),
        device: EngineDevice::Cpu,
    };
    let err = client.load(&model, None).await.unwrap_err();
    eprintln!("load error: {}", err.message);
    assert!(!err.aborted);
    assert!(err.message.starts_with("loading model C:\\flow-test\\missing.gguf: "), "{}", err.message);
    assert!(client.running());
    client.shutdown();
}
