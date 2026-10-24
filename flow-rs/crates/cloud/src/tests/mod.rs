//! Ports of `tests/unit/transcribe/cloud.test.ts` and `tests/unit/cleanup/llm.test.ts`.
//! The TS tests inject `fetch`; here every request goes to a local test server
//! (see `server.rs`), so the adapters' real request-building and HTTP code run.

mod cloud;
mod llm;
mod server;

use std::sync::Arc;

use flow_core::catalog::{ProviderApi, ProviderEntry};
use flow_core::config::Config;
use serde_json::Value;

use crate::CloudDeps;

/// A secret store with the test keys.
fn secrets(id: &str) -> Option<String> {
    let key = match id {
        "groq" => "gsk-test",
        "openai" => "sk-test",
        "mistral" => "mistral-test",
        "elevenlabs" => "xi-test",
        "deepgram" => "dg-test",
        "anthropic" => "sk-ant-test",
        "lan-key" => "lan-secret",
        _ => return None,
    };
    Some(key.to_string())
}

fn provider(id: &str, name: &str, api: ProviderApi, base_url: String) -> ProviderEntry {
    ProviderEntry { id: id.into(), name: name.into(), api, base_url, key_url: None, keyless: false }
}

/// The TS test catalog's providers, with each real base URL mapped to a path on
/// the test server (`https://api.groq.com/openai/v1` → `/groq/openai/v1`).
fn providers(server: &server::Server) -> Vec<ProviderEntry> {
    vec![
        provider("groq", "Groq", ProviderApi::Openai, server.url("/groq/openai/v1")),
        provider("openai", "OpenAI", ProviderApi::Openai, server.url("/openai/v1")),
        provider("mistral", "Mistral", ProviderApi::Mistral, server.url("/mistral/v1")),
        provider("elevenlabs", "ElevenLabs", ProviderApi::Elevenlabs, server.url("/elevenlabs/v1")),
        provider("deepgram", "Deepgram", ProviderApi::Deepgram, server.url("/deepgram/v1")),
        provider("anthropic", "Anthropic", ProviderApi::Anthropic, server.url("/anthropic/v1")),
        ProviderEntry { keyless: true, ..provider("ollama", "Ollama", ProviderApi::Openai, server.url("/ollama/v1")) },
    ]
}

/// A config from overrides, like `ConfigSchema.parse(overrides)` in the TS tests.
fn config(overrides: Value) -> Config {
    serde_json::from_value(overrides).expect("a valid test config")
}

fn http_client() -> reqwest::Client {
    // No system proxy: requests must reach the local test server directly.
    reqwest::Client::builder().no_proxy().build().expect("a test HTTP client")
}

fn deps_with(catalog: flow_core::catalog::Catalog, overrides: Value) -> CloudDeps {
    CloudDeps {
        catalog: Arc::new(catalog),
        config: Arc::new(config(overrides)),
        get_secret: Arc::new(secrets),
        http: http_client(),
    }
}

/// The same deps with another secret lookup.
fn with_secrets(deps: CloudDeps, get_secret: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> CloudDeps {
    CloudDeps { get_secret: Arc::new(get_secret), ..deps }
}
