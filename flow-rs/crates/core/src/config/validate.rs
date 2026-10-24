//! Lenient validation of a raw config value. PORT: src/shared/config.ts `validateConfig`.

use serde_json::Value;

use super::{Config, ConfigIssue};

/// A validated config and the problems found on the way.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    pub config: Config,
    pub issues: Vec<ConfigIssue>,
}

/// Validates leniently: a bad key is dropped (so it falls back to its default)
/// and reported, instead of rejecting the whole file. Never panics.
pub fn validate_config(raw: &Value) -> Validated {
    // Placeholder until the port lands: strict parse, else defaults.
    let config = serde_json::from_value(raw.clone()).unwrap_or_default();
    Validated { config, issues: Vec::new() }
}
