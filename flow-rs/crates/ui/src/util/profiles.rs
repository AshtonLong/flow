//! Per-app profile helpers: tri-state overrides, tidy serialisation, starters.
//! Port of `lib/profiles.ts`.

use flow_core::catalog::CatalogSnapshot;
use flow_core::config::{PROFILE_SECTIONS, Profile};
use serde_json::{Map, Value, json};

use super::format::prompt_label;
use super::models::model_label;

fn section_mut<'a>(profile: &'a mut Profile, section: &str) -> Option<&'a mut Option<Map<String, Value>>> {
    match section {
        "audio" => Some(&mut profile.audio),
        "model" => Some(&mut profile.model),
        "cleanup" => Some(&mut profile.cleanup),
        "insert" => Some(&mut profile.insert),
        "overlay" => Some(&mut profile.overlay),
        "history" => Some(&mut profile.history),
        _ => None,
    }
}

pub fn get_override<'a>(profile: &'a Profile, section: &str, key: &str) -> Option<&'a Value> {
    profile.section(section)?.get(key)
}

/// Sets or (with `None`) clears one override, dropping the section when it empties.
pub fn set_override(profile: &Profile, section: &str, key: &str, value: Option<Value>) -> Profile {
    let mut next = profile.clone();
    if let Some(slot) = section_mut(&mut next, section) {
        let mut current = slot.clone().unwrap_or_default();
        match value {
            Some(value) => {
                current.insert(key.to_string(), value);
            }
            None => {
                current.shift_remove(key);
            }
        }
        *slot = if current.is_empty() { None } else { Some(current) };
    }
    next
}

/// Omits unset keys so the file holds only what the profile actually overrides.
pub fn serialize_profile(profile: &Profile) -> Value {
    let mut out = Map::new();
    out.insert("name".into(), json!(profile.name));
    if !profile.match_process.is_empty() {
        out.insert("match_process".into(), json!(profile.match_process));
    }
    if !profile.match_title.is_empty() {
        out.insert("match_title".into(), json!(profile.match_title));
    }
    if !profile.enabled {
        out.insert("enabled".into(), json!(false));
    }
    for section in PROFILE_SECTIONS {
        if let Some(overrides) = profile.section(section)
            && !overrides.is_empty()
        {
            out.insert(section.into(), Value::Object(overrides.clone()));
        }
    }
    Value::Object(out)
}

pub fn serialize_profiles(profiles: &[Profile]) -> Value {
    Value::Array(profiles.iter().map(serialize_profile).collect())
}

/// Override keys the editor has pickers for; anything else is listed as "set in the file".
fn editable(section: &str) -> &'static [&'static str] {
    match section {
        "model" => &["active"],
        "cleanup" => &["llm_enabled", "llm_prompt", "filler_removal"],
        "insert" => &["method", "paste_shortcut"],
        _ => &[],
    }
}

pub fn other_overrides(profile: &Profile) -> Vec<String> {
    let mut keys = Vec::new();
    for section in PROFILE_SECTIONS {
        for key in profile.section(section).into_iter().flat_map(|m| m.keys()) {
            if !editable(section).contains(&key.as_str()) {
                keys.push(format!("{section}.{key}"));
            }
        }
    }
    keys
}

pub fn match_summary(profile: &Profile) -> String {
    let mut parts = Vec::new();
    if !profile.match_process.is_empty() {
        parts.push(profile.match_process.join(", "));
    }
    if !profile.match_title.is_empty() {
        let titles: Vec<String> = profile.match_title.iter().map(|t| format!("“{t}”")).collect();
        parts.push(format!("title contains {}", titles.join(" or ")));
    }
    if parts.is_empty() { "Matches no app yet".into() } else { parts.join("; ") }
}

pub fn override_summary(profile: &Profile, catalog: Option<&CatalogSnapshot>) -> String {
    if !profile.enabled {
        return "Dictation is off in this app".into();
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(Value::String(model)) = get_override(profile, "model", "active") {
        parts.push(model_label(catalog, model));
    }
    let llm = get_override(profile, "cleanup", "llm_enabled").and_then(Value::as_bool);
    let prompt = get_override(profile, "cleanup", "llm_prompt").and_then(Value::as_str);
    if llm == Some(false) {
        parts.push("AI cleanup off".into());
    } else if llm == Some(true) && prompt.is_none() {
        parts.push("AI cleanup on".into());
    }
    if let Some(prompt) = prompt
        && llm != Some(false)
    {
        parts.push(format!("{} prompt", prompt_label(prompt)));
    }
    match get_override(profile, "cleanup", "filler_removal").and_then(Value::as_bool) {
        Some(false) => parts.push("Keeps filler words".into()),
        Some(true) => parts.push("Removes filler words".into()),
        None => {}
    }
    match get_override(profile, "insert", "method").and_then(Value::as_str) {
        Some("type") => parts.push("Types instead of pasting".into()),
        Some("paste") => parts.push("Pastes".into()),
        _ => {}
    }
    if let Some(shortcut) = get_override(profile, "insert", "paste_shortcut").and_then(Value::as_str) {
        parts.push(format!("Pastes with {shortcut}"));
    }
    let others = other_overrides(profile).len();
    if others > 0 {
        parts.push(format!("{others} more set in the file"));
    }
    if parts.is_empty() { "No overrides yet".into() } else { parts.join(", ") }
}

pub struct Starter {
    pub id: &'static str,
    pub description: &'static str,
    pub profile: Profile,
}

fn table(value: Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The three one-click starters from the spec.
pub fn starters() -> Vec<Starter> {
    let blank = |name: &str, processes: &[&str], enabled: bool| Profile {
        name: name.into(),
        match_process: names(processes),
        match_title: Vec::new(),
        enabled,
        audio: None,
        model: None,
        cleanup: None,
        insert: None,
        overlay: None,
        history: None,
    };
    vec![
        Starter {
            id: "terminals",
            description: "Pastes with Ctrl+Shift+V and skips AI cleanup",
            profile: Profile {
                insert: table(json!({ "paste_shortcut": "Ctrl+Shift+V" })),
                cleanup: table(json!({ "llm_enabled": false })),
                ..blank("Terminals", &["WindowsTerminal.exe", "wezterm-gui.exe", "alacritty.exe"], true)
            },
        },
        Starter {
            id: "editors",
            description: "Code comment prompt, keeps filler words",
            profile: Profile {
                cleanup: table(json!({ "llm_prompt": "code_comment", "filler_removal": false })),
                ..blank("Code editors", &["Code.exe", "Cursor.exe", "devenv.exe", "idea64.exe"], true)
            },
        },
        Starter {
            id: "passwords",
            description: "Turns dictation off",
            profile: blank("Password managers", &["1Password.exe", "Bitwarden.exe", "KeePassXC.exe"], false),
        },
    ]
}

/// `New profile`, `New profile 2`, … not already used.
pub fn unique_name(existing: &[Profile], wanted: &str) -> String {
    let taken = |name: &str| existing.iter().any(|p| p.name.to_lowercase() == name.to_lowercase());
    if !taken(wanted) {
        return wanted.to_string();
    }
    (2..).map(|n| format!("{wanted} {n}")).find(|c| !taken(c)).unwrap_or_else(|| wanted.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flow_core::config::validate_config;

    fn slack() -> Profile {
        Profile {
            name: "Slack".into(),
            match_process: vec!["slack.exe".into()],
            match_title: vec![],
            enabled: true,
            audio: None,
            model: None,
            cleanup: table(json!({ "llm_enabled": true, "llm_prompt": "casual" })),
            insert: None,
            overlay: table(json!({ "style": "minimal" })),
            history: None,
        }
    }

    #[test]
    fn omits_defaults_and_empty_sections_when_serialising() {
        let empty = Profile { name: "Empty".into(), match_process: vec![], cleanup: None, overlay: None, ..slack() };
        assert_eq!(serialize_profile(&empty), json!({ "name": "Empty" }));
        assert_eq!(
            serialize_profile(&Profile { enabled: false, ..slack() }),
            json!({
                "name": "Slack",
                "match_process": ["slack.exe"],
                "enabled": false,
                "cleanup": { "llm_enabled": true, "llm_prompt": "casual" },
                "overlay": { "style": "minimal" },
            })
        );
    }

    #[test]
    fn sets_and_clears_overrides_dropping_emptied_sections() {
        let slack = slack();
        let with_model = set_override(&slack, "model", "active", Some(json!("groq/whisper-large-v3-turbo")));
        assert_eq!(with_model.model, table(json!({ "active": "groq/whisper-large-v3-turbo" })));
        assert_eq!(set_override(&with_model, "model", "active", None).model, None);
        let cleared = set_override(&slack, "cleanup", "llm_prompt", None);
        assert_eq!(cleared.cleanup, table(json!({ "llm_enabled": true })));
        assert_eq!(slack.cleanup, table(json!({ "llm_enabled": true, "llm_prompt": "casual" })));
    }

    #[test]
    fn lists_overrides_the_editor_has_no_picker_for() {
        assert_eq!(other_overrides(&slack()), vec!["overlay.style".to_string()]);
    }

    #[test]
    fn summarises_matches_and_overrides() {
        assert_eq!(match_summary(&slack()), "slack.exe");
        let by_title = Profile { match_process: vec![], match_title: vec!["Gmail".into()], ..slack() };
        assert_eq!(match_summary(&by_title), "title contains “Gmail”");
        assert_eq!(override_summary(&slack(), None), "Casual prompt, 1 more set in the file");
        assert_eq!(override_summary(&Profile { enabled: false, ..slack() }, None), "Dictation is off in this app");
    }

    #[test]
    fn ships_starters_that_survive_config_validation() {
        let profiles: Vec<Profile> = starters().into_iter().map(|s| s.profile).collect();
        let validated = validate_config(&json!({ "profiles": serialize_profiles(&profiles) }));
        assert!(validated.issues.is_empty());
        let names: Vec<&str> = validated.config.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Terminals", "Code editors", "Password managers"]);
        assert!(!validated.config.profiles[2].enabled);
        assert_eq!(validated.config.profiles[0].insert, table(json!({ "paste_shortcut": "Ctrl+Shift+V" })));
    }

    #[test]
    fn picks_a_free_name() {
        assert_eq!(unique_name(&[slack()], "New profile"), "New profile");
        assert_eq!(unique_name(&[slack()], "slack"), "slack 2");
    }
}
