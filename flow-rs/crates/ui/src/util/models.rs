//! Lookups over the catalog snapshot. Port of `lib/models.ts`.

use flow_core::catalog::{
    CUSTOM_PREFIX, CatalogSnapshot, CloudModelEntry, LocalModelEntry, ModelEntry, ProviderEntry,
    split_model_id,
};
use flow_core::config::Config;
use flow_core::types::BackendDevice;

/// Local, then cloud, then user-registered entries.
pub fn all_models(snapshot: &CatalogSnapshot) -> Vec<ModelEntry> {
    let local = snapshot
        .catalog
        .local
        .iter()
        .cloned()
        .map(ModelEntry::Local);
    let cloud = snapshot
        .catalog
        .cloud
        .iter()
        .cloned()
        .map(ModelEntry::Cloud);
    local
        .chain(cloud)
        .chain(snapshot.custom.iter().cloned())
        .collect()
}

pub fn find_model(snapshot: Option<&CatalogSnapshot>, id: &str) -> Option<ModelEntry> {
    let snapshot = snapshot?;
    if let Some(m) = snapshot.catalog.local.iter().find(|m| m.id == id) {
        return Some(ModelEntry::Local(m.clone()));
    }
    if let Some(m) = snapshot.catalog.cloud.iter().find(|m| m.id == id) {
        return Some(ModelEntry::Cloud(m.clone()));
    }
    snapshot.custom.iter().find(|m| m.id() == id).cloned()
}

pub fn find_provider<'a>(
    snapshot: Option<&'a CatalogSnapshot>,
    id: &str,
) -> Option<&'a ProviderEntry> {
    snapshot?.catalog.providers.iter().find(|p| p.id == id)
}

/// A name that stands on its own in a select or the status footer: `Groq: Whisper large-v3-turbo`.
pub fn model_label(snapshot: Option<&CatalogSnapshot>, id: &str) -> String {
    let Some(entry) = find_model(snapshot, id) else {
        return match id.strip_prefix(CUSTOM_PREFIX) {
            Some(rest) => format!("Custom: {rest}"),
            None => id.to_string(),
        };
    };
    match entry {
        ModelEntry::Local(m) => m.name,
        ModelEntry::Cloud(m) => match find_provider(snapshot, &m.provider).map(|p| p.name.as_str())
        {
            Some(provider) if !m.name.to_lowercase().starts_with(&provider.to_lowercase()) => {
                format!("{provider}: {}", m.name)
            }
            _ => m.name,
        },
    }
}

pub fn is_ready(snapshot: &CatalogSnapshot, id: &str) -> bool {
    snapshot.status.get(id).is_some_and(|s| s.ready)
}

/// Models that can transcribe right now: installed local, connected cloud, custom endpoints.
pub fn ready_models(snapshot: &CatalogSnapshot) -> Vec<ModelEntry> {
    all_models(snapshot)
        .into_iter()
        .filter(|m| is_ready(snapshot, m.id()))
        .collect()
}

pub fn installed_local(snapshot: &CatalogSnapshot) -> Vec<LocalModelEntry> {
    all_models(snapshot)
        .into_iter()
        .filter_map(|m| match m {
            ModelEntry::Local(local) if is_ready(snapshot, &local.id) => Some(local),
            _ => None,
        })
        .collect()
}

pub struct ProviderGroup<'a> {
    pub provider: &'a ProviderEntry,
    pub models: Vec<&'a CloudModelEntry>,
}

pub fn cloud_by_provider(snapshot: &CatalogSnapshot) -> Vec<ProviderGroup<'_>> {
    snapshot
        .catalog
        .providers
        .iter()
        .filter_map(|provider| {
            let models: Vec<&CloudModelEntry> = snapshot
                .catalog
                .cloud
                .iter()
                .filter(|m| m.provider == provider.id)
                .collect();
            (!models.is_empty()).then_some(ProviderGroup { provider, models })
        })
        .collect()
}

/// Whether a provider can be called: it has a stored key or needs none.
pub fn provider_connected(snapshot: &CatalogSnapshot, provider_id: &str) -> bool {
    if snapshot.keys.get(provider_id).copied().unwrap_or(false) {
        return true;
    }
    find_provider(Some(snapshot), provider_id).is_some_and(|p| p.keyless)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEndpoint {
    pub id: String,
    pub base_url: String,
    pub model: String,
    pub has_key_ref: bool,
}

/// Custom endpoints are `[providers.<id>]` tables with a `base_url` that are not catalog providers.
pub fn custom_endpoints(
    config: &Config,
    snapshot: Option<&CatalogSnapshot>,
) -> Vec<CustomEndpoint> {
    config
        .providers
        .iter()
        .filter(|(id, p)| {
            let known = snapshot.is_some_and(|s| s.catalog.providers.iter().any(|k| &k.id == *id));
            !known && p.base_url.as_deref().is_some_and(|u| !u.is_empty())
        })
        .map(|(id, p)| CustomEndpoint {
            id: id.clone(),
            base_url: p.base_url.clone().unwrap_or_default(),
            model: p.model.clone().unwrap_or_default(),
            has_key_ref: p.api_key.as_deref().is_some_and(|k| !k.is_empty()),
        })
        .collect()
}

pub fn provider_of_model(id: &str) -> Option<&str> {
    split_model_id(id).map(|(provider, _)| provider)
}

pub fn is_gpu_device(device: &BackendDevice) -> bool {
    let text = format!("{} {}", device.device_type, device.kind).to_lowercase();
    ["gpu", "cuda", "vulkan", "metal"]
        .iter()
        .any(|w| text.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flow_core::config::validate_config;
    use serde_json::json;

    #[test]
    fn lists_provider_tables_with_a_base_url_that_are_not_catalog_providers() {
        let config = validate_config(&json!({
            "providers": {
                "groq": { "api_key": "secret:groq" },
                "custom-lan": {
                    "base_url": "http://192.168.1.20:8000/v1",
                    "model": "whisper-1",
                    "api_key": "secret:custom-lan",
                },
            },
        }))
        .config;
        // Without a catalog every provider with a base URL counts; `groq` has none.
        assert_eq!(
            custom_endpoints(&config, None),
            vec![CustomEndpoint {
                id: "custom-lan".into(),
                base_url: "http://192.168.1.20:8000/v1".into(),
                model: "whisper-1".into(),
                has_key_ref: true,
            }]
        );
    }

    #[test]
    fn labels_unknown_models_by_id() {
        assert_eq!(model_label(None, "custom/lan"), "Custom: lan");
        assert_eq!(model_label(None, "parakeet"), "parakeet");
        assert!(is_gpu_device(&BackendDevice {
            name: "CUDA0".into(),
            kind: "cuda".into(),
            device_type: "gpu".into(),
            description: String::new(),
            memory_total: 0,
        }));
    }
}
