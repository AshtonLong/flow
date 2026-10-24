//! The parts of the hotkey grammar the settings window needs (key caps, the
//! recorder, the mock's capture). Port of `src/shared/hotkeys.ts`.
//!
//! ADAPTER: `flow_core::hotkeys` is being ported in parallel. Once it exports
//! `parse_binding`, `format_binding`, `normalize_binding`, `binding_from_keys`
//! and `display_tokens`, replace this file's body with re-exports of those.

/// Generic modifiers match either side; `L`/`R` variants match one side only.
const GENERIC_MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];
const SIDED_MODIFIERS: [&str; 8] = [
    "LCtrl", "RCtrl", "LAlt", "RAlt", "LShift", "RShift", "LWin", "RWin",
];
const MOUSE_BUTTONS: [&str; 3] = ["Mouse3", "Mouse4", "Mouse5"];
const NAMED_KEYS: [&str; 31] = [
    "Space",
    "Esc",
    "Enter",
    "Tab",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Up",
    "Down",
    "Left",
    "Right",
    "CapsLock",
    "ScrollLock",
    "NumLock",
    "PrintScreen",
    "Pause",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Backslash",
    "Semicolon",
    "Quote",
    "Comma",
    "Period",
    "Slash",
    "Backquote",
];

const ALIASES: [(&str, &str); 28] = [
    ("control", "Ctrl"),
    ("ctl", "Ctrl"),
    ("option", "Alt"),
    ("meta", "Win"),
    ("super", "Win"),
    ("windows", "Win"),
    ("cmd", "Win"),
    ("command", "Win"),
    ("escape", "Esc"),
    ("return", "Enter"),
    ("del", "Delete"),
    ("ins", "Insert"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    ("pagedown", "PageDown"),
    ("pageup", "PageUp"),
    ("spacebar", "Space"),
    ("rightctrl", "RCtrl"),
    ("leftctrl", "LCtrl"),
    ("rightalt", "RAlt"),
    ("leftalt", "LAlt"),
    ("rightshift", "RShift"),
    ("leftshift", "LShift"),
    ("rightwin", "RWin"),
    ("leftwin", "LWin"),
    ("mousemiddle", "Mouse3"),
    ("mouseback", "Mouse4"),
    ("mouseforward", "Mouse5"),
];

const MODIFIER_ORDER: [&str; 12] = [
    "Ctrl", "LCtrl", "RCtrl", "Alt", "LAlt", "RAlt", "Shift", "LShift", "RShift", "Win", "LWin",
    "RWin",
];

/// Canonical tokens: modifiers first in a fixed order, then keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub tokens: Vec<String>,
}

pub fn is_modifier_token(token: &str) -> bool {
    MODIFIER_ORDER.contains(&token)
}

/// The generic modifier a sided key belongs to: `LCtrl` → `Ctrl`.
pub fn generic_of(key: &str) -> Option<&'static str> {
    SIDED_MODIFIERS
        .contains(&key)
        .then(|| GENERIC_MODIFIERS.iter().copied().find(|g| key[1..] == **g))
        .flatten()
}

fn canonical_token(raw: &str) -> Option<String> {
    let t: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if t.is_empty() {
        return None;
    }
    if let Some((_, to)) = ALIASES.iter().find(|(from, _)| *from == t) {
        return Some((*to).to_string());
    }
    let numbered = (1..=24)
        .map(|n| format!("F{n}"))
        .chain((0..10).map(|n| format!("Numpad{n}")));
    let single = (b'A'..=b'Z')
        .map(|c| (c as char).to_string())
        .chain((0..10).map(|n| n.to_string()));
    GENERIC_MODIFIERS
        .iter()
        .chain(SIDED_MODIFIERS.iter())
        .chain(NAMED_KEYS.iter())
        .chain(MOUSE_BUTTONS.iter())
        .map(|s| s.to_string())
        .chain(single)
        .chain(numbered)
        .find(|candidate| candidate.to_lowercase() == t)
}

fn sort_tokens(tokens: Vec<String>) -> Vec<String> {
    let mut mods: Vec<String> = tokens
        .iter()
        .filter(|t| is_modifier_token(t))
        .cloned()
        .collect();
    mods.sort_by_key(|t| MODIFIER_ORDER.iter().position(|m| m == t));
    let keys = tokens.into_iter().filter(|t| !is_modifier_token(t));
    let mut out: Vec<String> = Vec::new();
    for token in mods.into_iter().chain(keys) {
        if !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

/// Parses a binding string. `None` when empty or when any token is unknown.
pub fn parse_binding(text: &str) -> Option<Binding> {
    if text.trim().is_empty() {
        return None;
    }
    let tokens = text
        .split('+')
        .map(canonical_token)
        .collect::<Option<Vec<_>>>()?;
    if tokens.is_empty() {
        return None;
    }
    Some(Binding {
        tokens: sort_tokens(tokens),
    })
}

pub fn format_binding(binding: &Binding) -> String {
    binding.tokens.join("+")
}

/// Canonical text for a binding string, or `None` if it does not parse.
pub fn normalize_binding(text: &str) -> Option<String> {
    parse_binding(text).map(|b| format_binding(&b))
}

/// Builds a binding from the set of keys currently held (used by the recorder).
pub fn binding_from_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    prefer_generic: bool,
) -> Binding {
    let tokens = keys
        .into_iter()
        .map(|k| match generic_of(k) {
            Some(generic) if prefer_generic => generic.to_string(),
            _ => k.to_string(),
        })
        .collect();
    Binding {
        tokens: sort_tokens(tokens),
    }
}

fn display_name(token: &str) -> &str {
    match token {
        "LCtrl" => "Left Ctrl",
        "RCtrl" => "Right Ctrl",
        "LAlt" => "Left Alt",
        "RAlt" => "Right Alt",
        "LShift" => "Left Shift",
        "RShift" => "Right Shift",
        "LWin" => "Left Win",
        "RWin" => "Right Win",
        "Mouse3" => "Middle click",
        "Mouse4" => "Mouse back",
        "Mouse5" => "Mouse forward",
        "PageUp" => "Page Up",
        "PageDown" => "Page Down",
        "Up" => "↑",
        "Down" => "↓",
        "Left" => "←",
        "Right" => "→",
        other => other,
    }
}

/// Human-readable key caps for a binding string, e.g. `["Ctrl", "Win"]`.
pub fn display_tokens(text: &str) -> Vec<String> {
    match parse_binding(text) {
        Some(b) => b
            .tokens
            .iter()
            .map(|t| display_name(t).to_string())
            .collect(),
        None if text.is_empty() => Vec::new(),
        None => vec![text.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders() {
        assert_eq!(normalize_binding("win + ctrl").as_deref(), Some("Ctrl+Win"));
        assert_eq!(
            normalize_binding("shift+alt+v").as_deref(),
            Some("Alt+Shift+V")
        );
        assert_eq!(normalize_binding("Ctrl+Ctrl").as_deref(), Some("Ctrl"));
        assert_eq!(normalize_binding("escape").as_deref(), Some("Esc"));
        assert_eq!(normalize_binding("f13").as_deref(), Some("F13"));
        assert_eq!(normalize_binding("Ctrl+Bogus"), None);
        assert_eq!(normalize_binding(""), None);
    }

    #[test]
    fn builds_from_held_keys() {
        assert_eq!(
            format_binding(&binding_from_keys(["Space", "LWin", "RCtrl"], true)),
            "Ctrl+Win+Space"
        );
        assert_eq!(
            format_binding(&binding_from_keys(["LCtrl"], false)),
            "LCtrl"
        );
    }

    #[test]
    fn display_tokens_read_well() {
        assert_eq!(display_tokens("Ctrl+Win"), vec!["Ctrl", "Win"]);
        assert_eq!(
            display_tokens("RCtrl+Mouse4"),
            vec!["Right Ctrl", "Mouse back"]
        );
        assert_eq!(display_tokens("nonsense+key"), vec!["nonsense+key"]);
        assert!(display_tokens("").is_empty());
    }
}
