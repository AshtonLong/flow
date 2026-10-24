//! Picks the transcriber for a model id (the local engine or a cloud adapter),
//! splits long recordings at pauses, and falls back to the installed local
//! model when a cloud request fails. Mirrors `src/main/transcribe/router.ts`.

use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Instant;

use flow_cloud::{CloudDeps, CloudTranscriber, TranscribeOptions};
use flow_core::catalog::{Catalog, is_cloud_model_id};
use flow_core::config::{Config, DeviceSetting};
use flow_core::errors::{TranscribeError, TranscribeErrorCode};
use flow_core::protocol::engine::{EngineDevice, EngineModelRef};
use flow_core::split::split_ranges;
use flow_core::types::{SAMPLE_RATE, Transcript};
use tokio_util::sync::CancellationToken;

use crate::session::{BoxFuture, RouteResult, SessionRouter, TranscribeFailure, TranscribeOpts};

/// Recordings longer than this are split at pauses.
const MAX_PIECE_SECONDS: f64 = 30.0;

/// An engine failure: its message, and whether it was a cancel.
#[derive(Debug, Clone)]
pub struct EngineFailure {
    pub message: String,
    pub aborted: bool,
}

/// The local engine, as the router uses it.
pub trait LocalEngine: Send + Sync {
    fn load(&self, model: EngineModelRef) -> BoxFuture<Result<(), EngineFailure>>;
    fn transcribe(
        &self,
        model: EngineModelRef,
        audio: Vec<f32>,
        language: String,
        hints: Vec<String>,
        cancel: CancellationToken,
    ) -> BoxFuture<Result<String, EngineFailure>>;
}

/// The model manager, as the router uses it.
pub trait ModelLookup: Send + Sync {
    fn catalog(&self) -> Arc<Catalog>;
    /// Display name of a local model (catalog or `[[local_models]]`).
    fn local_name(&self, id: &str) -> Option<String>;
    fn model_path(&self, id: &str) -> Option<PathBuf>;
    fn is_installed(&self, id: &str) -> bool;
}

enum Transcriber {
    Local(EngineModelRef),
    Cloud(CloudTranscriber),
}

impl Transcriber {
    fn id(&self) -> String {
        match self {
            Self::Local(model) => model.model_id.clone(),
            Self::Cloud(cloud) => cloud.id().to_string(),
        }
    }
}

pub struct Router {
    engine: Arc<dyn LocalEngine>,
    models: Arc<dyn ModelLookup>,
    get_secret: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    http: reqwest::Client,
    me: Weak<Router>,
}

fn engine_device(device: DeviceSetting) -> EngineDevice {
    match device {
        DeviceSetting::Auto => EngineDevice::Auto,
        DeviceSetting::Cpu => EngineDevice::Cpu,
        DeviceSetting::Gpu => EngineDevice::Gpu,
    }
}

/// The session's view of a [`TranscribeError`].
pub fn to_failure(err: &TranscribeError) -> TranscribeFailure {
    TranscribeFailure {
        aborted: err.code == TranscribeErrorCode::Aborted,
        code: err.code.as_str().to_string(),
        message: err.message.clone(),
        user_message: err.user_message(),
    }
}

fn engine_error(err: EngineFailure) -> TranscribeError {
    if err.aborted {
        TranscribeError::new(TranscribeErrorCode::Aborted, "Cancelled")
    } else {
        TranscribeError::new(TranscribeErrorCode::Engine, err.message)
    }
}

impl Router {
    pub fn new(
        engine: Arc<dyn LocalEngine>,
        models: Arc<dyn ModelLookup>,
        get_secret: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
        http: reqwest::Client,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self { engine, models, get_secret, http, me: me.clone() })
    }

    fn cloud_deps(&self, config: &Arc<Config>) -> CloudDeps {
        CloudDeps {
            catalog: self.models.catalog(),
            config: config.clone(),
            get_secret: self.get_secret.clone(),
            http: self.http.clone(),
        }
    }

    /// Builds the transcriber for a model id. Fails with a TranscribeError if it cannot be used.
    fn resolve(&self, model_id: &str, config: &Arc<Config>) -> Result<Transcriber, TranscribeError> {
        if is_cloud_model_id(model_id) {
            return flow_cloud::create_cloud_transcriber(model_id, &self.cloud_deps(config)).map(Transcriber::Cloud);
        }
        self.local(model_id, config).map(Transcriber::Local)
    }

    fn local(&self, model_id: &str, config: &Config) -> Result<EngineModelRef, TranscribeError> {
        let Some(name) = self.models.local_name(model_id) else {
            return Err(TranscribeError::new(
                TranscribeErrorCode::UnknownModel,
                format!("Unknown model \"{model_id}\""),
            ));
        };
        let Some(path) = self.models.model_path(model_id) else {
            return Err(TranscribeError::new(TranscribeErrorCode::NotInstalled, format!("{name} is not downloaded")));
        };
        Ok(EngineModelRef {
            model_id: model_id.to_string(),
            path: path.to_string_lossy().into_owned(),
            device: engine_device(config.model.device),
        })
    }

    /// Called at key-down so a local model loads while the user is speaking.
    pub fn warm_up_config(&self, config: Arc<Config>) {
        let id = config.model.active.clone();
        if is_cloud_model_id(&id) {
            return;
        }
        // Not installed: the dictation itself reports it.
        let Ok(model) = self.local(&id, &config) else { return };
        let load = self.engine.load(model);
        tokio::spawn(async move {
            if let Err(err) = load.await {
                log::warn!(target: "router", "warm-up failed: {}", err.message);
            }
        });
    }

    async fn transcribe_piece(
        &self,
        transcriber: &Transcriber,
        audio: &[f32],
        opts: &TranscribeOpts,
    ) -> Result<String, TranscribeError> {
        match transcriber {
            Transcriber::Local(model) => self
                .engine
                .transcribe(
                    model.clone(),
                    audio.to_vec(),
                    opts.language.clone(),
                    opts.hints.clone(),
                    opts.cancel.clone(),
                )
                .await
                .map_err(engine_error),
            Transcriber::Cloud(cloud) => {
                let options = TranscribeOptions {
                    language: opts.language.clone(),
                    hints: opts.hints.clone(),
                    cancel: opts.cancel.clone(),
                };
                cloud.transcribe(audio, &options).await.map(|t| t.text)
            }
        }
    }

    async fn run(
        &self,
        transcriber: &Transcriber,
        audio: &[f32],
        opts: &TranscribeOpts,
    ) -> Result<Transcript, TranscribeError> {
        let pieces = if audio.len() as f64 > MAX_PIECE_SECONDS * SAMPLE_RATE as f64 {
            split_ranges(audio, MAX_PIECE_SECONDS, SAMPLE_RATE)
        } else {
            std::iter::once(0..audio.len()).collect()
        };
        let started = Instant::now();
        let mut texts: Vec<String> = Vec::new();
        for piece in pieces {
            if opts.cancel.is_cancelled() {
                return Err(TranscribeError::new(TranscribeErrorCode::Aborted, "Cancelled"));
            }
            let text = self.transcribe_piece(transcriber, &audio[piece], opts).await?;
            if !text.is_empty() {
                texts.push(text);
            }
        }
        Ok(Transcript {
            text: texts.join(" ").trim().to_string(),
            model: transcriber.id(),
            audio_ms: audio.len() as f64 / SAMPLE_RATE as f64 * 1000.0,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Transcribes with one specific model and no fallback (test bench, retry on another model).
    pub async fn transcribe_with(
        &self,
        model_id: &str,
        audio: Arc<Vec<f32>>,
        config: Arc<Config>,
        opts: TranscribeOpts,
    ) -> Result<Transcript, TranscribeFailure> {
        self.transcribe_with_error(model_id, audio, config, opts).await.map_err(|e| to_failure(&e))
    }

    async fn transcribe_with_error(
        &self,
        model_id: &str,
        audio: Arc<Vec<f32>>,
        config: Arc<Config>,
        opts: TranscribeOpts,
    ) -> Result<Transcript, TranscribeError> {
        let transcriber = self.resolve(model_id, &config)?;
        self.run(&transcriber, &audio, &opts).await
    }

    /// Transcribes with the active model, falling back to the local fallback model on failure.
    pub async fn transcribe(
        &self,
        audio: Arc<Vec<f32>>,
        config: Arc<Config>,
        opts: TranscribeOpts,
    ) -> Result<RouteResult, TranscribeError> {
        let active_id = config.model.active.clone();
        let cloud = is_cloud_model_id(&active_id);
        let failure = match self.resolve(&active_id, &config) {
            Ok(transcriber) => match self.run(&transcriber, &audio, &opts).await {
                Ok(transcript) => {
                    return Ok(RouteResult { transcript, cloud, fell_back: false, fallback_reason: None });
                }
                Err(err) => err,
            },
            Err(err) => err,
        };
        if failure.code == TranscribeErrorCode::Aborted {
            return Err(failure);
        }
        let fallback_id = config.model.fallback.clone();
        let can_fall_back =
            fallback_id != active_id && !is_cloud_model_id(&fallback_id) && self.models.is_installed(&fallback_id);
        if !can_fall_back {
            return Err(failure);
        }
        log::warn!(target: "router", "{active_id} failed ({}); using {fallback_id}", failure.code.as_str());
        let local = match self.local(&fallback_id, &config) {
            Ok(model) => Transcriber::Local(model),
            Err(_) => return Err(failure),
        };
        match self.run(&local, &audio, &opts).await {
            Ok(transcript) => Ok(RouteResult {
                transcript,
                cloud: false,
                fell_back: true,
                fallback_reason: Some(failure.user_message()),
            }),
            Err(second) if second.code == TranscribeErrorCode::Aborted => Err(second),
            // Report the original failure: it is the one the user can act on.
            Err(_) => Err(failure),
        }
    }
}

impl SessionRouter for Router {
    fn is_cloud(&self, model_id: &str) -> bool {
        is_cloud_model_id(model_id)
    }

    fn warm_up(&self, config: Arc<Config>) {
        self.warm_up_config(config);
    }

    fn transcribe(
        &self,
        audio: Arc<Vec<f32>>,
        config: Arc<Config>,
        opts: TranscribeOpts,
    ) -> BoxFuture<Result<RouteResult, TranscribeFailure>> {
        let me = self.me.upgrade().expect("the router outlives its calls");
        Box::pin(async move { Router::transcribe(&me, audio, config, opts).await.map_err(|e| to_failure(&e)) })
    }
}

// ── Adapters for the real engine and model manager ────────────────────────

/// The engine client behind the router's [`LocalEngine`] interface.
pub struct EngineAdapter(pub Arc<flow_engine::EngineClient>);

impl LocalEngine for EngineAdapter {
    fn load(&self, model: EngineModelRef) -> BoxFuture<Result<(), EngineFailure>> {
        let me = self.0.clone();
        Box::pin(async move {
            me.load(&model, None)
                .await
                .map(|_| ())
                .map_err(|e| EngineFailure { message: e.message, aborted: e.aborted })
        })
    }

    fn transcribe(
        &self,
        model: EngineModelRef,
        audio: Vec<f32>,
        language: String,
        hints: Vec<String>,
        cancel: CancellationToken,
    ) -> BoxFuture<Result<String, EngineFailure>> {
        let me = self.0.clone();
        Box::pin(async move {
            me.transcribe(&model, &audio, &language, &hints, Some(cancel))
                .await
                .map(|r| r.text)
                .map_err(|e| EngineFailure { message: e.message, aborted: e.aborted })
        })
    }
}

impl ModelLookup for flow_store::ModelManager {
    fn catalog(&self) -> Arc<Catalog> {
        flow_store::ModelManager::catalog(self)
    }
    fn local_name(&self, id: &str) -> Option<String> {
        self.local_entry(id).map(|e| e.name)
    }
    fn model_path(&self, id: &str) -> Option<PathBuf> {
        flow_store::ModelManager::model_path(self, id)
    }
    fn is_installed(&self, id: &str) -> bool {
        flow_store::ModelManager::is_installed(self, id)
    }
}

#[cfg(test)]
mod tests {
    //! Port of `tests/unit/transcribe/router.test.ts`.
    use std::collections::HashSet;

    use flow_core::catalog::{CloudModelEntry, ProviderApi, ProviderEntry};
    use parking_lot::Mutex;

    use super::*;

    fn catalog() -> Catalog {
        Catalog {
            version: 1,
            as_of: "2026-10-03".into(),
            providers: vec![ProviderEntry {
                id: "groq".into(),
                name: "Groq".into(),
                api: ProviderApi::Openai,
                base_url: "https://api.groq.com/openai/v1".into(),
                key_url: None,
                keyless: false,
            }],
            local: vec![],
            cloud: vec![CloudModelEntry {
                id: "groq/whisper-large-v3-turbo".into(),
                provider: "groq".into(),
                model: "whisper-large-v3-turbo".into(),
                name: "Whisper large-v3-turbo".into(),
                description: String::new(),
                price_per_hour: 0.04,
                recommended: false,
                hints: None,
                min_billed_seconds: None,
                docs_url: None,
            }],
            cleanup: vec![],
        }
    }

    #[derive(Default)]
    struct FakeEngine {
        loads: Mutex<Vec<EngineModelRef>>,
        calls: Mutex<Vec<(EngineModelRef, usize)>>,
        abort_next: Mutex<bool>,
    }

    impl LocalEngine for FakeEngine {
        fn load(&self, model: EngineModelRef) -> BoxFuture<Result<(), EngineFailure>> {
            self.loads.lock().push(model);
            Box::pin(async { Ok(()) })
        }
        fn transcribe(
            &self,
            model: EngineModelRef,
            audio: Vec<f32>,
            _language: String,
            _hints: Vec<String>,
            _cancel: CancellationToken,
        ) -> BoxFuture<Result<String, EngineFailure>> {
            self.calls.lock().push((model, audio.len()));
            let abort = std::mem::take(&mut *self.abort_next.lock());
            let len = audio.len();
            Box::pin(async move {
                if abort {
                    Err(EngineFailure { message: "Aborted".into(), aborted: true })
                } else {
                    Ok(format!("local {len}"))
                }
            })
        }
    }

    struct FakeModels {
        installed: HashSet<String>,
    }

    impl ModelLookup for FakeModels {
        fn catalog(&self) -> Arc<Catalog> {
            Arc::new(catalog())
        }
        fn local_name(&self, id: &str) -> Option<String> {
            Some(id.to_string())
        }
        fn model_path(&self, id: &str) -> Option<PathBuf> {
            self.installed.contains(id).then(|| PathBuf::from(format!("C:/models/{id}.gguf")))
        }
        fn is_installed(&self, id: &str) -> bool {
            self.installed.contains(id)
        }
    }

    fn setup(installed: &[&str]) -> (Arc<Router>, Arc<FakeEngine>) {
        let engine = Arc::new(FakeEngine::default());
        let models = Arc::new(FakeModels { installed: installed.iter().map(|s| s.to_string()).collect() });
        let router = Router::new(engine.clone(), models, Arc::new(|_| None), reqwest::Client::new());
        (router, engine)
    }

    fn default_setup() -> (Arc<Router>, Arc<FakeEngine>) {
        setup(&["parakeet-tdt-0.6b-v2"])
    }

    fn opts() -> TranscribeOpts {
        TranscribeOpts { language: "en".into(), hints: vec![], cancel: CancellationToken::new() }
    }

    fn default_config() -> Arc<Config> {
        Arc::new(Config::default())
    }

    fn cloud_config() -> Arc<Config> {
        let mut config = Config::default();
        config.model.active = "groq/whisper-large-v3-turbo".into();
        Arc::new(config)
    }

    #[tokio::test]
    async fn sends_local_model_ids_to_the_engine() {
        let (router, engine) = default_setup();
        let result = router.transcribe(Arc::new(vec![0.0; 16000]), default_config(), opts()).await.unwrap();
        assert!(!result.cloud);
        assert!(!result.fell_back);
        assert_eq!(result.transcript.text, "local 16000");
        assert_eq!(result.transcript.model, "parakeet-tdt-0.6b-v2");
        assert_eq!(result.transcript.audio_ms, 1000.0);
        let (model, _) = engine.calls.lock()[0].clone();
        assert_eq!(model.model_id, "parakeet-tdt-0.6b-v2");
        assert_eq!(model.device, EngineDevice::Auto);
    }

    #[tokio::test]
    async fn reports_a_local_model_that_is_not_downloaded() {
        let (router, _) = setup(&[]);
        let err = router.transcribe(Arc::new(vec![0.0; 16000]), default_config(), opts()).await.unwrap_err();
        assert_eq!(err.code, TranscribeErrorCode::NotInstalled);
    }

    #[tokio::test]
    async fn falls_back_to_the_local_model_when_the_cloud_model_cannot_run() {
        // No key is stored, so the cloud adapter cannot be built.
        let (router, engine) = default_setup();
        let result = router.transcribe(Arc::new(vec![0.0; 16000]), cloud_config(), opts()).await.unwrap();
        assert!(result.fell_back);
        assert!(!result.cloud);
        assert_eq!(result.transcript.model, "parakeet-tdt-0.6b-v2");
        assert!(result.fallback_reason.is_some_and(|r| !r.is_empty()));
        assert_eq!(engine.calls.lock().len(), 1);
    }

    #[tokio::test]
    async fn reports_the_cloud_failure_when_no_fallback_is_installed() {
        let (router, _) = setup(&[]);
        let err = router.transcribe(Arc::new(vec![0.0; 16000]), cloud_config(), opts()).await.unwrap_err();
        assert_eq!(err.code, TranscribeErrorCode::NoKey);
    }

    #[tokio::test]
    async fn does_not_fall_back_when_the_user_cancels() {
        let (router, engine) = default_setup();
        *engine.abort_next.lock() = true;
        let err = router.transcribe(Arc::new(vec![0.0; 16000]), default_config(), opts()).await.unwrap_err();
        assert_eq!(err.code, TranscribeErrorCode::Aborted);
    }

    #[tokio::test]
    async fn splits_recordings_over_30_seconds_and_joins_the_pieces() {
        let (router, engine) = default_setup();
        let audio = Arc::new(vec![0.2f32; 16000 * 70]);
        let result = router.transcribe(audio.clone(), default_config(), opts()).await.unwrap();
        let lengths: Vec<usize> = engine.calls.lock().iter().map(|(_, len)| *len).collect();
        assert!(lengths.len() >= 3);
        assert!(*lengths.iter().max().unwrap() <= 16000 * 30);
        assert_eq!(lengths.iter().sum::<usize>(), audio.len());
        assert_eq!(result.transcript.text.split(" local ").count(), lengths.len());
        assert_eq!(result.transcript.audio_ms, 70_000.0);
    }

    #[tokio::test]
    async fn transcribe_with_runs_exactly_the_requested_model_with_no_fallback() {
        let (router, engine) = default_setup();
        let result = router
            .transcribe_with("groq/whisper-large-v3-turbo", Arc::new(vec![0.0; 16000]), cloud_config(), opts())
            .await;
        assert!(result.is_err());
        assert!(engine.calls.lock().is_empty());
    }

    #[tokio::test]
    async fn warm_up_loads_the_active_local_model_and_ignores_cloud_models() {
        let (router, engine) = default_setup();
        router.warm_up_config(default_config());
        tokio::task::yield_now().await;
        {
            let loads = engine.loads.lock();
            assert_eq!(loads.len(), 1);
            assert_eq!(loads[0].model_id, "parakeet-tdt-0.6b-v2");
            assert_eq!(loads[0].path, "C:/models/parakeet-tdt-0.6b-v2.gguf");
            assert_eq!(loads[0].device, EngineDevice::Auto);
        }
        engine.loads.lock().clear();
        router.warm_up_config(cloud_config());
        assert!(engine.loads.lock().is_empty());
    }

    #[test]
    fn failures_keep_the_code_and_the_overlay_line() {
        let failure = to_failure(&TranscribeError::new(TranscribeErrorCode::Auth, "bad key"));
        assert!(!failure.aborted);
        assert_eq!(failure.code, "auth");
        assert_eq!(failure.message, "bad key");
        assert_eq!(failure.user_message, "The API key was rejected");
        assert!(to_failure(&engine_error(EngineFailure { message: "x".into(), aborted: true })).aborted);
        let engine = engine_error(EngineFailure { message: "engine exploded".into(), aborted: false });
        assert_eq!(engine.code, TranscribeErrorCode::Engine);
        assert_eq!(engine.message, "engine exploded");
    }
}
