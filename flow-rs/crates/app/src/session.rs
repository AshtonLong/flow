//! The dictation session: hotkey events in, text at the cursor out.
//!
//!   Idle → Recording → Transcribing → Inserting → Idle
//!
//! A new recording can start while earlier ones are still transcribing; results
//! are inserted in the order they were spoken. Every failure ends in typed text,
//! clipboard text or a visible error — a dictation is never silently lost.
//!
//! Inbound calls (hotkeys, capture callbacks, tray actions) arrive on any
//! thread; state lives behind one mutex and outbound calls never call back into
//! the session synchronously, so there is no re-entrancy.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use flow_core::catalog::Catalog;
use flow_core::cleanup::{RulesContext, apply_rules, dictionary_hints};
use flow_core::config::{Config, InsertMethod};
use flow_core::matcher::HotkeyEvent;
use flow_core::profiles::resolve_profile;
use flow_core::types::{
    CaptureErrorReason, CaptureStart, DictationEvent, FocusedApp, InsertOutcome, InsertResult, NewHistoryEntry,
    OverlayPhase, OverlayState, SAMPLE_RATE, SessionState, Transcript,
};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

/// Recordings shorter than this are accidental taps.
pub const MIN_RECORDING_MS: f64 = 300.0;
/// How long to wait for the capture to flush its last audio chunk.
const FLUSH_TIMEOUT_MS: u64 = 300;

fn flash_ms(phase: OverlayPhase) -> u64 {
    match phase {
        OverlayPhase::Notice => 2600,
        OverlayPhase::Error => 4200,
        _ => 900,
    }
}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Why a transcription failed, as the session needs to know it.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscribeFailure {
    /// Esc (or another cancel) stopped it: nothing is reported.
    pub aborted: bool,
    /// The error code, for the log.
    pub code: String,
    pub message: String,
    /// One line fit for the overlay.
    pub user_message: String,
}

/// What the router produced for one dictation.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteResult {
    pub transcript: Transcript,
    pub cloud: bool,
    /// The active model failed and the fallback produced the text.
    pub fell_back: bool,
    /// One line describing why the fallback was used.
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TranscribeOpts {
    pub language: String,
    pub hints: Vec<String>,
    pub cancel: CancellationToken,
}

/// Options for one insertion, mirroring `InsertOptions` in the inserter.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertRequest {
    pub method: InsertMethod,
    pub paste_shortcut: String,
    pub restore_clipboard: bool,
    pub target: Option<FocusedApp>,
}

pub trait SessionRouter: Send + Sync {
    fn is_cloud(&self, model_id: &str) -> bool;
    /// Called at key-down so a local model loads while the user is speaking.
    fn warm_up(&self, config: Arc<Config>);
    fn transcribe(
        &self,
        audio: Arc<Vec<f32>>,
        config: Arc<Config>,
        opts: TranscribeOpts,
    ) -> BoxFuture<Result<RouteResult, TranscribeFailure>>;
}

pub trait SessionCapture: Send + Sync {
    fn start_capture(&self, options: CaptureStart);
    fn stop_capture(&self);
}

pub trait SessionOverlay: Send + Sync {
    fn set_state(&self, state: OverlayState);
}

/// Everything the session needs from the rest of the app.
pub struct SessionDeps {
    pub get_config: Arc<dyn Fn() -> Arc<Config> + Send + Sync>,
    pub get_catalog: Arc<dyn Fn() -> Arc<Catalog> + Send + Sync>,
    pub get_focused_app: Arc<dyn Fn() -> Option<FocusedApp> + Send + Sync>,
    pub insert: Arc<dyn Fn(String, InsertRequest) -> BoxFuture<InsertResult> + Send + Sync>,
    pub copy: Arc<dyn Fn(&str) + Send + Sync>,
    pub router: Arc<dyn SessionRouter>,
    /// Voice detection: speech with silence removed, `Ok(None)` when there is none.
    pub trim: Arc<dyn Fn(Arc<Vec<f32>>) -> BoxFuture<Result<Option<Vec<f32>>, String>> + Send + Sync>,
    /// The optional language-model pass over already rule-cleaned text.
    pub cleanup_llm:
        Arc<dyn Fn(String, Arc<Config>, Arc<Catalog>, CancellationToken) -> BoxFuture<Result<String, String>> + Send + Sync>,
    pub capture: Arc<dyn SessionCapture>,
    pub overlay: Arc<dyn SessionOverlay>,
    /// `cancel` (Esc) is only listened for while there is something to cancel.
    pub set_cancel_armed: Arc<dyn Fn(bool) + Send + Sync>,
    pub add_history: Arc<dyn Fn(NewHistoryEntry) + Send + Sync>,
    pub on_event: Arc<dyn Fn(SessionEvent) + Send + Sync>,
    /// Milliseconds on a monotonic clock.
    pub now: Arc<dyn Fn() -> f64 + Send + Sync>,
    /// Wall clock, Unix epoch milliseconds (history timestamps).
    pub epoch_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Dictation(DictationEvent),
    State(SessionState),
    Paused(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Hold,
    Locked,
}

struct Recording {
    id: u64,
    mode: Mode,
    chunks: Vec<Vec<f32>>,
    samples: usize,
    /// Effective config, with the focused app's profile applied at key-down.
    config: Arc<Config>,
    /// Audio is flowing from the microphone.
    started: bool,
    cancelled: bool,
    max_timer: Option<AbortHandle>,
}

struct Flushing {
    rec: Recording,
    target: Option<FocusedApp>,
    released_at: f64,
    timer: AbortHandle,
}

struct Job {
    id: u64,
    cancel: CancellationToken,
    cloud: bool,
}

struct LastDictation {
    audio: Arc<Vec<f32>>,
    config: Arc<Config>,
    target: Option<FocusedApp>,
    text: Option<String>,
}

enum Outcome {
    Text { text: String, raw_text: String, route: RouteResult },
    Empty,
    Cancelled,
    Error(String),
}

#[derive(Default)]
struct State {
    recording: Option<Recording>,
    flushing: Option<Flushing>,
    jobs: Vec<Job>,
    /// Resolves when the previous dictation has been delivered.
    insert_chain: Option<oneshot::Receiver<()>>,
    last: Option<LastDictation>,
    swallow_toggle: bool,
    paused: bool,
    next_id: u64,
    flash: Option<OverlayState>,
    flash_timer: Option<AbortHandle>,
    inserting: u32,
}

impl State {
    fn state(&self) -> SessionState {
        if self.recording.is_some() {
            SessionState::Recording
        } else if self.inserting > 0 {
            SessionState::Inserting
        } else if !self.jobs.is_empty() || self.flushing.is_some() {
            SessionState::Transcribing
        } else {
            SessionState::Idle
        }
    }
}

pub struct DictationSession {
    deps: SessionDeps,
    state: Mutex<State>,
    rt: Handle,
    me: std::sync::Weak<DictationSession>,
}

fn samples_ms(samples: usize) -> f64 {
    samples as f64 / SAMPLE_RATE as f64 * 1000.0
}

impl DictationSession {
    /// Must be called inside a tokio runtime: timers and jobs run on it.
    pub fn new(deps: SessionDeps) -> Arc<Self> {
        let rt = Handle::current();
        Arc::new_cyclic(|me| Self {
            deps,
            state: Mutex::new(State { next_id: 1, ..State::default() }),
            rt,
            me: me.clone(),
        })
    }

    fn arc(&self) -> Arc<Self> {
        self.me.upgrade().expect("session is alive while its methods run")
    }

    pub fn state(&self) -> SessionState {
        self.state.lock().state()
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().paused
    }

    pub fn last_text(&self) -> Option<String> {
        self.state.lock().last.as_ref().and_then(|l| l.text.clone())
    }

    pub fn can_retry(&self) -> bool {
        self.state.lock().last.is_some()
    }

    pub fn set_paused(&self, paused: bool) {
        self.state.lock().paused = paused;
        if paused {
            self.cancel();
        }
        (self.deps.on_event)(SessionEvent::Paused(paused));
    }

    // ── Hotkey events ────────────────────────────────────────────────────────

    pub fn handle_hotkey(&self, event: HotkeyEvent) {
        let mut st = self.state.lock();
        if st.paused && event != HotkeyEvent::Cancel {
            return;
        }
        match event {
            HotkeyEvent::HoldStart => {
                if st.recording.as_ref().is_some_and(|r| r.mode == Mode::Locked) {
                    // Hands-free recording ends on the next tap of the hold key. The same
                    // chord may go on to complete the toggle binding; that must not restart.
                    st.swallow_toggle = true;
                    self.stop(&mut st);
                } else if st.recording.is_none() {
                    self.start(&mut st, Mode::Hold);
                }
            }
            HotkeyEvent::HoldEnd { aborted } => {
                st.swallow_toggle = false;
                if st.recording.as_ref().is_some_and(|r| r.mode == Mode::Hold) {
                    if aborted {
                        self.discard_recording(&mut st);
                    } else {
                        self.stop(&mut st);
                    }
                }
            }
            HotkeyEvent::Lock => {
                if st.recording.is_none() {
                    self.start(&mut st, Mode::Locked);
                }
            }
            HotkeyEvent::Toggle => {
                if st.swallow_toggle {
                    return;
                }
                match st.recording.as_mut() {
                    None => self.start(&mut st, Mode::Locked),
                    Some(rec) if rec.mode == Mode::Hold => {
                        rec.mode = Mode::Locked;
                        self.refresh(&mut st);
                    }
                    Some(_) => self.stop(&mut st),
                }
            }
            HotkeyEvent::Cancel => {
                drop(st);
                self.cancel();
            }
            HotkeyEvent::PasteLast => {
                drop(st);
                let me = self.arc();
                self.rt.spawn(async move { me.paste_last().await });
            }
        }
    }

    // ── Recording ────────────────────────────────────────────────────────────

    fn start(&self, st: &mut State, mode: Mode) {
        // A previous recording still waiting on its last chunk is finalised now.
        self.finish_flushing(st);
        let app = (self.deps.get_focused_app)();
        let resolved = resolve_profile(&(self.deps.get_config)(), app.as_ref());
        if resolved.disabled {
            log::info!(
                target: "session",
                "dictation is disabled by profile \"{}\"",
                resolved.profile.as_deref().unwrap_or_default()
            );
            return;
        }
        let config = Arc::new(resolved.config);
        let id = st.next_id;
        st.next_id += 1;
        let max = Duration::from_secs(config.audio.max_recording_seconds.max(0) as u64);
        let me = self.me.clone();
        let max_timer = self
            .rt
            .spawn(async move {
                tokio::time::sleep(max).await;
                if let Some(me) = me.upgrade() {
                    let mut st = me.state.lock();
                    if st.recording.as_ref().is_some_and(|r| r.id == id) {
                        me.stop(&mut st);
                    }
                }
            })
            .abort_handle();
        st.recording = Some(Recording {
            id,
            mode,
            chunks: Vec::new(),
            samples: 0,
            config: config.clone(),
            started: false,
            cancelled: false,
            max_timer: Some(max_timer),
        });
        self.clear_flash(st);
        self.deps.capture.start_capture(CaptureStart {
            device_id: config.audio.input_device.clone(),
            keep_warm: config.audio.keep_mic_warm,
        });
        // The local model loads while the user is speaking.
        self.deps.router.warm_up(config);
        self.refresh(st);
    }

    fn stop(&self, st: &mut State) {
        let Some(mut rec) = st.recording.take() else { return };
        if let Some(timer) = rec.max_timer.take() {
            timer.abort();
        }
        let released_at = (self.deps.now)();
        // The window focused at key-up receives the text, even if focus moves later.
        let target = (self.deps.get_focused_app)();
        self.deps.capture.stop_capture();
        let id = rec.id;
        let me = self.me.clone();
        let timer = self
            .rt
            .spawn(async move {
                tokio::time::sleep(Duration::from_millis(FLUSH_TIMEOUT_MS)).await;
                if let Some(me) = me.upgrade() {
                    let mut st = me.state.lock();
                    if st.flushing.as_ref().is_some_and(|f| f.rec.id == id) {
                        me.finish_flushing(&mut st);
                    }
                }
            })
            .abort_handle();
        // A recording already waiting on its flush is finalised first.
        self.finish_flushing(st);
        st.flushing = Some(Flushing { rec, target, released_at, timer });
        self.refresh(st);
    }

    /// The flush is over (or timed out, or was superseded): hand the audio on.
    fn finish_flushing(&self, st: &mut State) {
        let Some(flushing) = st.flushing.take() else { return };
        flushing.timer.abort();
        self.finalize(st, flushing.rec, flushing.target, flushing.released_at);
    }

    fn discard_recording(&self, st: &mut State) {
        let Some(mut rec) = st.recording.take() else { return };
        rec.cancelled = true;
        if let Some(timer) = rec.max_timer.take() {
            timer.abort();
        }
        self.deps.capture.stop_capture();
        self.refresh(st);
    }

    /// Esc: drops the recording and any dictation still being transcribed. Types nothing.
    pub fn cancel(&self) {
        let mut st = self.state.lock();
        self.discard_recording(&mut st);
        if let Some(flushing) = st.flushing.as_mut() {
            flushing.rec.cancelled = true;
            self.finish_flushing(&mut st);
        }
        for job in &st.jobs {
            job.cancel.cancel();
        }
        self.clear_flash(&mut st);
        self.refresh(&mut st);
    }

    // ── Capture callbacks ────────────────────────────────────────────────────

    pub fn on_capture_started(&self) {
        let mut st = self.state.lock();
        let Some(rec) = st.recording.as_mut() else { return };
        rec.started = true;
        self.refresh(&mut st);
    }

    pub fn on_capture_chunk(&self, pcm: Vec<f32>) {
        let mut st = self.state.lock();
        let st = &mut *st;
        let rec = match (st.flushing.as_mut(), st.recording.as_mut()) {
            (Some(f), _) => &mut f.rec,
            (None, Some(r)) => r,
            (None, None) => return,
        };
        rec.samples += pcm.len();
        rec.chunks.push(pcm);
    }

    pub fn on_capture_stopped(&self) {
        let mut st = self.state.lock();
        self.finish_flushing(&mut st);
    }

    pub fn on_capture_error(&self, reason: CaptureErrorReason, message: Option<String>) {
        let mut st = self.state.lock();
        let Some(mut rec) = st.recording.take() else { return };
        rec.cancelled = true;
        if let Some(timer) = rec.max_timer.take() {
            timer.abort();
        }
        let text = message.unwrap_or_else(|| reason.default_message().to_string());
        log::error!(target: "session", "capture failed: {}", reason_name(reason));
        self.show_flash(&mut st, OverlayState { phase: OverlayPhase::Error, cloud: false, message: Some(text.clone()), ..Default::default() });
        drop(st);
        self.emit_event(DictationEvent { id: rec.id, ok: false, error: Some(text), ..Default::default() });
    }

    // ── Transcribe, clean up, insert ─────────────────────────────────────────

    fn finalize(&self, st: &mut State, rec: Recording, target: Option<FocusedApp>, released_at: f64) {
        if rec.cancelled {
            self.refresh(st);
            return;
        }
        if samples_ms(rec.samples) < MIN_RECORDING_MS {
            self.refresh(st);
            return;
        }
        let mut audio = Vec::with_capacity(rec.samples);
        for chunk in &rec.chunks {
            audio.extend_from_slice(chunk);
        }
        let audio = Arc::new(audio);
        // Kept in memory (never on disk) until the next dictation, for "retry last".
        st.last = Some(LastDictation { audio: audio.clone(), config: rec.config.clone(), target: target.clone(), text: None });
        self.enqueue(st, rec.id, audio, rec.config, target, released_at);
    }

    fn enqueue(
        &self,
        st: &mut State,
        id: u64,
        audio: Arc<Vec<f32>>,
        config: Arc<Config>,
        target: Option<FocusedApp>,
        released_at: f64,
    ) {
        let cancel = CancellationToken::new();
        let cloud = self.deps.router.is_cloud(&config.model.active);
        st.jobs.push(Job { id, cancel: cancel.clone(), cloud });
        self.refresh(st);
        let (done_tx, done_rx) = oneshot::channel::<()>();
        let previous = st.insert_chain.replace(done_rx);
        let me = self.arc();
        let work = {
            let me = me.clone();
            let audio = audio.clone();
            let config = config.clone();
            let cancel = cancel.clone();
            self.rt.spawn(async move { me.process(audio, config, cancel).await })
        };
        self.rt.spawn(async move {
            let outcome = match work.await {
                Ok(outcome) => outcome,
                Err(err) => {
                    log::error!(target: "session", "delivery failed: {err}");
                    Outcome::Error("Something went wrong".into())
                }
            };
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            me.deliver(id, cancel, cloud, outcome, config, target, audio, released_at).await;
            let _ = done_tx.send(());
        });
    }

    async fn trim(&self, audio: Arc<Vec<f32>>) -> Option<Vec<f32>> {
        match (self.deps.trim)(audio.clone()).await {
            Ok(speech) => speech,
            Err(err) => {
                // Voice detection is an optimisation; without it the raw audio is used.
                log::warn!(target: "session", "voice detection failed: {err}");
                Some(audio.as_ref().clone())
            }
        }
    }

    async fn process(&self, audio: Arc<Vec<f32>>, config: Arc<Config>, cancel: CancellationToken) -> Outcome {
        let speech = self.trim(audio).await;
        if cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        // No speech: nothing is sent or typed. This also prevents phantom text on silence.
        let Some(speech) = speech.filter(|s| s.len() as f64 >= SAMPLE_RATE as f64 * 0.1) else {
            return Outcome::Empty;
        };
        let opts = TranscribeOpts {
            language: config.general.language.clone(),
            hints: dictionary_hints(&config),
            cancel: cancel.clone(),
        };
        let route = match self.deps.router.transcribe(Arc::new(speech), config.clone(), opts).await {
            Ok(route) => route,
            Err(failure) => {
                if failure.aborted || cancel.is_cancelled() {
                    return Outcome::Cancelled;
                }
                log::error!(target: "session", "transcription failed: {} {}", failure.code, failure.message);
                return Outcome::Error(failure.user_message);
            }
        };
        if cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        let raw_text = route.transcript.text.clone();
        if raw_text.trim().is_empty() {
            return Outcome::Empty;
        }
        let text = self.clean_up(&raw_text, config, Some(cancel.clone())).await;
        if cancel.is_cancelled() {
            return Outcome::Cancelled;
        }
        if text.trim().is_empty() {
            return Outcome::Empty;
        }
        Outcome::Text { text, raw_text, route }
    }

    /// Layer 1 rules, then the optional LLM pass. The LLM can never lose a dictation.
    pub async fn clean_up(&self, raw_text: &str, config: Arc<Config>, cancel: Option<CancellationToken>) -> String {
        let ruled = apply_rules(raw_text, &config, &RulesContext::default());
        if !config.cleanup.llm_enabled || ruled.trim().is_empty() {
            return ruled;
        }
        let cancel = cancel.unwrap_or_default();
        let catalog = (self.deps.get_catalog)();
        match (self.deps.cleanup_llm)(ruled.trim().to_string(), config.clone(), catalog, cancel.clone()).await {
            Ok(rewritten) => {
                let trailing = if config.insert.trailing_space && !rewritten.ends_with('\n') { " " } else { "" };
                format!("{}{}", rewritten.trim(), trailing)
            }
            Err(err) => {
                if !cancel.is_cancelled() {
                    log::warn!(target: "session", "LLM cleanup skipped: {err}");
                }
                ruled
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver(
        &self,
        id: u64,
        cancel: CancellationToken,
        job_cloud: bool,
        outcome: Outcome,
        config: Arc<Config>,
        target: Option<FocusedApp>,
        audio: Arc<Vec<f32>>,
        released_at: f64,
    ) {
        let audio_ms = samples_ms(audio.len());
        let finish = |flash: Option<OverlayState>| {
            let mut st = self.state.lock();
            st.jobs.retain(|j| j.id != id);
            match flash {
                Some(flash) => self.show_flash(&mut st, flash),
                None => self.refresh(&mut st),
            }
        };

        let (text, raw_text, route) = match outcome {
            _ if cancel.is_cancelled() => return finish(None),
            Outcome::Cancelled | Outcome::Empty => return finish(None),
            Outcome::Error(message) => {
                self.emit_event(DictationEvent {
                    id,
                    ok: false,
                    error: Some(message.clone()),
                    audio_ms,
                    cloud: job_cloud,
                    ..Default::default()
                });
                return finish(Some(OverlayState {
                    phase: OverlayPhase::Error,
                    cloud: false,
                    message: Some(message),
                    ..Default::default()
                }));
            }
            Outcome::Text { text, raw_text, route } => (text, raw_text, route),
        };

        {
            let mut st = self.state.lock();
            st.inserting += 1;
        }
        let request = InsertRequest {
            method: config.insert.method,
            paste_shortcut: config.insert.paste_shortcut.clone(),
            restore_clipboard: config.insert.restore_clipboard,
            target: target.clone(),
        };
        let result = (self.deps.insert)(text.clone(), request).await;
        let elapsed_ms = (self.deps.now)() - released_at;
        {
            let mut st = self.state.lock();
            st.inserting -= 1;
            if let Some(last) = st.last.as_mut().filter(|l| Arc::ptr_eq(&l.audio, &audio)) {
                last.text = Some(text.clone());
            }
        }
        log::info!(
            target: "session",
            "dictation {id}: {} ms audio, {}, {} ms to {}",
            audio_ms.round(),
            route.transcript.model,
            elapsed_ms.round(),
            outcome_name(result.outcome)
        );
        log::debug!(target: "session", "text: {text}");

        if config.history.enabled {
            (self.deps.add_history)(NewHistoryEntry {
                created_at: (self.deps.epoch_ms)(),
                text: text.clone(),
                raw_text: raw_text.clone(),
                model: route.transcript.model.clone(),
                app: target.as_ref().map(|t| t.process_name.clone()).unwrap_or_default(),
                audio_ms: audio_ms.round() as i64,
                elapsed_ms: elapsed_ms.round() as i64,
            });
        }

        self.emit_event(DictationEvent {
            id,
            ok: true,
            text: text.clone(),
            raw_text,
            model: route.transcript.model.clone(),
            cloud: route.cloud,
            audio_ms,
            elapsed_ms,
            outcome: Some(result.outcome),
            fell_back: route.fell_back,
            error: None,
        });

        if result.outcome == InsertOutcome::Clipboard {
            let why = result.reason.unwrap_or(flow_core::types::InsertFailure::Failed).describe();
            return finish(Some(OverlayState {
                phase: OverlayPhase::Notice,
                cloud: false,
                message: Some(format!("{why} — copied to clipboard")),
                ..Default::default()
            }));
        }
        if route.fell_back {
            let reason = route.fallback_reason.clone().unwrap_or_else(|| "Cloud model failed".into());
            return finish(Some(OverlayState {
                phase: OverlayPhase::Notice,
                cloud: false,
                message: Some(format!("{reason} — used the local model")),
                ..Default::default()
            }));
        }
        finish(Some(OverlayState { phase: OverlayPhase::Done, cloud: route.cloud, ..Default::default() }))
    }

    // ── Tray and settings actions ────────────────────────────────────────────

    /// Re-inserts the previous transcript at the cursor.
    pub async fn paste_last(&self) {
        let Some(text) = self.last_text() else { return };
        let app = (self.deps.get_focused_app)();
        let config = resolve_profile(&(self.deps.get_config)(), app.as_ref()).config;
        let request = InsertRequest {
            method: config.insert.method,
            paste_shortcut: config.insert.paste_shortcut.clone(),
            restore_clipboard: config.insert.restore_clipboard,
            target: (self.deps.get_focused_app)(),
        };
        (self.deps.insert)(text, request).await;
    }

    pub fn copy_last(&self) -> bool {
        let Some(text) = self.last_text() else { return false };
        (self.deps.copy)(&text);
        true
    }

    /// Re-runs the last recording, with the current active model.
    pub fn retry_last(&self) -> bool {
        let mut st = self.state.lock();
        let Some(last) = st.last.as_ref() else { return false };
        let mut config = (*last.config).clone();
        config.model = (self.deps.get_config)().model.clone();
        let (audio, target) = (last.audio.clone(), last.target.clone());
        let id = st.next_id;
        st.next_id += 1;
        let now = (self.deps.now)();
        self.enqueue(&mut st, id, audio, Arc::new(config), target, now);
        true
    }

    /// Transcribes audio recorded in the settings window; nothing is inserted.
    pub async fn test(&self, audio: Vec<f32>) -> Result<Transcript, TranscribeFailure> {
        let config = (self.deps.get_config)();
        let audio_ms = samples_ms(audio.len());
        let Some(speech) = self.trim(Arc::new(audio)).await else {
            return Ok(Transcript { text: String::new(), model: config.model.active.clone(), audio_ms, elapsed_ms: 0.0 });
        };
        let started = (self.deps.now)();
        let opts = TranscribeOpts {
            language: config.general.language.clone(),
            hints: dictionary_hints(&config),
            cancel: CancellationToken::new(),
        };
        let route = self.deps.router.transcribe(Arc::new(speech), config.clone(), opts).await?;
        let text = self.clean_up(&route.transcript.text, config, None).await;
        Ok(Transcript { text, model: route.transcript.model, audio_ms, elapsed_ms: (self.deps.now)() - started })
    }

    // ── Overlay ──────────────────────────────────────────────────────────────

    fn emit_event(&self, event: DictationEvent) {
        (self.deps.on_event)(SessionEvent::Dictation(event));
    }

    fn clear_flash(&self, st: &mut State) {
        if let Some(timer) = st.flash_timer.take() {
            timer.abort();
        }
        st.flash = None;
    }

    fn show_flash(&self, st: &mut State, state: OverlayState) {
        self.clear_flash(st);
        let ms = flash_ms(state.phase);
        st.flash = Some(state);
        let me = self.me.clone();
        st.flash_timer = Some(
            self.rt
                .spawn(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    if let Some(me) = me.upgrade() {
                        let mut st = me.state.lock();
                        st.flash = None;
                        st.flash_timer = None;
                        me.refresh(&mut st);
                    }
                })
                .abort_handle(),
        );
        self.refresh(st);
    }

    /// Recomputes what the overlay shows from the current recording, jobs and flash message.
    fn refresh(&self, st: &mut State) {
        (self.deps.set_cancel_armed)(st.recording.is_some() || st.flushing.is_some() || !st.jobs.is_empty());
        (self.deps.on_event)(SessionEvent::State(st.state()));
        let queued = st.jobs.len() as u32;
        if let Some(rec) = st.recording.as_ref().filter(|r| r.started) {
            self.deps.overlay.set_state(OverlayState {
                phase: OverlayPhase::Listening,
                cloud: self.deps.router.is_cloud(&rec.config.model.active),
                locked: rec.mode == Mode::Locked,
                message: None,
                queued,
            });
            return;
        }
        // A tap too short to keep must not flash the pill while its audio is flushed.
        let flushing_real = st.flushing.as_ref().is_some_and(|f| {
            !f.rec.cancelled && f.rec.samples as f64 >= MIN_RECORDING_MS / 1000.0 * SAMPLE_RATE as f64
        });
        if !st.jobs.is_empty() || flushing_real {
            let cloud = st.jobs.iter().any(|j| j.cloud)
                || (flushing_real
                    && st.flushing.as_ref().is_some_and(|f| self.deps.router.is_cloud(&f.rec.config.model.active)));
            self.deps.overlay.set_state(OverlayState {
                phase: OverlayPhase::Transcribing,
                cloud,
                queued,
                ..Default::default()
            });
            return;
        }
        // The pill appears only once audio is flowing, so its appearance means "speak now".
        if st.recording.is_some() {
            return;
        }
        self.deps.overlay.set_state(st.flash.clone().unwrap_or_else(OverlayState::hidden));
    }
}

fn reason_name(reason: CaptureErrorReason) -> &'static str {
    match reason {
        CaptureErrorReason::DeviceMissing => "device-missing",
        CaptureErrorReason::Permission => "permission",
        CaptureErrorReason::Busy => "busy",
        CaptureErrorReason::Unknown => "unknown",
    }
}

fn outcome_name(outcome: InsertOutcome) -> &'static str {
    match outcome {
        InsertOutcome::Pasted => "pasted",
        InsertOutcome::Typed => "typed",
        InsertOutcome::Clipboard => "clipboard",
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
