//! Hotkey binding grammar, shared by the input hook (main) and the hotkey
//! recorder (settings). A binding is `+`-joined tokens, e.g. `Ctrl+Win`,
//! `RCtrl`, `Alt+Shift+V`, `Mouse4`. Mirrors `src/shared/hotkeys.ts`.

use serde::{Deserialize, Serialize};

use crate::config::{HotkeyAction, HotkeysConfig};

/// Generic modifiers match either side; `L`/`R` variants match one side only.
pub const GENERIC_MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Win"];

pub const SIDED_MODIFIERS: [&str; 8] = ["LCtrl", "RCtrl", "LAlt", "RAlt", "LShift", "RShift", "LWin", "RWin"];

pub const MOUSE_BUTTONS: [&str; 3] = ["Mouse3", "Mouse4", "Mouse5"];

/// Every physical key id the hook can report. Sided modifiers, never generic ones.
#[rustfmt::skip]
pub const KEY_IDS: [&str; 112] = [
    // Sided modifiers.
    "LCtrl", "RCtrl", "LAlt", "RAlt", "LShift", "RShift", "LWin", "RWin",
    // Letters.
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
    // Digits.
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
    // Function keys.
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15",
    "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    // Numpad digits.
    "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7",
    "Numpad8", "Numpad9",
    // Named keys.
    "Space", "Esc", "Enter", "Tab", "Backspace", "Delete", "Insert", "Home", "End", "PageUp",
    "PageDown", "Up", "Down", "Left", "Right", "CapsLock", "ScrollLock", "NumLock", "PrintScreen",
    "Pause", "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote",
    "Comma", "Period", "Slash", "Backquote",
    // Mouse buttons.
    "Mouse3", "Mouse4", "Mouse5",
];

/// A physical key as reported by the hook, e.g. `LCtrl`, `A`, `Mouse4`.
pub type KeyId = String;

fn alias(token: &str) -> Option<&'static str> {
    Some(match token {
        "control" | "ctl" => "Ctrl",
        "option" => "Alt",
        "meta" | "super" | "windows" | "cmd" | "command" => "Win",
        "escape" => "Esc",
        "return" => "Enter",
        "del" => "Delete",
        "ins" => "Insert",
        "pgup" | "pageup" => "PageUp",
        "pgdn" | "pagedown" => "PageDown",
        "spacebar" => "Space",
        "rightctrl" => "RCtrl",
        "leftctrl" => "LCtrl",
        "rightalt" => "RAlt",
        "leftalt" => "LAlt",
        "rightshift" => "RShift",
        "leftshift" => "LShift",
        "rightwin" => "RWin",
        "leftwin" => "LWin",
        "mousemiddle" => "Mouse3",
        "mouseback" => "Mouse4",
        "mouseforward" => "Mouse5",
        _ => return None,
    })
}

/// The canonical spelling of a generic modifier or key id, case-insensitively.
fn canonical(token: &str) -> Option<&'static str> {
    GENERIC_MODIFIERS.iter().chain(KEY_IDS.iter()).copied().find(|t| t.to_lowercase() == token)
}

const MODIFIER_ORDER: [&str; 12] =
    ["Ctrl", "LCtrl", "RCtrl", "Alt", "LAlt", "RAlt", "Shift", "LShift", "RShift", "Win", "LWin", "RWin"];

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Binding {
    /// Canonical tokens: modifiers first in a fixed order, then keys.
    pub tokens: Vec<String>,
}

pub fn is_modifier_token(token: &str) -> bool {
    MODIFIER_ORDER.contains(&token)
}

fn modifier_rank(token: &str) -> usize {
    MODIFIER_ORDER.iter().position(|t| *t == token).unwrap_or(MODIFIER_ORDER.len())
}

/// The generic modifier a sided key belongs to: `LCtrl` → `Ctrl`.
pub fn generic_of(key: &str) -> Option<&'static str> {
    if SIDED_MODIFIERS.contains(&key) {
        GENERIC_MODIFIERS.iter().copied().find(|g| *g == &key[1..])
    } else {
        None
    }
}

fn canonical_token(raw: &str) -> Option<&'static str> {
    let t: String = raw.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return None;
    }
    alias(&t).or_else(|| canonical(&t))
}

fn sort_tokens(tokens: Vec<String>) -> Vec<String> {
    let (mut mods, keys): (Vec<String>, Vec<String>) = tokens.into_iter().partition(|t| is_modifier_token(t));
    // Stable, like Array.prototype.sort.
    mods.sort_by_key(|t| modifier_rank(t));
    let mut out: Vec<String> = Vec::with_capacity(mods.len() + keys.len());
    for token in mods.into_iter().chain(keys) {
        if !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

/// Parses a binding string. Returns `None` when empty or when any token is unknown.
pub fn parse_binding(text: &str) -> Option<Binding> {
    if text.trim().is_empty() {
        return None;
    }
    let mut tokens = Vec::new();
    for part in text.split('+') {
        tokens.push(canonical_token(part)?.to_string());
    }
    if tokens.is_empty() {
        return None;
    }
    Some(Binding { tokens: sort_tokens(tokens) })
}

pub fn format_binding(binding: &Binding) -> String {
    binding.tokens.join("+")
}

/// Canonical text for a binding string, or `None` if it does not parse.
pub fn normalize_binding(text: &str) -> Option<String> {
    parse_binding(text).map(|b| format_binding(&b))
}

/// Whether a physical key satisfies a binding token (`LCtrl` satisfies `Ctrl`).
pub fn key_matches_token(key: &str, token: &str) -> bool {
    key == token || generic_of(key) == Some(token)
}

/// True when every token of the binding is satisfied by a key that is down.
pub fn binding_satisfied<'a, I>(binding: &Binding, down: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
    I::IntoIter: Clone,
{
    let down = down.into_iter();
    binding.tokens.iter().all(|token| down.clone().any(|key| key_matches_token(key, token)))
}

/// Builds a binding from the set of keys currently held (used by the recorder).
pub fn binding_from_keys<I, S>(keys: I, prefer_generic: bool) -> Binding
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let tokens = keys
        .into_iter()
        .map(|k| {
            let k = k.as_ref();
            match generic_of(k) {
                Some(generic) if prefer_generic => generic.to_string(),
                _ => k.to_string(),
            }
        })
        .collect();
    Binding { tokens: sort_tokens(tokens) }
}

/// True when `a` contains every token of `b` and more (`Ctrl+Win+Space` ⊃ `Ctrl+Win`).
pub fn is_superset(a: &Binding, b: &Binding) -> bool {
    a.tokens.len() > b.tokens.len()
        && b.tokens.iter().all(|t| a.tokens.iter().any(|x| x == t || generic_of(x) == Some(t.as_str())))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictKind {
    /// Identical bindings.
    Same,
    /// Does not parse.
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BindingConflict {
    pub action: String,
    pub other: String,
    pub kind: ConflictKind,
}

/// Flags bindings that cannot be told apart. A superset (`Ctrl+Win+Space` over
/// `Ctrl+Win`) is not a conflict: the longer chord wins when it completes.
/// `bindings` is `(action, binding text)` in display order.
pub fn find_conflicts<A, T>(bindings: &[(A, T)]) -> Vec<BindingConflict>
where
    A: AsRef<str>,
    T: AsRef<str>,
{
    let mut conflicts = Vec::new();
    for (action, text) in bindings {
        if parse_binding(text.as_ref()).is_none() {
            conflicts.push(BindingConflict {
                action: action.as_ref().to_string(),
                other: action.as_ref().to_string(),
                kind: ConflictKind::Invalid,
            });
        }
    }
    for i in 0..bindings.len() {
        for j in (i + 1)..bindings.len() {
            let a = normalize_binding(bindings[i].1.as_ref());
            let b = normalize_binding(bindings[j].1.as_ref());
            if let (Some(a), Some(b)) = (a, b)
                && a == b
            {
                let (first, second) = (bindings[i].0.as_ref(), bindings[j].0.as_ref());
                conflicts.push(BindingConflict {
                    action: first.to_string(),
                    other: second.to_string(),
                    kind: ConflictKind::Same,
                });
                conflicts.push(BindingConflict {
                    action: second.to_string(),
                    other: first.to_string(),
                    kind: ConflictKind::Same,
                });
            }
        }
    }
    conflicts
}

/// [`find_conflicts`] over the configured bindings, keyed by config name
/// (`hold_to_talk`, `toggle`, `cancel`, `paste_last`), as the Hotkeys page checks them.
pub fn hotkey_conflicts(hotkeys: &HotkeysConfig) -> Vec<BindingConflict> {
    let bindings: Vec<(&str, &str)> =
        HotkeyAction::ALL.iter().map(|action| (action.key(), hotkeys.binding(*action))).collect();
    find_conflicts(&bindings)
}

fn display_name(token: &str) -> Option<&'static str> {
    Some(match token {
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
        _ => return None,
    })
}

/// Human-readable key caps for a binding string, e.g. `["Ctrl", "Win"]`.
pub fn display_tokens(text: &str) -> Vec<String> {
    match parse_binding(text) {
        Some(b) => b.tokens.iter().map(|t| display_name(t).map(str::to_string).unwrap_or_else(|| t.clone())).collect(),
        None if text.is_empty() => Vec::new(),
        None => vec![text.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_binding {
        use super::*;

        #[test]
        fn normalises_bindings() {
            for (input, expected) in [
                ("Ctrl+Win", "Ctrl+Win"),
                ("win + ctrl", "Ctrl+Win"),
                ("Ctrl+Win+Space", "Ctrl+Win+Space"),
                ("alt+shift+v", "Alt+Shift+V"),
                ("Esc", "Esc"),
                ("escape", "Esc"),
                ("RCtrl", "RCtrl"),
                ("right ctrl", "RCtrl"),
                ("Mouse4", "Mouse4"),
                ("control+f9", "Ctrl+F9"),
                ("Shift+Ctrl+Alt+K", "Ctrl+Alt+Shift+K"),
            ] {
                assert_eq!(normalize_binding(input).as_deref(), Some(expected), "normalises {input} to {expected}");
            }
        }

        #[test]
        fn rejects() {
            for input in ["", "   ", "Ctrl+", "Ctrl+Banana", "+"] {
                assert_eq!(parse_binding(input), None, "rejects {input:?}");
            }
        }

        #[test]
        fn round_trips_through_format_binding() {
            let binding = parse_binding("Ctrl+Win+Space").unwrap();
            assert_eq!(parse_binding(&format_binding(&binding)), Some(binding));
        }
    }

    mod matching {
        use super::*;

        #[test]
        fn lets_either_side_satisfy_a_generic_modifier() {
            assert!(key_matches_token("LCtrl", "Ctrl"));
            assert!(key_matches_token("RCtrl", "Ctrl"));
            assert!(!key_matches_token("LCtrl", "RCtrl"));
            assert!(key_matches_token("A", "A"));
        }

        #[test]
        fn is_satisfied_only_when_every_token_is_held() {
            let binding = parse_binding("Ctrl+Win").unwrap();
            assert!(!binding_satisfied(&binding, ["LCtrl"]));
            assert!(binding_satisfied(&binding, ["LCtrl", "LWin"]));
            assert!(binding_satisfied(&binding, ["RCtrl", "RWin"]));
        }

        #[test]
        fn builds_a_binding_from_held_keys() {
            assert_eq!(format_binding(&binding_from_keys(["LWin", "Space", "LCtrl"], true)), "Ctrl+Win+Space");
            assert_eq!(format_binding(&binding_from_keys(["RCtrl"], false)), "RCtrl");
        }

        #[test]
        fn recognises_a_longer_chord_as_a_superset() {
            let hold = parse_binding("Ctrl+Win").unwrap();
            let toggle = parse_binding("Ctrl+Win+Space").unwrap();
            assert!(is_superset(&toggle, &hold));
            assert!(!is_superset(&hold, &toggle));
            assert!(!is_superset(&hold, &hold));
            assert!(is_superset(&parse_binding("LCtrl+Win+Space").unwrap(), &hold));
        }
    }

    mod find_conflicts {
        use super::*;

        fn conflict(action: &str, other: &str, kind: ConflictKind) -> BindingConflict {
            BindingConflict { action: action.into(), other: other.into(), kind }
        }

        #[test]
        fn accepts_the_defaults() {
            assert_eq!(
                find_conflicts(&[
                    ("hold_to_talk", "Ctrl+Win"),
                    ("toggle", "Ctrl+Win+Space"),
                    ("cancel", "Esc"),
                    ("paste_last", "Alt+Shift+V"),
                ]),
                vec![]
            );
        }

        #[test]
        fn flags_identical_bindings_on_both_actions() {
            let conflicts = find_conflicts(&[("hold_to_talk", "F9"), ("toggle", "f9"), ("cancel", "Esc")]);
            assert_eq!(
                conflicts,
                vec![
                    conflict("hold_to_talk", "toggle", ConflictKind::Same),
                    conflict("toggle", "hold_to_talk", ConflictKind::Same),
                ]
            );
        }

        #[test]
        fn checks_the_configured_bindings_by_config_name() {
            assert_eq!(hotkey_conflicts(&HotkeysConfig::default()), vec![]);
            let hotkeys =
                HotkeysConfig { toggle: "win+ctrl".into(), cancel: "Nope".into(), ..HotkeysConfig::default() };
            assert_eq!(
                hotkey_conflicts(&hotkeys),
                vec![
                    conflict("cancel", "cancel", ConflictKind::Invalid),
                    conflict("hold_to_talk", "toggle", ConflictKind::Same),
                    conflict("toggle", "hold_to_talk", ConflictKind::Same),
                ]
            );
        }

        #[test]
        fn flags_a_binding_that_does_not_parse() {
            assert_eq!(
                find_conflicts(&[("cancel", "Nope")]),
                vec![conflict("cancel", "cancel", ConflictKind::Invalid)]
            );
        }
    }

    mod display_tokens {
        use super::*;

        #[test]
        fn spells_out_sided_modifiers_and_mouse_buttons() {
            assert_eq!(display_tokens("RCtrl"), vec!["Right Ctrl"]);
            assert_eq!(display_tokens("Ctrl+Win"), vec!["Ctrl", "Win"]);
            assert_eq!(display_tokens("Mouse4"), vec!["Mouse back"]);
        }
    }

    #[test]
    fn key_ids_are_canonical_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for key in KEY_IDS {
            assert!(seen.insert(key), "{key} listed twice");
            assert_eq!(parse_binding(key).map(|b| b.tokens), Some(vec![key.to_string()]));
        }
    }
}
