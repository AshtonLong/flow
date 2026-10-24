//! The model catalog manifest: local models, cloud models, cleanup models and
//! providers. Mirrors `src/shared/catalog.ts`. Field names are camelCase, as in
//! `resources/catalog.json`.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// How a provider's HTTP API is spoken. One adapter exists per value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderApi {
    Openai,
    Mistral,
    Elevenlabs,
    Deepgram,
    Anthropic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEntry {
    pub id: String,
    pub name: String,
    pub api: ProviderApi,
    /// Base URL without a trailing slash, e.g. `https://api.groq.com/openai/v1`.
    pub base_url: String,
    /// Where the user creates an API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_url: Option<String>,
    /// Needs no key (local endpoints such as Ollama).
    #[serde(default, skip_serializing_if = "is_false")]
    pub keyless: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunsOn {
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Core,
    Accuracy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelEntry {
    pub id: String,
    pub name: String,
    pub family: String,
    /// "Pick it for…" line shown in the catalog.
    pub description: String,
    pub url: String,
    /// File name under the models directory.
    pub file: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub languages: Vec<String>,
    pub licence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_url: Option<String>,
    /// Hardware the model is practical on.
    pub runs_on: RunsOn,
    /// `core` ships in v1; `accuracy` is the large GPU tier.
    pub tier: Tier,
    /// Average WER on the Open ASR Leaderboard, in percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wer: Option<f64>,
    /// The user must accept terms on the model page before downloading.
    #[serde(default, skip_serializing_if = "is_false")]
    pub gated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudModelEntry {
    /// `<provider>/<model>`, e.g. `groq/whisper-large-v3-turbo`.
    pub id: String,
    pub provider: String,
    /// Model name sent to the provider.
    pub model: String,
    pub name: String,
    pub description: String,
    /// USD per hour of audio, list price.
    pub price_per_hour: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recommended: bool,
    /// Supports dictionary terms as a hint.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hints: bool,
    /// Minimum billed seconds per request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_billed_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupModelEntry {
    /// `<provider>/<model>`, e.g. `groq/openai/gpt-oss-20b`.
    pub id: String,
    pub provider: String,
    pub model: String,
    pub name: String,
    pub description: String,
    /// USD per million tokens.
    pub price_in: f64,
    pub price_out: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub version: i64,
    /// Date the prices were read, `YYYY-MM-DD`.
    pub as_of: String,
    pub providers: Vec<ProviderEntry>,
    pub local: Vec<LocalModelEntry>,
    pub cloud: Vec<CloudModelEntry>,
    pub cleanup: Vec<CleanupModelEntry>,
}

impl Catalog {
    pub fn provider(&self, id: &str) -> Option<&ProviderEntry> {
        self.providers.iter().find(|p| p.id == id)
    }
    pub fn local_entry(&self, id: &str) -> Option<&LocalModelEntry> {
        self.local.iter().find(|m| m.id == id)
    }
    pub fn cloud_entry(&self, id: &str) -> Option<&CloudModelEntry> {
        self.cloud.iter().find(|m| m.id == id)
    }
    pub fn cleanup_entry(&self, id: &str) -> Option<&CleanupModelEntry> {
        self.cleanup.iter().find(|m| m.id == id)
    }
}

/// A local or cloud speech model, tagged with `kind` like the manifest entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ModelEntry {
    Local(LocalModelEntry),
    Cloud(CloudModelEntry),
}

impl ModelEntry {
    pub fn id(&self) -> &str {
        match self {
            Self::Local(m) => &m.id,
            Self::Cloud(m) => &m.id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Self::Local(m) => &m.name,
            Self::Cloud(m) => &m.name,
        }
    }
    pub fn is_cloud(&self) -> bool {
        matches!(self, Self::Cloud(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Downloading,
    Verifying,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub id: String,
    pub received_bytes: u64,
    pub total_bytes: u64,
    /// Bytes per second, smoothed.
    pub speed: f64,
    pub state: DownloadState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Runtime status of one model, merged with its catalog entry for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub id: String,
    /// Local: file present and verified. Cloud: the provider has a key (or needs none).
    pub ready: bool,
    pub active: bool,
    /// Local model currently held in engine memory.
    pub loaded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<DownloadProgress>,
    /// Registered by the user in `[[local_models]]` or as a custom endpoint.
    #[serde(default, skip_serializing_if = "is_false")]
    pub custom: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub catalog: Catalog,
    pub status: IndexMap<String, ModelStatus>,
    /// Provider id → has a stored key.
    pub keys: IndexMap<String, bool>,
    /// User-registered entries (custom local files and custom endpoints).
    pub custom: Vec<ModelEntry>,
}

/// Prefix of model ids served by a custom OpenAI-compatible endpoint: `custom/<provider-id>`.
pub const CUSTOM_PREFIX: &str = "custom/";

/// Splits `groq/openai/gpt-oss-20b` into provider `groq` and model `openai/gpt-oss-20b`.
pub fn split_model_id(id: &str) -> Option<(&str, &str)> {
    let slash = id.find('/')?;
    if slash == 0 {
        return None;
    }
    Some((&id[..slash], &id[slash + 1..]))
}

/// Local model ids contain no slash; cloud ids are `<provider>/<model>`.
pub fn is_cloud_model_id(id: &str) -> bool {
    id.contains('/')
}

/// Cost in USD of transcribing `audio_ms` of audio on a cloud model.
pub fn cloud_cost(entry: &CloudModelEntry, audio_ms: f64) -> f64 {
    let seconds = (audio_ms / 1000.0).max(entry.min_billed_seconds.unwrap_or(0.0));
    seconds / 3600.0 * entry.price_per_hour
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_model_ids() {
        assert_eq!(split_model_id("groq/openai/gpt-oss-20b"), Some(("groq", "openai/gpt-oss-20b")));
        assert_eq!(split_model_id("parakeet"), None);
        assert_eq!(split_model_id("/x"), None);
        assert!(is_cloud_model_id("groq/whisper-large-v3"));
        assert!(!is_cloud_model_id("parakeet-tdt-0.6b-v2"));
    }

    #[test]
    fn costs_respect_minimum_billing() {
        let entry = CloudModelEntry {
            id: "groq/x".into(),
            provider: "groq".into(),
            model: "x".into(),
            name: "X".into(),
            description: String::new(),
            price_per_hour: 0.04,
            recommended: false,
            hints: false,
            min_billed_seconds: Some(10.0),
            docs_url: None,
        };
        assert!((cloud_cost(&entry, 2000.0) - 10.0 / 3600.0 * 0.04).abs() < 1e-12);
        assert!((cloud_cost(&entry, 36_000_000.0) - 0.4).abs() < 1e-9);
    }
}
