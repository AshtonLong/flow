//! Applies config patches to plain values, for optimistic updates and the mock
//! backend. Port of `lib/patch.ts`; the work is done by `flow_core::config::value`.

use flow_core::config::{Config, ConfigPatch, validate_config};

pub use flow_core::config::value::{apply_patches, format_path, get_at};

/// The config as the main process will see it after `patches`: patched, then
/// re-validated so defaults are filled in exactly as the main process fills them.
pub fn patched_config(config: &Config, patches: &[ConfigPatch]) -> Config {
    validate_config(&apply_patches(&config.to_value(), patches)).config
}

#[cfg(test)]
#[allow(clippy::useless_vec)] // `path![]` builds the `Vec` that `ConfigPath` is.
mod tests {
    use super::*;
    use flow_core::path;
    use serde_json::json;

    #[test]
    fn sets_a_nested_value_without_touching_the_original() {
        let config = Config::default();
        let next = patched_config(
            &config,
            &[ConfigPatch::set(path!["audio", "sounds"], false)],
        );
        assert!(!next.audio.sounds);
        assert!(config.audio.sounds);
    }

    #[test]
    fn creates_missing_tables_on_the_way_down() {
        let next = apply_patches(
            &json!({}),
            &[ConfigPatch::set(
                path!["prompts", "standup", "text"],
                "Short notes.",
            )],
        );
        assert_eq!(
            next,
            json!({ "prompts": { "standup": { "text": "Short notes." } } })
        );
    }

    #[test]
    fn removes_a_key_when_the_value_is_undefined() {
        let start = json!({ "prompts": { "clean": { "text": "x" }, "mine": { "text": "y" } } });
        let next = apply_patches(&start, &[ConfigPatch::remove(path!["prompts", "clean"])]);
        assert_eq!(next, json!({ "prompts": { "mine": { "text": "y" } } }));
    }

    #[test]
    fn splices_an_array_item_and_ignores_removals_under_a_missing_parent() {
        let start = json!({ "list": ["a", "b", "c"] });
        assert_eq!(
            apply_patches(&start, &[ConfigPatch::remove(path!["list", 1usize])]),
            json!({ "list": ["a", "c"] })
        );
        assert_eq!(
            apply_patches(&start, &[ConfigPatch::remove(path!["nope", "x"])]),
            start
        );
    }

    #[test]
    fn replaces_whole_arrays() {
        let next = patched_config(
            &Config::default(),
            &[ConfigPatch::set(
                path!["dictionary"],
                json!([{ "heard": ["tory"], "write": "Tauri" }]),
            )],
        );
        assert_eq!(next.dictionary.len(), 1);
        assert_eq!(next.dictionary[0].write, "Tauri");
    }

    #[test]
    fn joins_keys_with_dots_and_indexes_with_brackets() {
        assert_eq!(
            format_path(&path!["audio", "max_recording_seconds"]),
            "audio.max_recording_seconds"
        );
        assert_eq!(
            format_path(&path!["profiles", 1usize, "name"]),
            "profiles[1].name"
        );
    }
}
