//! Port of `tests/unit/session/session.test.ts`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flow_core::catalog::Catalog;
use flow_core::config::{Config, InsertMethod, Profile};
use flow_core::matcher::HotkeyEvent;
use flow_core::types::{
    CaptureErrorReason, CaptureStart, DictationEvent, FocusedApp, InsertFailure, InsertOutcome, InsertResult,
    OverlayPhase, OverlayState, SessionState, Transcript,
};
use parking_lot::Mutex;
use tokio::sync::oneshot;

use super::*;

fn notepad() -> FocusedApp {
    FocusedApp {
        window_id: "100".into(),
        process_name: "notepad.exe".into(),
        title: "Notes".into(),
        pid: 1,
        elevated: false,
    }
}
fn slack() -> FocusedApp {
    FocusedApp {
        window_id: "200".into(),
        process_name: "slack.exe".into(),
        title: "Slack".into(),
        pid: 2,
        elevated: false,
    }
}

fn seconds(n: f64) -> Vec<f32> {
    vec![0.1; (16000.0 * n).round() as usize]
}

fn route(text: &str) -> RouteResult {
    RouteResult {
        transcript: Transcript {
            text: text.into(),
            model: "parakeet-tdt-0.6b-v2".into(),
            audio_ms: 1000.0,
            elapsed_ms: 5.0,
        },
        cloud: false,
        fell_back: false,
        fallback_reason: None,
    }
}

fn failure(code: &str, message: &str) -> TranscribeFailure {
    TranscribeFailure {
        aborted: code == "aborted",
        code: code.into(),
        message: message.into(),
        user_message: message.into(),
    }
}

enum Behaviour {
    Resolve(RouteResult),
    Reject(TranscribeFailure),
    /// Resolves when the sender fires.
    Wait(oneshot::Receiver<RouteResult>),
    /// Like `Wait`, but rejects as aborted as soon as the job is cancelled.
    WaitCancellable(oneshot::Receiver<RouteResult>),
}

#[derive(Default)]
struct FakeRouter {
    calls: Mutex<usize>,
    queue: Mutex<VecDeque<Behaviour>>,
    warmed: Mutex<usize>,
}

impl FakeRouter {
    fn calls(&self) -> usize {
        *self.calls.lock()
    }
    fn push(&self, behaviour: Behaviour) {
        self.queue.lock().push_back(behaviour);
    }
}

impl SessionRouter for FakeRouter {
    fn is_cloud(&self, model_id: &str) -> bool {
        model_id.contains('/')
    }
    fn warm_up(&self, _config: Arc<Config>) {
        *self.warmed.lock() += 1;
    }
    fn transcribe(
        &self,
        _audio: Arc<Vec<f32>>,
        _config: Arc<Config>,
        opts: TranscribeOpts,
    ) -> BoxFuture<Result<RouteResult, TranscribeFailure>> {
        *self.calls.lock() += 1;
        let behaviour = self.queue.lock().pop_front().unwrap_or(Behaviour::Resolve(route("Hello world")));
        Box::pin(async move {
            match behaviour {
                Behaviour::Resolve(r) => Ok(r),
                Behaviour::Reject(f) => Err(f),
                Behaviour::Wait(rx) => rx.await.map_err(|_| failure("engine", "dropped")),
                Behaviour::WaitCancellable(rx) => tokio::select! {
                    r = rx => r.map_err(|_| failure("engine", "dropped")),
                    _ = opts.cancel.cancelled() => Err(failure("aborted", "Cancelled")),
                },
            }
        })
    }
}

struct Recorder {
    captures: Mutex<Vec<&'static str>>,
    states: Mutex<Vec<OverlayState>>,
}

impl SessionCapture for Recorder {
    fn start_capture(&self, _options: CaptureStart) {
        self.captures.lock().push("start");
    }
    fn stop_capture(&self) {
        self.captures.lock().push("stop");
    }
}

impl SessionOverlay for Recorder {
    fn set_state(&self, state: OverlayState) {
        self.states.lock().push(state);
    }
}

type TrimResult = Result<Option<Vec<f32>>, String>;

struct Harness {
    session: Arc<DictationSession>,
    recorder: Arc<Recorder>,
    inserted: Arc<Mutex<Vec<(String, InsertRequest)>>>,
    events: Arc<Mutex<Vec<DictationEvent>>>,
    copied: Arc<Mutex<Vec<String>>>,
    armed: Arc<Mutex<Vec<bool>>>,
    focused: Arc<Mutex<Option<FocusedApp>>>,
    config: Arc<Mutex<Config>>,
    router: Arc<FakeRouter>,
    insert_result: Arc<Mutex<InsertResult>>,
    trim_queue: Arc<Mutex<VecDeque<TrimResult>>>,
}

fn harness() -> Harness {
    let recorder = Arc::new(Recorder { captures: Mutex::default(), states: Mutex::default() });
    let inserted: Arc<Mutex<Vec<(String, InsertRequest)>>> = Arc::default();
    let events: Arc<Mutex<Vec<DictationEvent>>> = Arc::default();
    let copied: Arc<Mutex<Vec<String>>> = Arc::default();
    let armed: Arc<Mutex<Vec<bool>>> = Arc::default();
    let focused = Arc::new(Mutex::new(Some(notepad())));
    let config = Arc::new(Mutex::new(Config::default()));
    let router = Arc::new(FakeRouter::default());
    let insert_result = Arc::new(Mutex::new(InsertResult::ok(InsertOutcome::Pasted)));
    let trim_queue: Arc<Mutex<VecDeque<TrimResult>>> = Arc::default();
    let started = Instant::now();

    let deps = SessionDeps {
        get_config: {
            let config = config.clone();
            Arc::new(move || Arc::new(config.lock().clone()))
        },
        get_catalog: Arc::new(|| {
            Arc::new(Catalog {
                version: 1,
                as_of: "2026-10-03".into(),
                providers: vec![],
                local: vec![],
                cloud: vec![],
                cleanup: vec![],
            })
        }),
        get_focused_app: {
            let focused = focused.clone();
            Arc::new(move || focused.lock().clone())
        },
        insert: {
            let inserted = inserted.clone();
            let insert_result = insert_result.clone();
            Arc::new(move |text, request| {
                inserted.lock().push((text, request));
                let result = *insert_result.lock();
                Box::pin(async move { result })
            })
        },
        copy: {
            let copied = copied.clone();
            Arc::new(move |text: &str| copied.lock().push(text.to_string()))
        },
        router: router.clone(),
        trim: {
            let trim_queue = trim_queue.clone();
            Arc::new(move |audio: Arc<Vec<f32>>| {
                let next = trim_queue.lock().pop_front();
                Box::pin(async move { next.unwrap_or_else(|| Ok(Some(audio.as_ref().clone()))) })
            })
        },
        cleanup_llm: Arc::new(|text, _, _, _| Box::pin(async move { Ok(text) })),
        capture: recorder.clone(),
        overlay: recorder.clone(),
        set_cancel_armed: {
            let armed = armed.clone();
            Arc::new(move |value| armed.lock().push(value))
        },
        add_history: Arc::new(|_| {}),
        on_event: {
            let events = events.clone();
            Arc::new(move |event| {
                if let SessionEvent::Dictation(event) = event {
                    events.lock().push(event);
                }
            })
        },
        now: Arc::new(move || started.elapsed().as_secs_f64() * 1000.0),
        epoch_ms: Arc::new(|| 0),
    };
    Harness {
        session: DictationSession::new(deps),
        recorder,
        inserted,
        events,
        copied,
        armed,
        focused,
        config,
        router,
        insert_result,
        trim_queue,
    }
}

impl Harness {
    /// Hold the key, speak for `sec` seconds, release.
    fn dictate(&self, sec: f64) {
        self.session.handle_hotkey(HotkeyEvent::HoldStart);
        self.session.on_capture_started();
        self.session.on_capture_chunk(seconds(sec));
        self.session.handle_hotkey(HotkeyEvent::HoldEnd { aborted: false });
        self.session.on_capture_stopped();
    }

    /// Overlay phases in order, with consecutive repeats collapsed (the real window de-duplicates).
    fn phases(&self) -> Vec<OverlayPhase> {
        let mut out: Vec<OverlayPhase> = Vec::new();
        for state in self.recorder.states.lock().iter() {
            if out.last() != Some(&state.phase) {
                out.push(state.phase);
            }
        }
        out
    }

    fn last_state(&self) -> Option<OverlayState> {
        self.recorder.states.lock().last().cloned()
    }

    fn inserted_count(&self) -> usize {
        self.inserted.lock().len()
    }
}

async fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn types_the_transcript_at_the_cursor_after_hold_and_release() {
    let h = harness();
    h.dictate(1.0);
    wait_for("insert", || h.inserted_count() == 1).await;
    let (text, options) = h.inserted.lock()[0].clone();
    assert_eq!(text, "Hello world ");
    assert_eq!(options.method, InsertMethod::Paste);
    assert_eq!(options.target, Some(notepad()));
    wait_for("done", || h.phases().contains(&OverlayPhase::Done)).await;
    let visible: Vec<_> = h.phases().into_iter().filter(|p| *p != OverlayPhase::Hidden).take(3).collect();
    assert_eq!(visible, vec![OverlayPhase::Listening, OverlayPhase::Transcribing, OverlayPhase::Done]);
    let event = h.events.lock()[0].clone();
    assert!(event.ok);
    assert_eq!(event.text, "Hello world ");
    assert_eq!(event.outcome, Some(InsertOutcome::Pasted));
    assert_eq!(h.session.last_text().as_deref(), Some("Hello world "));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shows_the_pill_only_once_audio_is_flowing() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    assert!(!h.phases().contains(&OverlayPhase::Listening));
    h.session.on_capture_started();
    assert_eq!(h.phases().last(), Some(&OverlayPhase::Listening));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discards_recordings_under_300_ms_as_accidental_taps() {
    let h = harness();
    h.dictate(0.2);
    settle().await;
    assert_eq!(h.router.calls(), 0);
    assert_eq!(h.inserted_count(), 0);
    assert!(!h.phases().contains(&OverlayPhase::Transcribing));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn types_nothing_when_no_speech_is_detected() {
    let h = harness();
    h.trim_queue.lock().push_back(Ok(None));
    h.dictate(1.0);
    wait_for("hidden", || h.phases().last() == Some(&OverlayPhase::Hidden)).await;
    assert_eq!(h.router.calls(), 0);
    assert_eq!(h.inserted_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn still_transcribes_when_voice_detection_fails() {
    let h = harness();
    h.trim_queue.lock().push_back(Err("engine down".into()));
    h.dictate(1.0);
    wait_for("insert", || h.inserted_count() == 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancels_a_recording_with_esc_and_types_nothing() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    h.session.on_capture_started();
    h.session.on_capture_chunk(seconds(1.0));
    h.session.handle_hotkey(HotkeyEvent::Cancel);
    h.session.handle_hotkey(HotkeyEvent::HoldEnd { aborted: false });
    settle().await;
    assert_eq!(h.inserted_count(), 0);
    assert_eq!(h.router.calls(), 0);
    assert_eq!(h.session.state(), SessionState::Idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancels_during_transcription_and_types_nothing() {
    let h = harness();
    let (release, rx) = oneshot::channel();
    h.router.push(Behaviour::WaitCancellable(rx));
    h.dictate(1.0);
    wait_for("transcribe", || h.router.calls() == 1).await;
    assert_eq!(h.session.state(), SessionState::Transcribing);
    h.session.handle_hotkey(HotkeyEvent::Cancel);
    let _ = release.send(route("too late"));
    wait_for("idle", || h.session.state() == SessionState::Idle).await;
    assert_eq!(h.inserted_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inserts_back_to_back_dictations_in_the_order_they_were_spoken() {
    let h = harness();
    let (finish_first, rx) = oneshot::channel();
    h.router.push(Behaviour::Wait(rx));
    h.router.push(Behaviour::Resolve(route("Second")));
    h.dictate(1.0);
    wait_for("first transcribe", || h.router.calls() == 1).await;
    h.dictate(1.0);
    wait_for("second transcribe", || h.router.calls() == 2).await;
    settle().await;
    assert_eq!(h.inserted_count(), 0);
    let _ = finish_first.send(route("First"));
    wait_for("both inserted", || h.inserted_count() == 2).await;
    let texts: Vec<String> = h.inserted.lock().iter().map(|(t, _)| t.trim().to_string()).collect();
    assert_eq!(texts, vec!["First", "Second"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn targets_the_window_focused_at_key_up() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    h.session.on_capture_started();
    h.session.on_capture_chunk(seconds(1.0));
    *h.focused.lock() = Some(slack());
    h.session.handle_hotkey(HotkeyEvent::HoldEnd { aborted: false });
    *h.focused.lock() = Some(notepad());
    h.session.on_capture_stopped();
    wait_for("insert", || h.inserted_count() == 1).await;
    assert_eq!(h.inserted.lock()[0].1.target, Some(slack()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drops_the_recording_when_a_foreign_key_aborts_the_hold() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    h.session.on_capture_started();
    h.session.on_capture_chunk(seconds(1.0));
    h.session.handle_hotkey(HotkeyEvent::HoldEnd { aborted: true });
    settle().await;
    assert_eq!(h.router.calls(), 0);
    assert_eq!(*h.recorder.captures.lock(), vec!["start", "stop"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locks_hands_free_when_the_toggle_chord_completes_during_a_hold() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    h.session.on_capture_started();
    h.session.handle_hotkey(HotkeyEvent::Toggle);
    let last = h.last_state().unwrap();
    assert_eq!(last.phase, OverlayPhase::Listening);
    assert!(last.locked);
    h.session.handle_hotkey(HotkeyEvent::HoldEnd { aborted: false });
    assert_eq!(h.session.state(), SessionState::Recording);
    h.session.on_capture_chunk(seconds(1.0));
    // Pressing the chord again: the hold part stops it, the toggle part must not restart.
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    h.session.handle_hotkey(HotkeyEvent::Toggle);
    h.session.handle_hotkey(HotkeyEvent::HoldEnd { aborted: false });
    h.session.on_capture_stopped();
    wait_for("insert", || h.inserted_count() == 1).await;
    wait_for("idle", || h.session.state() == SessionState::Idle).await;
    assert_eq!(*h.recorder.captures.lock(), vec!["start", "stop"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starts_hands_free_on_a_double_tap_and_stops_on_the_next_tap() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::Lock);
    h.session.on_capture_started();
    let last = h.last_state().unwrap();
    assert_eq!(last.phase, OverlayPhase::Listening);
    assert!(last.locked);
    h.session.on_capture_chunk(seconds(1.0));
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    h.session.on_capture_stopped();
    h.session.handle_hotkey(HotkeyEvent::HoldEnd { aborted: false });
    wait_for("insert", || h.inserted_count() == 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toggles_with_a_binding_that_is_not_a_superset_of_hold() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::Toggle);
    h.session.on_capture_started();
    h.session.on_capture_chunk(seconds(1.0));
    h.session.handle_hotkey(HotkeyEvent::Toggle);
    h.session.on_capture_stopped();
    wait_for("insert", || h.inserted_count() == 1).await;
}

fn profile(name: &str, process: &str, enabled: bool) -> Profile {
    Profile {
        name: name.into(),
        match_process: vec![process.into()],
        match_title: vec![],
        enabled,
        audio: None,
        model: None,
        cleanup: None,
        insert: None,
        overlay: None,
        history: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_nothing_in_an_app_whose_profile_disables_dictation() {
    let h = harness();
    h.config.lock().profiles = vec![profile("Vault", "notepad.exe", false)];
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    assert!(h.recorder.captures.lock().is_empty());
    assert_eq!(h.session.state(), SessionState::Idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn applies_the_focused_app_profile_to_insertion() {
    let h = harness();
    let mut terminals = profile("Terminals", "notepad.exe", true);
    let mut insert = serde_json::Map::new();
    insert.insert("paste_shortcut".into(), "Ctrl+Shift+V".into());
    terminals.insert = Some(insert);
    h.config.lock().profiles = vec![terminals];
    h.dictate(1.0);
    wait_for("insert", || h.inserted_count() == 1).await;
    assert_eq!(h.inserted.lock()[0].1.paste_shortcut, "Ctrl+Shift+V");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn says_so_when_the_text_went_to_the_clipboard_instead() {
    let h = harness();
    *h.insert_result.lock() = InsertResult::clipboard(InsertFailure::NoTarget);
    h.dictate(1.0);
    wait_for("notice", || h.phases().contains(&OverlayPhase::Notice)).await;
    let notice = h.recorder.states.lock().iter().find(|s| s.phase == OverlayPhase::Notice).cloned().unwrap();
    assert!(notice.message.unwrap().to_lowercase().contains("clipboard"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn says_so_when_the_local_fallback_produced_the_text() {
    let h = harness();
    let mut fell_back = route("offline text");
    fell_back.fell_back = true;
    fell_back.fallback_reason = Some("No connection".into());
    h.router.push(Behaviour::Resolve(fell_back));
    h.dictate(1.0);
    wait_for("notice", || h.phases().contains(&OverlayPhase::Notice)).await;
    let notice = h.recorder.states.lock().iter().find(|s| s.phase == OverlayPhase::Notice).cloned().unwrap();
    assert!(notice.message.unwrap().to_lowercase().contains("local model"));
    let event = h.events.lock()[0].clone();
    assert!(event.ok);
    assert!(event.fell_back);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shows_a_one_line_error_and_keeps_the_audio_for_retry() {
    let h = harness();
    h.router.push(Behaviour::Reject(failure("auth", "Invalid API key")));
    h.dictate(1.0);
    wait_for("error", || h.phases().contains(&OverlayPhase::Error)).await;
    assert_eq!(h.inserted_count(), 0);
    assert!(!h.events.lock()[0].ok);
    assert!(h.session.can_retry());
    assert!(h.session.retry_last());
    wait_for("insert", || h.inserted_count() == 1).await;
    assert_eq!(h.inserted.lock()[0].1.target, Some(notepad()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_a_microphone_error_immediately_at_key_down() {
    let h = harness();
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    h.session.on_capture_error(CaptureErrorReason::Busy, None);
    let last = h.last_state().unwrap();
    assert_eq!(last.phase, OverlayPhase::Error);
    assert!(last.message.unwrap().to_lowercase().contains("in use"));
    assert_eq!(h.session.state(), SessionState::Idle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shows_the_cloud_flag_while_a_cloud_model_is_active() {
    let h = harness();
    h.config.lock().model.active = "groq/whisper-large-v3-turbo".into();
    let mut cloud = route("from the cloud");
    cloud.cloud = true;
    h.router.push(Behaviour::Resolve(cloud));
    h.dictate(1.0);
    wait_for("insert", || h.inserted_count() == 1).await;
    let states = h.recorder.states.lock().clone();
    assert!(states.iter().find(|s| s.phase == OverlayPhase::Listening).unwrap().cloud);
    assert!(states.iter().find(|s| s.phase == OverlayPhase::Transcribing).unwrap().cloud);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ignores_hotkeys_while_paused() {
    let h = harness();
    h.session.set_paused(true);
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    assert!(h.recorder.captures.lock().is_empty());
    h.session.set_paused(false);
    h.session.handle_hotkey(HotkeyEvent::HoldStart);
    assert_eq!(*h.recorder.captures.lock(), vec!["start"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn re_inserts_the_previous_transcript_on_paste_last() {
    let h = harness();
    h.dictate(1.0);
    wait_for("insert", || h.inserted_count() == 1).await;
    *h.focused.lock() = Some(slack());
    h.session.handle_hotkey(HotkeyEvent::PasteLast);
    wait_for("second insert", || h.inserted_count() == 2).await;
    let (text, options) = h.inserted.lock()[1].clone();
    assert_eq!(text, "Hello world ");
    assert_eq!(options.target, Some(slack()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn arms_esc_only_while_there_is_something_to_cancel() {
    let h = harness();
    h.dictate(1.0);
    wait_for("insert", || h.inserted_count() == 1).await;
    wait_for("disarmed", || h.armed.lock().last() == Some(&false)).await;
    assert!(h.armed.lock().contains(&true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copy_last_puts_the_previous_transcript_on_the_clipboard() {
    let h = harness();
    assert!(!h.session.copy_last());
    h.dictate(1.0);
    wait_for("insert", || h.inserted_count() == 1).await;
    assert!(h.session.copy_last());
    assert_eq!(*h.copied.lock(), vec!["Hello world ".to_string()]);
}
