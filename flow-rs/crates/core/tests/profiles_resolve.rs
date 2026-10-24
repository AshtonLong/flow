//! PORT: tests/unit/profiles/resolve.test.ts

use flow_core::config::{validate_config, Config, DeviceSetting, InsertMethod, OverlayPosition, OverlaySize, OverlayStyle, Profile};
use flow_core::profiles::{match_profile, resolve_profile, ResolvedProfile};
use flow_core::types::FocusedApp;
use serde_json::{json, Value};

fn app(process_name: &str, title: &str) -> FocusedApp {
    FocusedApp {
        window_id: "1".into(),
        process_name: process_name.into(),
        title: title.into(),
        pid: 100,
        elevated: false,
    }
}

fn profile(name: &str, match_process: &[&str], match_title: &[&str]) -> Profile {
    Profile {
        name: name.into(),
        match_process: match_process.iter().map(|s| s.to_string()).collect(),
        match_title: match_title.iter().map(|s| s.to_string()).collect(),
        enabled: true,
        audio: None,
        model: None,
        cleanup: None,
        insert: None,
        overlay: None,
        history: None,
    }
}

fn config(raw: Value) -> Config {
    let result = validate_config(&raw);
    assert_eq!(result.issues, vec![]);
    result.config
}

fn matches_process(patterns: &[&str], focused: &FocusedApp) -> bool {
    match_profile(&[profile("P", patterns, &[])], Some(focused)).is_some()
}

fn matches_title(patterns: &[&str], focused: &FocusedApp) -> bool {
    match_profile(&[profile("P", &[], patterns)], Some(focused)).is_some()
}

mod match_profile_process {
    use super::*;

    #[test]
    fn matches_the_exe_name_case_insensitively_with_or_without_exe() {
        assert!(matches_process(&["slack.exe"], &app("slack.exe", "")));
        assert!(matches_process(&["Slack.EXE"], &app("slack.exe", "")));
        assert!(matches_process(&["slack"], &app("Slack.exe", "")));
        assert!(matches_process(&["slack.exe"], &app("slack", "")));
        assert!(matches_process(&["WindowsTerminal.exe"], &app("windowsterminal.exe", "")));
    }

    #[test]
    fn matches_the_whole_name_not_a_part_of_it() {
        assert!(!matches_process(&["slack"], &app("slack-helper.exe", "")));
        assert!(!matches_process(&["code"], &app("vscode.exe", "")));
        assert!(!matches_process(&["a.b"], &app("axb.exe", "")));
    }

    #[test]
    fn supports_wildcards() {
        assert!(matches_process(&["wezterm*"], &app("wezterm-gui.exe", "")));
        assert!(matches_process(&["*term*"], &app("WindowsTerminal.exe", "")));
        assert!(matches_process(&["*.game.exe"], &app("super.game.exe", "")));
        assert!(matches_process(&["code*.exe"], &app("Code - Insiders.exe", "")));
        assert!(!matches_process(&["wezterm*"], &app("alacritty.exe", "")));
        assert!(matches_process(&["*"], &app("anything.exe", "")));
    }

    #[test]
    fn treats_regex_characters_in_a_pattern_literally() {
        assert!(matches_process(&["c++(x)"], &app("c++(x).exe", "")));
        assert!(!matches_process(&["a+"], &app("aaa.exe", "")));
    }

    #[test]
    fn accepts_a_full_path_as_the_process_name() {
        assert!(matches_process(&["slack.exe"], &app("C:\\Apps\\Slack\\slack.exe", "")));
    }

    #[test]
    fn ignores_empty_patterns() {
        assert!(!matches_process(&[""], &app("slack.exe", "")));
        assert!(!matches_process(&["  "], &app("slack.exe", "")));
    }
}

mod match_profile_title {
    use super::*;

    #[test]
    fn matches_a_case_insensitive_substring() {
        assert!(matches_title(&["inbox"], &app("chrome.exe", "Inbox (3) - Gmail")));
        assert!(matches_title(&["GMAIL"], &app("chrome.exe", "Inbox (3) - Gmail")));
        assert!(!matches_title(&["outlook"], &app("chrome.exe", "Inbox (3) - Gmail")));
    }

    #[test]
    fn treats_pattern_flags_as_a_regex_with_exactly_those_flags() {
        let title = "main.ts - flow - Visual Studio Code";
        assert!(matches_title(&["/\\.tsx? - /"], &app("code.exe", title)));
        assert!(!matches_title(&["/visual studio/"], &app("code.exe", title)));
        assert!(matches_title(&["/visual studio/i"], &app("code.exe", title)));
        assert!(matches_title(&["/^main.*Code$/"], &app("code.exe", title)));
        assert!(!matches_title(&["/^flow/"], &app("code.exe", title)));
    }

    #[test]
    fn gives_the_same_answer_every_time_for_a_g_regex() {
        let profiles = [profile("P", &[], &["/code/gi"])];
        let focused = app("code.exe", "Visual Studio Code");
        for _ in 0..4 {
            assert!(match_profile(&profiles, Some(&focused)).is_some());
        }
    }

    #[test]
    fn falls_back_to_plain_text_for_an_invalid_regex() {
        assert!(matches_title(&["/(unclosed/"], &app("x.exe", "has /(unclosed/ in it")));
        assert!(!matches_title(&["/(unclosed/"], &app("x.exe", "something else")));
        // A path-like title pattern is not a regex either way.
        assert!(matches_title(&["C:/Users/"], &app("x.exe", "c:/users/me")));
    }

    #[test]
    fn ignores_empty_patterns() {
        assert!(!matches_title(&[""], &app("x.exe", "anything")));
    }
}

mod match_profile_choosing {
    use super::*;

    fn profiles() -> Vec<Profile> {
        vec![
            profile("Terminals", &["wt.exe", "wezterm-gui.exe"], &[]),
            profile("Mail", &["outlook.exe"], &["gmail"]),
            profile("Browsers", &["chrome.exe"], &[]),
            profile("Never", &[], &[]),
        ]
    }

    fn name(profiles: &[Profile], focused: &FocusedApp) -> Option<String> {
        match_profile(profiles, Some(focused)).map(|profile| profile.name.clone())
    }

    #[test]
    fn returns_null_without_a_focused_app_or_a_match() {
        let profiles = profiles();
        assert!(match_profile(&profiles, None).is_none());
        assert!(match_profile(&profiles, Some(&app("notepad.exe", "Untitled"))).is_none());
        assert!(match_profile(&[], Some(&app("wt.exe", ""))).is_none());
    }

    #[test]
    fn matches_a_profile_with_both_lists_if_either_matches() {
        let profiles = profiles();
        assert_eq!(name(&profiles, &app("outlook.exe", "Calendar")).as_deref(), Some("Mail"));
        assert_eq!(name(&profiles, &app("firefox.exe", "Inbox - Gmail")).as_deref(), Some("Mail"));
    }

    #[test]
    fn picks_the_first_match_in_file_order() {
        let profiles = profiles();
        assert_eq!(name(&profiles, &app("chrome.exe", "Inbox - Gmail")).as_deref(), Some("Mail"));
        assert_eq!(name(&profiles, &app("chrome.exe", "News")).as_deref(), Some("Browsers"));
        assert_eq!(name(&profiles, &app("wezterm-gui.exe", "gmail")).as_deref(), Some("Terminals"));
    }

    #[test]
    fn never_matches_a_profile_with_no_patterns() {
        assert!(match_profile(&[profile("Never", &[], &[])], Some(&app("x.exe", ""))).is_none());
    }
}

mod resolve_profile {
    use super::*;

    fn base() -> Config {
        config(json!({
            "audio": { "sounds": false, "max_recording_seconds": 120 },
            "model": { "active": "parakeet-tdt-0.6b-v2", "keep_loaded_minutes": 30 },
            "cleanup": { "llm_enabled": false, "llm_prompt": "clean", "llm_timeout_ms": 5000 },
            "insert": { "paste_shortcut": "Ctrl+V", "trailing_space": false },
            "overlay": { "style": "minimal" },
            "history": { "enabled": true, "retention_days": 90 },
            "profiles": [
                {
                    "name": "Terminals",
                    "match_process": ["WindowsTerminal.exe", "wezterm-gui.exe"],
                    "insert": { "paste_shortcut": "Ctrl+Shift+V" },
                    "cleanup": { "llm_enabled": false },
                },
                {
                    "name": "Slack",
                    "match_process": ["slack.exe"],
                    "model": { "active": "groq/whisper-large-v3-turbo" },
                    "cleanup": { "llm_enabled": true, "llm_prompt": "casual" },
                },
                { "name": "Vault", "match_title": ["/1password|bitwarden/i"], "enabled": false },
                {
                    "name": "Broken",
                    "match_process": ["broken.exe"],
                    "audio": { "max_recording_seconds": 1, "sounds": true },
                    "cleanup": { "llm_timeout_ms": "fast", "llm_enabled": true },
                    "overlay": { "style": "banner", "position": "top-left" },
                    "history": { "retention_days": 0, "enabled": "yes" },
                    "insert": { "method": "telepathy", "not_a_setting": 1 },
                    "model": { "active": "" },
                },
                { "name": "Plain", "match_process": ["plain.exe"] },
                {
                    "name": "Everything",
                    "match_process": ["all.exe"],
                    "audio": { "keep_mic_warm": true },
                    "model": { "device": "gpu" },
                    "cleanup": { "filler_removal": false },
                    "insert": { "method": "type" },
                    "overlay": { "size": "large" },
                    "history": { "enabled": false },
                },
            ],
        }))
    }

    #[test]
    fn returns_the_base_config_itself_when_nothing_matches() {
        let base = base();
        for focused in [None, Some(app("notepad.exe", "Untitled"))] {
            let resolved = resolve_profile(&base, focused.as_ref());
            assert_eq!(resolved, ResolvedProfile { config: base.clone(), profile: None, disabled: false });
        }
    }

    #[test]
    fn merges_overrides_shallowly_into_each_section() {
        let base = base();
        let resolved = resolve_profile(&base, Some(&app("WindowsTerminal.exe", "pwsh")));
        assert_eq!(resolved.profile.as_deref(), Some("Terminals"));
        assert!(!resolved.disabled);
        let mut insert = base.insert.clone();
        insert.paste_shortcut = "Ctrl+Shift+V".into();
        assert_eq!(resolved.config.insert, insert);
        assert_eq!(resolved.config.cleanup, base.cleanup);
        // Untouched sections keep the user's settings, not the schema defaults.
        assert_eq!(resolved.config.audio, base.audio);
        assert_eq!(resolved.config.history, base.history);
        assert_eq!(resolved.config.general, base.general);
        assert_eq!(resolved.config.profiles, base.profiles);
    }

    #[test]
    fn applies_overrides_from_several_sections() {
        let base = base();
        let resolved = resolve_profile(&base, Some(&app("slack.exe", "general")));
        assert_eq!(resolved.profile.as_deref(), Some("Slack"));
        let mut model = base.model.clone();
        model.active = "groq/whisper-large-v3-turbo".into();
        assert_eq!(resolved.config.model, model);
        let mut cleanup = base.cleanup.clone();
        cleanup.llm_enabled = true;
        cleanup.llm_prompt = "casual".into();
        assert_eq!(resolved.config.cleanup, cleanup);
        assert_eq!(resolved.config.insert, base.insert);
    }

    #[test]
    fn covers_every_overridable_section() {
        let effective = resolve_profile(&base(), Some(&app("all.exe", ""))).config;
        assert!(effective.audio.keep_mic_warm);
        assert_eq!(effective.model.device, DeviceSetting::Gpu);
        assert!(!effective.cleanup.filler_removal);
        assert_eq!(effective.insert.method, InsertMethod::Type);
        assert_eq!(effective.overlay.size, OverlaySize::Large);
        assert!(!effective.history.enabled);
        assert_eq!(effective.history.retention_days, 90);
    }

    #[test]
    fn reports_a_profile_that_disables_dictation() {
        let base = base();
        let resolved = resolve_profile(&base, Some(&app("chrome.exe", "Bitwarden Web Vault")));
        assert_eq!(resolved, ResolvedProfile { config: base.clone(), profile: Some("Vault".into()), disabled: true });
    }

    #[test]
    fn returns_the_base_settings_for_a_profile_with_no_overrides() {
        let base = base();
        let resolved = resolve_profile(&base, Some(&app("plain.exe", "")));
        assert_eq!(resolved, ResolvedProfile { config: base.clone(), profile: Some("Plain".into()), disabled: false });
    }

    #[test]
    fn drops_an_invalid_override_and_keeps_the_base_value_not_the_schema_default() {
        let base = base();
        let resolved = resolve_profile(&base, Some(&app("broken.exe", "")));
        assert_eq!(resolved.profile.as_deref(), Some("Broken"));
        let effective = resolved.config;
        // Invalid: base value (120), not the default (300).
        assert_eq!(effective.audio.max_recording_seconds, 120);
        assert_eq!(effective.cleanup.llm_timeout_ms, 5000);
        assert_eq!(effective.overlay.style, OverlayStyle::Minimal);
        assert_eq!(effective.history.retention_days, 90);
        assert!(effective.history.enabled);
        assert_eq!(effective.insert.method, InsertMethod::Paste);
        assert_eq!(effective.model.active, "parakeet-tdt-0.6b-v2");
        // Valid overrides in the same sections still apply.
        assert!(effective.audio.sounds);
        assert!(effective.cleanup.llm_enabled);
        assert_eq!(effective.overlay.position, OverlayPosition::TopLeft);
        // Unknown keys do not leak into the config.
        assert_eq!(effective.insert, base.insert);
        assert_eq!(effective.to_value()["insert"].get("not_a_setting"), None);
    }

    #[test]
    fn does_not_modify_the_base_config() {
        let base = base();
        let before = base.clone();
        resolve_profile(&base, Some(&app("slack.exe", "")));
        resolve_profile(&base, Some(&app("broken.exe", "")));
        assert_eq!(base, before);
    }

    #[test]
    fn resolves_the_spec_example_a_terminal_pastes_with_ctrl_shift_v_and_skips_cleanup() {
        let spec = config(json!({
            "cleanup": { "llm_enabled": true },
            "profiles": [
                {
                    "name": "Terminals",
                    "match_process": ["WindowsTerminal.exe", "wezterm-gui.exe"],
                    "insert": { "paste_shortcut": "Ctrl+Shift+V" },
                    "cleanup": { "llm_enabled": false },
                },
            ],
        }));
        let effective = resolve_profile(&spec, Some(&app("wezterm-gui.exe", "zsh"))).config;
        assert_eq!(effective.insert.paste_shortcut, "Ctrl+Shift+V");
        assert!(!effective.cleanup.llm_enabled);
        assert!(resolve_profile(&spec, Some(&app("notepad.exe", ""))).config.cleanup.llm_enabled);
    }
}
