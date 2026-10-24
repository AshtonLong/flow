//! Windows virtual-key codes for every `KeyId`. Pure data: no Win32 call is made
//! here. Mirrors `src/main/platform/vk.ts`.

/// The virtual-key code for a key id, or `None` when it has none.
pub fn vk_from_key_id(key: &str) -> Option<u16> {
    let named = match key {
        "LShift" => 0xa0,
        "RShift" => 0xa1,
        "LCtrl" => 0xa2,
        "RCtrl" => 0xa3,
        "LAlt" => 0xa4,
        "RAlt" => 0xa5,
        "LWin" => 0x5b,
        "RWin" => 0x5c,
        "Space" => 0x20,
        "Esc" => 0x1b,
        "Enter" => 0x0d,
        "Tab" => 0x09,
        "Backspace" => 0x08,
        "Delete" => 0x2e,
        "Insert" => 0x2d,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "CapsLock" => 0x14,
        "ScrollLock" => 0x91,
        "NumLock" => 0x90,
        "PrintScreen" => 0x2c,
        "Pause" => 0x13,
        "Minus" => 0xbd,
        "Equal" => 0xbb,
        "BracketLeft" => 0xdb,
        "BracketRight" => 0xdd,
        "Backslash" => 0xdc,
        "Semicolon" => 0xba,
        "Quote" => 0xde,
        "Comma" => 0xbc,
        "Period" => 0xbe,
        "Slash" => 0xbf,
        "Backquote" => 0xc0,
        // Mouse buttons have virtual-key codes too; they are only valid for GetAsyncKeyState.
        "Mouse3" => 0x04,
        "Mouse4" => 0x05,
        "Mouse5" => 0x06,
        _ => return numbered(key),
    };
    Some(named)
}

/// Letters, digits, `F1`–`F24` and `Numpad0`–`Numpad9`.
fn numbered(key: &str) -> Option<u16> {
    let bytes = key.as_bytes();
    if bytes.len() == 1 {
        return match bytes[0] {
            b @ b'A'..=b'Z' => Some(u16::from(b)),
            b @ b'0'..=b'9' => Some(u16::from(b)),
            _ => None,
        };
    }
    let index = |rest: &str, max: u16| -> Option<u16> {
        // Only canonical spellings: no sign, no leading zero.
        if rest.is_empty() || rest.starts_with('0') && rest.len() > 1 || !rest.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        rest.parse::<u16>().ok().filter(|n| *n <= max)
    };
    if let Some(rest) = key.strip_prefix("Numpad") {
        return index(rest, 9).map(|n| 0x60 + n);
    }
    if let Some(rest) = key.strip_prefix('F') {
        return index(rest, 24).filter(|n| *n >= 1).map(|n| 0x70 + n - 1);
    }
    None
}

/// Keys that need `KEYEVENTF_EXTENDEDKEY` when injected. Without it Windows reads
/// Insert/arrows/etc. as their numpad twins, which breaks e.g. `Shift+Insert`.
const EXTENDED: [&str; 16] = [
    "RCtrl",
    "RAlt",
    "LWin",
    "RWin",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Up",
    "Down",
    "Left",
    "Right",
    "NumLock",
    "PrintScreen",
];

pub fn is_extended_key(key: &str) -> bool {
    EXTENDED.contains(&key)
}

/// An unassigned virtual key, injected to stop the Start menu opening.
pub const VK_MASK_KEY: u16 = 0xe8;
/// LShift, RShift, LCtrl, RCtrl, LAlt, RAlt, LWin, RWin.
pub const MODIFIER_VKS: [u16; 8] = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0x5b, 0x5c];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use flow_core::hotkeys::{KEY_IDS, SIDED_MODIFIERS};

    use super::*;

    #[test]
    fn has_a_distinct_code_for_every_key_id() {
        let codes: Vec<(&str, Option<u16>)> = KEY_IDS.iter().map(|k| (*k, vk_from_key_id(k))).collect();
        let missing: Vec<&str> = codes.iter().filter(|(_, vk)| vk.is_none()).map(|(k, _)| *k).collect();
        assert_eq!(missing, Vec::<&str>::new());
        let distinct: HashSet<Option<u16>> = codes.iter().map(|(_, vk)| *vk).collect();
        assert_eq!(distinct.len(), KEY_IDS.len());
    }

    #[test]
    fn matches_the_win32_constants() {
        assert_eq!(vk_from_key_id("A"), Some(0x41));
        assert_eq!(vk_from_key_id("Z"), Some(0x5a));
        assert_eq!(vk_from_key_id("0"), Some(0x30));
        assert_eq!(vk_from_key_id("F1"), Some(0x70));
        assert_eq!(vk_from_key_id("F24"), Some(0x87));
        assert_eq!(vk_from_key_id("Numpad9"), Some(0x69));
        assert_eq!(vk_from_key_id("V"), Some(0x56));
        assert_eq!(vk_from_key_id("LCtrl"), Some(0xa2));
        assert_eq!(vk_from_key_id("Enter"), Some(0x0d));
        assert_eq!(vk_from_key_id("Insert"), Some(0x2d));
        assert_eq!(vk_from_key_id("Mouse4"), Some(0x05));
    }

    #[test]
    fn has_no_code_for_generic_modifiers_or_unknown_ids() {
        for key in ["Ctrl", "toString", "", "a", "F0", "F25", "F01", "Numpad10", "Numpad", "F+1", "Mouse6"] {
            assert_eq!(vk_from_key_id(key), None, "{key}");
        }
    }

    #[test]
    fn lists_every_sided_modifier_for_the_modifiers_up_check() {
        let mut listed = MODIFIER_VKS.to_vec();
        listed.sort();
        let mut sided: Vec<u16> = SIDED_MODIFIERS.iter().filter_map(|k| vk_from_key_id(k)).collect();
        sided.sort();
        assert_eq!(listed, sided);
    }

    #[test]
    fn flags_the_keys_that_need_keyeventf_extendedkey() {
        for key in ["Insert", "Delete", "Home", "End", "Up", "RCtrl", "RAlt", "LWin"] {
            assert!(is_extended_key(key), "{key}");
        }
        for key in ["LCtrl", "LShift", "RShift", "LAlt", "V", "Enter", "Numpad0"] {
            assert!(!is_extended_key(key), "{key}");
        }
    }
}
