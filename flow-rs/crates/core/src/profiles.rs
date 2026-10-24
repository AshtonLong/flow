//! Per-app profiles: which profile matches the focused app, and the config that results.
//!
//! PORT: src/main/profiles/resolve.ts

use serde_json::Value;

use crate::config::{Config, PROFILE_SECTIONS, PathSeg, Profile, validate_config};
use crate::types::FocusedApp;

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProfile {
    /// Effective config: base config with the matching profile's overrides merged and re-validated.
    pub config: Config,
    /// Name of the matching profile, if any.
    pub profile: Option<String>,
    /// The profile sets `enabled = false`: dictation is off in this app.
    pub disabled: bool,
}

/// Lower-cased executable name without directory or `.exe`.
fn normalise_process(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let base = lower.rsplit(['\\', '/']).next().unwrap_or_default();
    base.strip_suffix(".exe").unwrap_or(base).to_string()
}

fn matches_process(pattern: &str, process_name: &str) -> bool {
    let wanted = normalise_process(pattern);
    if wanted.is_empty() {
        return false;
    }
    if !wanted.contains('*') {
        return wanted == process_name;
    }
    let source = wanted.split('*').map(regex::escape).collect::<Vec<_>>().join(".*");
    regex::Regex::new(&format!("^{source}$")).is_ok_and(|re| re.is_match(process_name))
}

/// Splits `/pattern/flags` into its parts, like `/^\/(.+)\/([a-z]*)$/s`.
fn regex_literal(pattern: &str) -> Option<(&str, &str)> {
    let inner = pattern.strip_prefix('/')?;
    let slash = inner.rfind('/')?;
    let (source, flags) = (&inner[..slash], &inner[slash + 1..]);
    (!source.is_empty() && flags.bytes().all(|b| b.is_ascii_lowercase())).then_some((source, flags))
}

/// Compiles a JavaScript regex with its flags, or `None` where `new RegExp`
/// would throw (or the pattern uses syntax this engine does not have).
fn compile_js_regex(source: &str, flags: &str) -> Option<fancy_regex::Regex> {
    let mut inline = String::new();
    let mut seen = String::new();
    for flag in flags.chars() {
        // A repeated or unknown flag is a SyntaxError in JavaScript.
        if seen.contains(flag) {
            return None;
        }
        seen.push(flag);
        match flag {
            'i' => inline.push('i'),
            'm' => inline.push('m'),
            's' => inline.push('s'),
            // Unicode mode is the default here; `d` only adds match indices.
            'u' | 'v' | 'd' => {}
            _ => return None,
        }
    }
    if seen.contains('u') && seen.contains('v') {
        return None;
    }
    let source = if inline.is_empty() { source.to_string() } else { format!("(?{inline}){source}") };
    fancy_regex::Regex::new(&source).ok()
}

fn matches_title(pattern: &str, title: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    if let Some((source, flags)) = regex_literal(pattern) {
        // `g` and `y` make `test` stateful and add nothing to a yes/no match.
        let flags: String = flags.chars().filter(|c| !matches!(c, 'g' | 'y')).collect();
        if let Some(regex) = compile_js_regex(source, &flags) {
            return regex.is_match(title).unwrap_or(false);
        }
        // Not a valid regex after all: fall through and treat it as plain text.
    }
    title.to_lowercase().contains(&pattern.to_lowercase())
}

/// The first profile, in file order, whose process or title patterns match the
/// focused app. A profile with both lists matches if either does.
pub fn match_profile<'a>(profiles: &'a [Profile], app: Option<&FocusedApp>) -> Option<&'a Profile> {
    let app = app?;
    let process_name = normalise_process(&app.process_name);
    profiles.iter().find(|profile| {
        profile.match_process.iter().any(|pattern| matches_process(pattern, &process_name))
            || profile.match_title.iter().any(|pattern| matches_title(pattern, &app.title))
    })
}

/// Applies the matching profile's overrides to `config`, section by section.
pub fn resolve_profile(config: &Config, app: Option<&FocusedApp>) -> ResolvedProfile {
    let Some(profile) = match_profile(&config.profiles, app) else {
        return ResolvedProfile { config: config.clone(), profile: None, disabled: false };
    };
    let resolved =
        ResolvedProfile { config: config.clone(), profile: Some(profile.name.clone()), disabled: !profile.enabled };

    let base = config.to_value();
    let mut merged = base.clone();
    let mut overridden = false;
    for section in PROFILE_SECTIONS {
        let Some(overrides) = profile.section(section) else { continue };
        let Some(Value::Object(target)) = merged.get_mut(section) else { continue };
        for (key, value) in overrides {
            // Only settings that exist: an unknown key is a typo, not a new option.
            if !target.contains_key(key) {
                continue;
            }
            target.insert(key.clone(), value.clone());
            overridden = true;
        }
    }
    if !overridden {
        return resolved;
    }

    // `validate_config` resets a bad key to the schema default, but a bad override
    // should fall back to the user's own setting. So put the base value back for
    // each key it rejects, then validate again.
    let mut result = validate_config(&merged);
    let mut attempt = 0;
    while attempt < 3 && !result.issues.is_empty() {
        for issue in &result.issues {
            let Some(PathSeg::Key(section)) = issue.path.first() else { continue };
            if !PROFILE_SECTIONS.contains(&section.as_str()) {
                continue;
            }
            let Some(base_section) = base.get(section.as_str()) else { continue };
            match issue.path.get(1) {
                Some(PathSeg::Key(key)) => {
                    if let Some(Value::Object(target)) = merged.get_mut(section.as_str()) {
                        match base_section.get(key.as_str()) {
                            Some(value) => {
                                target.insert(key.clone(), value.clone());
                            }
                            None => {
                                target.shift_remove(key.as_str());
                            }
                        }
                    }
                }
                _ => {
                    if let Some(target) = merged.get_mut(section.as_str()) {
                        *target = base_section.clone();
                    }
                }
            }
        }
        result = validate_config(&merged);
        attempt += 1;
    }
    if result.issues.is_empty() { ResolvedProfile { config: result.config, ..resolved } } else { resolved }
}
