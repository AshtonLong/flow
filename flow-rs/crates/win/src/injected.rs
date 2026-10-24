//! Telling our own simulated input from the user's. Every `SendInput` call in
//! this crate tags its events with [`INJECT_TAG`] in `dwExtraInfo`, and the
//! input hook drops events carrying it, so the paste shortcut, the Enter keys
//! sent while typing and the Start-menu mask never reach the hotkey matcher.
//!
//! This replaces the TS `InjectedKeys` ledger (`src/main/platform/injected.ts`),
//! which existed only because uiohook hides the injected flag. Input injected by
//! other software (AutoHotkey, remote desktop, on-screen keyboards) carries no
//! tag of ours and is still seen, as before.

use flow_core::hotkeys::{KeyId, is_modifier_token, parse_binding};

/// Tags our injected events (`dwExtraInfo`) so they can be told from real keys: "FLOW".
pub const INJECT_TAG: usize = 0x464c_4f57;

/// True when an event's `dwExtraInfo` marks it as input this process injected.
pub fn is_own_injection(extra_info: usize) -> bool {
    extra_info == INJECT_TAG
}

fn left_of(token: &str) -> Option<&'static str> {
    match token {
        "Ctrl" => Some("LCtrl"),
        "Alt" => Some("LAlt"),
        "Shift" => Some("LShift"),
        "Win" => Some("LWin"),
        _ => None,
    }
}

/// The physical keys to press for a shortcut such as `Ctrl+Shift+V`, in press
/// order (modifiers first; generic modifiers use the left key). `None` when the
/// binding does not parse or contains a mouse button.
pub fn shortcut_keys(binding: &str) -> Option<Vec<KeyId>> {
    let parsed = parse_binding(binding)?;
    let keys: Vec<KeyId> = parsed
        .tokens
        .iter()
        .map(|t| match left_of(t) {
            Some(left) if is_modifier_token(t) => left.to_string(),
            _ => t.clone(),
        })
        .collect();
    if keys.iter().any(|k| k.starts_with("Mouse")) {
        return None;
    }
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod injection_tag {
        use super::*;

        #[test]
        fn recognises_only_our_own_tag() {
            assert!(is_own_injection(INJECT_TAG));
            // Real keys carry 0; other software uses tags of its own.
            assert!(!is_own_injection(0));
            assert!(!is_own_injection(INJECT_TAG + 1));
            assert!(!is_own_injection(0xFFC3_C3C3)); // AutoHotkey's default
        }

        #[test]
        fn spells_flow() {
            assert_eq!(&(INJECT_TAG as u32).to_be_bytes(), b"FLOW");
        }
    }

    mod shortcut_keys {
        use super::*;

        fn keys(binding: &str) -> Option<Vec<String>> {
            shortcut_keys(binding)
        }

        #[test]
        fn orders_modifiers_first_and_uses_the_left_key_for_generic_modifiers() {
            assert_eq!(keys("Ctrl+V"), Some(vec!["LCtrl".into(), "V".into()]));
            assert_eq!(keys("v + shift + ctrl"), Some(vec!["LCtrl".into(), "LShift".into(), "V".into()]));
            assert_eq!(keys("Shift+Insert"), Some(vec!["LShift".into(), "Insert".into()]));
            assert_eq!(keys("Alt+Win+V"), Some(vec!["LAlt".into(), "LWin".into(), "V".into()]));
        }

        #[test]
        fn keeps_sided_modifiers_and_lone_keys() {
            assert_eq!(keys("RCtrl+V"), Some(vec!["RCtrl".into(), "V".into()]));
            assert_eq!(keys("Enter"), Some(vec!["Enter".into()]));
        }

        #[test]
        fn rejects_unparsable_bindings_and_mouse_buttons() {
            assert_eq!(keys(""), None);
            assert_eq!(keys("Ctrl+Nope"), None);
            assert_eq!(keys("Ctrl+Mouse4"), None);
        }
    }
}
