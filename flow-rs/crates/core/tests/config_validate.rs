//! PORT: tests/unit/shared/config.test.ts

use flow_core::config::{Config, ConfigIssue, PathSeg, ProviderConfig, prompt_names, resolve_prompt, validate_config};
use flow_core::path;
use serde_json::{Value, json};

fn joined(path: &[PathSeg]) -> String {
    path.iter().map(ToString::to_string).collect::<Vec<_>>().join(".")
}

mod validate_config {
    use super::*;

    #[test]
    fn accepts_an_empty_config_with_all_defaults() {
        let result = validate_config(&json!({}));
        assert_eq!(result.issues, vec![]);
        assert_eq!(result.config, Config::default());
        assert_eq!(result.config.hotkeys.hold_to_talk, "Ctrl+Win");
        assert_eq!(result.config.model.active, "parakeet-tdt-0.6b-v2");
        assert!(!result.config.history.enabled);
        assert!(!result.config.cleanup.llm_enabled);
    }

    #[test]
    fn drops_a_bad_key_to_its_default_and_reports_it_keeping_the_rest() {
        let result = validate_config(&json!({
            "audio": { "max_recording_seconds": "forever", "sounds": false },
            "overlay": { "style": "banner" },
        }));
        assert_eq!(result.config.audio.max_recording_seconds, 300);
        assert!(!result.config.audio.sounds);
        assert_eq!(result.config.overlay.style, flow_core::config::OverlayStyle::Pill);
        let mut paths: Vec<String> = result.issues.iter().map(|issue| joined(&issue.path)).collect();
        paths.sort();
        assert_eq!(paths, vec!["audio.max_recording_seconds", "overlay.style"]);
    }

    #[test]
    fn drops_an_invalid_list_item_without_losing_its_siblings() {
        let result = validate_config(&json!({
            "dictionary": [
                { "heard": ["tory"], "write": "Tauri" },
                { "heard": [], "write": "Nope" },
                { "heard": ["pee npm"], "write": "pnpm" },
            ],
        }));
        let writes: Vec<&str> = result.config.dictionary.iter().map(|entry| entry.write.as_str()).collect();
        assert_eq!(writes, vec!["Tauri", "pnpm"]);
        assert_eq!(result.issues.len(), 1);
    }

    #[test]
    fn never_throws_on_nonsense_input() {
        assert_eq!(validate_config(&Value::Null).config, Config::default());
        assert_eq!(validate_config(&json!("x")).config, Config::default());
        assert_eq!(validate_config(&json!({ "general": 5, "hotkeys": [] })).config, Config::default());
    }

    #[test]
    fn accepts_the_profile_overrides_from_the_spec_example() {
        let result = validate_config(&json!({
            "profiles": [
                {
                    "name": "Terminals",
                    "match_process": ["WindowsTerminal.exe"],
                    "insert": { "paste_shortcut": "Ctrl+Shift+V" },
                    "cleanup": { "llm_enabled": false },
                },
            ],
            "providers": { "groq": { "api_key": "secret:groq" } },
        }));
        assert_eq!(result.issues, vec![]);
        assert_eq!(
            result.config.profiles[0].insert.as_ref().map(|map| Value::Object(map.clone())),
            Some(json!({ "paste_shortcut": "Ctrl+Shift+V" }))
        );
    }

    #[test]
    fn rejects_a_raw_api_key_in_the_file() {
        let result = validate_config(&json!({ "providers": { "groq": { "api_key": "gsk_live_abc" } } }));
        assert_eq!(result.config.providers.get("groq").and_then(|provider| provider.api_key.clone()), None);
        assert_eq!(result.issues.len(), 1);
    }
}

mod prompts {
    use super::*;

    #[test]
    fn falls_back_to_built_in_templates_and_lets_the_file_override_them() {
        assert!(resolve_prompt(&Config::default(), "email").to_lowercase().contains("email"));
        let config = validate_config(&json!({
            "prompts": { "email": { "text": "Mine." }, "pirate": { "text": "Arr." } },
        }))
        .config;
        assert_eq!(resolve_prompt(&config, "email"), "Mine.");
        assert_eq!(prompt_names(&config), vec!["clean", "email", "casual", "code_comment", "verbatim", "pirate"]);
        assert_eq!(resolve_prompt(&config, "missing"), resolve_prompt(&Config::default(), "clean"));
    }
}

/// Not in the TS suite: pins the messages zod v4 prints, which the settings
/// window shows next to each problem.
mod zod_messages {
    use super::*;

    fn issues(raw: Value) -> Vec<(String, String)> {
        validate_config(&raw).issues.into_iter().map(|issue| (joined(&issue.path), issue.message)).collect()
    }

    fn one(raw: Value) -> String {
        let found = issues(raw);
        assert_eq!(found.len(), 1, "{found:?}");
        found[0].1.clone()
    }

    #[test]
    fn reports_type_and_range_problems() {
        assert_eq!(one(json!({ "audio": { "sounds": "loud" } })), "Invalid input: expected boolean, received string");
        assert_eq!(one(json!({ "audio": { "max_recording_seconds": 2 } })), "Too small: expected number to be >=5");
        assert_eq!(one(json!({ "audio": { "max_recording_seconds": 3601 } })), "Too big: expected number to be <=3600");
        assert_eq!(
            one(json!({ "audio": { "max_recording_seconds": 2.5 } })),
            "Invalid input: expected int, received number"
        );
        assert_eq!(one(json!({ "model": { "keep_loaded_minutes": -2 } })), "Too small: expected number to be >=-1");
        assert_eq!(
            one(json!({ "model": { "keep_loaded_minutes": [] } })),
            "Invalid input: expected number, received array"
        );
        assert_eq!(one(json!({ "version": 1.5 })), "Invalid input: expected int, received number");
        assert_eq!(one(json!({ "version": 1e300 })), "Too big: expected int to be <=9007199254740991");
        assert_eq!(
            one(json!({ "general": { "theme": "blue" } })),
            "Invalid option: expected one of \"system\"|\"light\"|\"dark\""
        );
        assert_eq!(one(json!({ "general": { "language": "fr" } })), "Invalid input: expected \"en\"");
        assert_eq!(one(json!({ "general": 5 })), "Invalid input: expected object, received number");
        assert_eq!(one(json!({ "hotkeys": [] })), "Invalid input: expected object, received array");
        assert_eq!(one(json!({ "model": { "active": "" } })), "Too small: expected string to have >=1 characters");
        assert_eq!(one(json!({ "providers": [] })), "Invalid input: expected record, received array");
        assert_eq!(one(json!({ "dictionary": {} })), "Invalid input: expected array, received object");
    }

    #[test]
    fn reports_string_formats() {
        assert_eq!(one(json!({ "providers": { "x": { "base_url": "not a url" } } })), "Invalid URL");
        assert_eq!(
            one(json!({ "providers": { "x": { "api_key": "gsk" } } })),
            "Invalid string: must start with \"secret:\""
        );
        // A valid URL is trimmed, as zod does.
        let config = validate_config(&json!({ "providers": { "x": { "base_url": " http://a/v1 " } } })).config;
        assert_eq!(
            config.providers.get("x"),
            Some(&ProviderConfig { base_url: Some("http://a/v1".into()), api_key: None, model: None })
        );
    }

    #[test]
    fn reports_list_items_by_their_first_problem() {
        assert_eq!(
            issues(json!({ "dictionary": [{ "heard": [] , "write": "" }, { "heard": ["a"] }, "x"] })),
            vec![
                ("dictionary.0.heard".to_string(), "Too small: expected array to have >=1 items".to_string()),
                ("dictionary.0.write".to_string(), "Too small: expected string to have >=1 characters".to_string()),
                ("dictionary.1.write".to_string(), "Invalid input: expected string, received undefined".to_string()),
                ("dictionary.2".to_string(), "Invalid input: expected object, received string".to_string()),
                // Both issues of item 0 drop an item, so "x" shifted to index 0
                // and is reported again on the next pass, as in the TS.
                ("dictionary.0".to_string(), "Invalid input: expected object, received string".to_string()),
            ]
        );
        assert_eq!(
            one(json!({ "profiles": [{ "name": "A", "audio": 5 }] })),
            "Invalid input: expected record, received number"
        );
    }

    #[test]
    fn strips_unknown_keys_silently() {
        let result = validate_config(&json!({ "audio": { "sounds": false, "bogus": 1 }, "nope": {} }));
        assert_eq!(result.issues, Vec::<ConfigIssue>::new());
        assert!(!result.config.audio.sounds);
        let value = result.config.to_value();
        assert_eq!(value.get("nope"), None);
        assert_eq!(value["audio"].get("bogus"), None);
    }

    #[test]
    fn keeps_the_issue_paths_of_the_typescript_retry_loop() {
        // Two bad items: the lower index goes first and shifts the higher one,
        // which is then reported again under its new index, as in the TS.
        let result = validate_config(&json!({
            "snippets": [{ "trigger": "" , "text": "a" }, { "trigger": "b", "text": "b" }, { "trigger": "", "text": "c" }],
        }));
        let paths: Vec<String> = result.issues.iter().map(|issue| joined(&issue.path)).collect();
        assert_eq!(paths, vec!["snippets.0.trigger", "snippets.2.trigger", "snippets.1.trigger"]);
        let texts: Vec<&str> = result.config.snippets.iter().map(|snippet| snippet.text.as_str()).collect();
        assert_eq!(texts, vec!["b"]);
        assert_eq!(result.issues[0].path, path!["snippets", 0usize, "trigger"]);
    }
}
