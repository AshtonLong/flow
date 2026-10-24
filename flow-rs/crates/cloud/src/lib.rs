//! Cloud speech adapters, one per provider API, and the optional language-model
//! cleanup pass. Port of `src/main/transcribe/cloud/*` and `src/main/cleanup/llm.ts`.

use std::sync::Arc;
use std::time::Duration;

use flow_core::catalog::{CUSTOM_PREFIX, Catalog, ProviderApi};
use flow_core::config::Config;
pub use flow_core::errors::{TranscribeError, TranscribeErrorCode};
use flow_core::protocol::settings::KeyTestResult;
use tokio_util::sync::CancellationToken;

mod base;
mod deepgram;
mod elevenlabs;
pub mod hints;
pub mod http;
pub mod llm;
mod mistral;
mod openai_compatible;
pub mod provider;
mod text;

#[cfg(test)]
mod tests;

pub use base::CloudTranscriber;
pub use elevenlabs::keyterms;
pub use llm::{CleanupError, CleanupErrorCode, Messages, build_messages, cleanup_with_llm, sanitise_reply};
pub use mistral::context_bias;
pub use openai_compatible::hint_prompt;
pub use provider::{ANTHROPIC_VERSION, Headers, ResolvedProvider, auth_headers, resolve_provider, secret_id_for};

use base::Adapter;
use http::error_detail;
use provider::with_headers;
use text::{js_trim, js_truncate};

/// Looks up a stored secret by id, normally the provider id.
pub type SecretLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Everything the adapters and the cleanup pass need from the app.
#[derive(Clone)]
pub struct CloudDeps {
    pub catalog: Arc<Catalog>,
    /// The effective (profile-resolved) config.
    pub config: Arc<Config>,
    /// Looks up a stored secret by id, normally the provider id.
    pub get_secret: SecretLookup,
    pub http: reqwest::Client,
}

impl std::fmt::Debug for CloudDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudDeps").finish_non_exhaustive()
    }
}

/// Per-dictation options for [`CloudTranscriber::transcribe`].
#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    /// `"en"` in v1.
    pub language: String,
    /// Dictionary terms, used where the model supports biasing.
    pub hints: Vec<String>,
    /// Cancels the request (Esc).
    pub cancel: CancellationToken,
}

/// Model a custom endpoint is asked for when `[providers.<id>].model` is not set.
const DEFAULT_CUSTOM_MODEL: &str = "whisper-1";

const KEY_TEST_TIMEOUT: Duration = Duration::from_millis(10_000);

fn adapter_for(provider: &ResolvedProvider, id: &str) -> Result<Adapter, TranscribeError> {
    match provider.api {
        ProviderApi::Openai => Ok(Adapter::OpenAiCompatible),
        ProviderApi::Mistral => Ok(Adapter::Mistral),
        ProviderApi::Elevenlabs => Ok(Adapter::ElevenLabs),
        ProviderApi::Deepgram => Ok(Adapter::Deepgram),
        ProviderApi::Anthropic => Err(TranscribeError::new(
            TranscribeErrorCode::UnknownModel,
            format!("{} has no speech-to-text API (model \"{id}\")", provider.name),
        )),
    }
}

/// Builds the adapter for a cloud model id (`groq/whisper-large-v3-turbo`, `custom/<provider-id>`, …).
/// Fails with TranscribeError `unknown-model` or `no-key` if it cannot.
pub fn create_cloud_transcriber(model_id: &str, deps: &CloudDeps) -> Result<CloudTranscriber, TranscribeError> {
    if let Some(provider_id) = model_id.strip_prefix(CUSTOM_PREFIX) {
        let settings = deps.config.providers.get(provider_id);
        let configured = settings.is_some_and(|s| s.base_url.as_deref().is_some_and(|url| !url.is_empty()));
        let resolved = if provider_id.is_empty() || !configured {
            None
        } else {
            resolve_provider(provider_id, deps)
        };
        let Some(resolved) = resolved else {
            return Err(TranscribeError::new(
                TranscribeErrorCode::UnknownModel,
                format!("Custom endpoint \"{provider_id}\" has no base_url in [providers.{provider_id}]"),
            )
            .with_user_message("Custom endpoint is not configured"));
        };
        let model = settings
            .and_then(|s| s.model.as_deref())
            .map(js_trim)
            .filter(|model| !model.is_empty())
            .unwrap_or(DEFAULT_CUSTOM_MODEL);
        return Ok(CloudTranscriber {
            id: model_id.to_string(),
            model: model.to_string(),
            // A custom endpoint always speaks the OpenAI API, whatever its id collides with.
            provider: ResolvedProvider { api: ProviderApi::Openai, keyless: true, ..resolved },
            adapter: Adapter::OpenAiCompatible,
            use_hints: true,
            http: deps.http.clone(),
        });
    }

    let Some(entry) = deps.catalog.cloud_entry(model_id) else {
        return Err(TranscribeError::new(
            TranscribeErrorCode::UnknownModel,
            format!("Unknown cloud model \"{model_id}\""),
        ));
    };
    let Some(provider) = resolve_provider(&entry.provider, deps) else {
        return Err(TranscribeError::new(
            TranscribeErrorCode::UnknownModel,
            format!("Model \"{model_id}\" names an unknown provider \"{}\"", entry.provider),
        ));
    };
    if provider.api_key.is_none() && !provider.keyless {
        return Err(TranscribeError::new(
            TranscribeErrorCode::NoKey,
            format!("No API key saved for {}", provider.name),
        )
        .with_user_message(format!("Add a {} API key in Settings", provider.name)));
    }
    let adapter = adapter_for(&provider, model_id)?;
    Ok(CloudTranscriber {
        id: model_id.to_string(),
        model: entry.model.clone(),
        provider,
        adapter,
        // The catalog opts a model in to hints (`hints` defaults to false in the Rust schema).
        use_hints: entry.hints != Some(false),
        http: deps.http.clone(),
    })
}

/// The cheapest authenticated GET each API offers.
fn key_test_url(provider: &ResolvedProvider) -> String {
    match provider.api {
        ProviderApi::Anthropic => format!("{}/models?limit=1", provider.base_url),
        ProviderApi::Elevenlabs => format!("{}/user", provider.base_url),
        ProviderApi::Deepgram => format!("{}/projects", provider.base_url),
        ProviderApi::Openai | ProviderApi::Mistral => format!("{}/models", provider.base_url),
    }
}

/// One cheap authenticated request (e.g. GET models) to check a stored key. Never fails.
pub async fn test_provider_key(provider_id: &str, deps: &CloudDeps) -> KeyTestResult {
    let result = |ok: bool, message: String| KeyTestResult { ok, message };
    let Some(provider) = resolve_provider(provider_id, deps) else {
        return result(false, format!("Unknown provider \"{provider_id}\""));
    };
    let name = &provider.name;
    if provider.api_key.is_none() && !provider.keyless {
        return result(false, format!("No API key saved for {name}"));
    }
    let deadline = tokio::time::Instant::now() + KEY_TEST_TIMEOUT;
    let request = with_headers(deps.http.get(key_test_url(&provider)), auth_headers(&provider));
    let response = match tokio::time::timeout_at(deadline, request.send()).await {
        Err(_) => return result(false, format!("{name} did not respond")),
        Ok(Err(_)) => return result(false, format!("Couldn't reach {name}")),
        Ok(Ok(response)) => response,
    };
    let status = response.status().as_u16();
    let body = match tokio::time::timeout_at(deadline, response.text()).await {
        Ok(Ok(body)) => body,
        _ => String::new(),
    };

    if (200..300).contains(&status) {
        let message = if provider.api_key.is_some() { "Key works".to_string() } else { format!("{name} is reachable") };
        return result(true, message);
    }
    // A scoped ElevenLabs key is valid but may not read the account endpoint.
    if status == 401 && body.to_ascii_lowercase().contains("missing_permissions") {
        return result(true, "Key works (restricted key)".into());
    }
    if status == 401 || status == 403 {
        return result(false, format!("{name} rejected the key ({status})"));
    }
    if status == 429 {
        return result(true, "Key accepted, but rate limited right now".into());
    }
    let detail = error_detail(&body);
    let message = if detail.is_empty() {
        format!("{name} returned {status}")
    } else {
        format!("{name} returned {status}: {detail}")
    };
    result(false, js_truncate(&message, 160).to_string())
}
