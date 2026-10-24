//! Layer-1 text cleanup rules. PORT: src/main/cleanup/*.ts

use crate::config::Config;

/// Context for fitting the text to what is around the cursor.
#[derive(Debug, Clone, Default)]
pub struct RulesContext {}

pub fn apply_rules(text: &str, _config: &Config, _ctx: &RulesContext) -> String {
    text.to_string()
}

pub fn dictionary_hints(_config: &Config) -> Vec<String> {
    Vec::new()
}
