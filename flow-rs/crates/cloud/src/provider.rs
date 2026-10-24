//! Resolves a provider id to its endpoint and key, for speech adapters and the cleanup LLM.
//! Port of `src/main/transcribe/cloud/provider.ts`.

use flow_core::catalog::ProviderApi;
use flow_core::config::Config;

use crate::CloudDeps;
use crate::text::js_trim;

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub id: String,
    pub name: String,
    pub api: ProviderApi,
    /// Base URL without a trailing slash.
    pub base_url: String,
    /// Requests work without a key (local endpoints, custom endpoints).
    pub keyless: bool,
    pub api_key: Option<String>,
    /// Defined only in `[providers.<id>]`, not in the catalog.
    pub custom: bool,
}

/// Never prints the key, so a provider can be logged safely.
impl std::fmt::Debug for ResolvedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedProvider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("api", &self.api)
            .field("base_url", &self.base_url)
            .field("keyless", &self.keyless)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("custom", &self.custom)
            .finish()
    }
}

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

const SECRET_PREFIX: &str = "secret:";

/// The secret id a provider's key is stored under: `api_key = "secret:<id>"`, else the provider id.
pub fn secret_id_for(provider_id: &str, config: &Config) -> String {
    let reference = config.providers.get(provider_id).and_then(|p| p.api_key.as_deref());
    match reference.and_then(|r| r.strip_prefix(SECRET_PREFIX)) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => provider_id.to_string(),
    }
}

/// Looks a provider up in the catalog, with `[providers.<id>].base_url`
/// overriding the endpoint. A provider that exists only in the config (it has a
/// `base_url`) is treated as an OpenAI-compatible endpoint whose key is optional.
/// Returns `None` for an unknown provider.
pub fn resolve_provider(id: &str, deps: &CloudDeps) -> Option<ResolvedProvider> {
    let entry = deps.catalog.provider(id);
    let settings = deps.config.providers.get(id);
    let base_url = settings
        .and_then(|s| s.base_url.as_deref())
        .or(entry.map(|e| e.base_url.as_str()))
        .filter(|url| !url.is_empty())?;
    let api_key = (deps.get_secret)(&secret_id_for(id, &deps.config))
        .map(|key| js_trim(&key).to_string())
        .filter(|key| !key.is_empty());
    Some(ResolvedProvider {
        id: id.to_string(),
        name: entry.map_or_else(|| id.to_string(), |e| e.name.clone()),
        api: entry.map_or(ProviderApi::Openai, |e| e.api),
        base_url: base_url.trim_end_matches('/').to_string(),
        keyless: entry.is_none_or(|e| e.keyless),
        api_key,
        custom: entry.is_none(),
    })
}

/// Request headers as name/value pairs, in the order they are sent.
pub type Headers = Vec<(&'static str, String)>;

/// Headers that authenticate a request to the provider; empty when it has no key.
pub fn auth_headers(provider: &ResolvedProvider) -> Headers {
    let Some(key) = &provider.api_key else {
        return Vec::new();
    };
    match provider.api {
        ProviderApi::Anthropic => {
            vec![("x-api-key", key.clone()), ("anthropic-version", ANTHROPIC_VERSION.to_string())]
        }
        ProviderApi::Elevenlabs => vec![("xi-api-key", key.clone())],
        ProviderApi::Deepgram => vec![("Authorization", format!("Token {key}"))],
        ProviderApi::Openai | ProviderApi::Mistral => vec![("Authorization", format!("Bearer {key}"))],
    }
}

/// Adds headers to a request.
pub(crate) fn with_headers(
    mut builder: reqwest::RequestBuilder,
    headers: impl IntoIterator<Item = (&'static str, String)>,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
}
