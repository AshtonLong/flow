//! Low-level hook events → `KeyId`. Produces the same ids `src/main/input/keymap.ts`
//! derived from uiohook's keycodes: sided modifiers, the numpad reported the
//! same with NumLock on or off, numpad Enter as `Enter`, middle/back/forward
//! mouse buttons as `Mouse3/4/5`. Left and right click, keys Flow has no id
//! for, Unicode packets and the Start-menu mask key (VK 0xE8) map to nothing.
//! Pure: the hook procedure calls these and nothing else.

use std::collections::HashSet;
use std::sync::LazyLock;

use windows::Win32::UI::WindowsAndMessaging::{
    LLKHF_EXTENDED, WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    WM_XBUTTONUP,
};

use crate::injected::is_own_injection;

/// A key or mouse button going down or up, as the hook reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    pub key: &'static str,
    pub down: bool,
}

/// The right Shift's scan code. Shift has no extended flag, so a generic
/// `VK_SHIFT` (sent by some software) can only be told apart by scan code.
const SCAN_RSHIFT: u32 = 0x36;

/// The key for a virtual-key code from a `WH_KEYBOARD_LL` event. `extended` is
/// `LLKHF_EXTENDED`. With NumLock off the numpad reports navigation keys without
/// the extended flag (and the 5 key reports `VK_CLEAR`); those stay numpad keys.
pub fn key_id_from_vk(vk: u32, extended: bool, scan: u32) -> Option<&'static str> {
    let nav = |named: &'static str, numpad: &'static str| Some(if extended { named } else { numpad });
    match vk {
        0x08 => Some("Backspace"),
        0x09 => Some("Tab"),
        0x0c => Some("Numpad5"), // VK_CLEAR
        // The main Enter and the numpad Enter (extended) are one id.
        0x0d => Some("Enter"),
        // Generic modifiers only arrive from software that injects them.
        0x10 => Some(if scan & 0xff == SCAN_RSHIFT { "RShift" } else { "LShift" }),
        0x11 => Some(if extended { "RCtrl" } else { "LCtrl" }),
        0x12 => Some(if extended { "RAlt" } else { "LAlt" }),
        0x13 => Some("Pause"),
        0x14 => Some("CapsLock"),
        0x1b => Some("Esc"),
        0x20 => Some("Space"),
        0x21 => nav("PageUp", "Numpad9"),
        0x22 => nav("PageDown", "Numpad3"),
        0x23 => nav("End", "Numpad1"),
        0x24 => nav("Home", "Numpad7"),
        0x25 => nav("Left", "Numpad4"),
        0x26 => nav("Up", "Numpad8"),
        0x27 => nav("Right", "Numpad6"),
        0x28 => nav("Down", "Numpad2"),
        0x2c => Some("PrintScreen"),
        0x2d => nav("Insert", "Numpad0"),
        // The numpad's Delete (decimal point) has no id.
        0x2e => extended.then_some("Delete"),
        0x30..=0x39 => Some(DIGITS[(vk - 0x30) as usize]),
        0x41..=0x5a => Some(LETTERS[(vk - 0x41) as usize]),
        0x5b => Some("LWin"),
        0x5c => Some("RWin"),
        0x60..=0x69 => Some(NUMPAD[(vk - 0x60) as usize]),
        0x70..=0x87 => Some(FUNCTION[(vk - 0x70) as usize]),
        0x90 => Some("NumLock"),
        0x91 => Some("ScrollLock"),
        0xa0 => Some("LShift"),
        0xa1 => Some("RShift"),
        0xa2 => Some("LCtrl"),
        0xa3 => Some("RCtrl"),
        0xa4 => Some("LAlt"),
        0xa5 => Some("RAlt"),
        0xba => Some("Semicolon"),
        0xbb => Some("Equal"),
        0xbc => Some("Comma"),
        0xbd => Some("Minus"),
        0xbe => Some("Period"),
        0xbf => Some("Slash"),
        0xc0 => Some("Backquote"),
        0xdb => Some("BracketLeft"),
        0xdc => Some("Backslash"),
        0xdd => Some("BracketRight"),
        0xde => Some("Quote"),
        _ => None,
    }
}

const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W",
    "X", "Y", "Z",
];

const NUMPAD: [&str; 10] =
    ["Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9"];

const FUNCTION: [&str; 24] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15", "F16", "F17",
    "F18", "F19", "F20", "F21", "F22", "F23", "F24",
];

/// The key for a mouse button number (3 middle, 4 back, 5 forward). Left (1) and right (2) are not bindable.
pub fn key_id_from_mouse_button(button: u32) -> Option<&'static str> {
    match button {
        3 => Some("Mouse3"),
        4 => Some("Mouse4"),
        5 => Some("Mouse5"),
        _ => None,
    }
}

/// `KEY_IDS` entries the hook can never report. Every other id must be reachable above.
pub const UNSUPPORTED_KEY_IDS: [&str; 0] = [];

static MAPPED: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut ids = HashSet::new();
    for vk in 0..=0xff {
        for extended in [false, true] {
            ids.extend(key_id_from_vk(vk, extended, 0));
            ids.extend(key_id_from_vk(vk, extended, SCAN_RSHIFT));
        }
    }
    ids.extend((0..=8).filter_map(key_id_from_mouse_button));
    ids
});

/// Whether the hook can report this key id at all.
pub fn is_mapped_key_id(key: &str) -> bool {
    MAPPED.contains(key)
}

/// Maps a `WH_KEYBOARD_LL` event (`wParam`, then the `KBDLLHOOKSTRUCT` fields).
/// `None` for our own injected input, other messages and keys without an id.
pub fn map_keyboard_event(message: u32, vk: u32, scan: u32, flags: u32, extra_info: usize) -> Option<InputEvent> {
    if is_own_injection(extra_info) {
        return None;
    }
    let down = match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return None,
    };
    let key = key_id_from_vk(vk, flags & LLKHF_EXTENDED.0 != 0, scan)?;
    Some(InputEvent { key, down })
}

/// Maps a `WH_MOUSE_LL` event (`wParam`, then the `MSLLHOOKSTRUCT` fields).
/// Only the middle and X buttons have ids; moves, wheels and left/right clicks are `None`.
pub fn map_mouse_event(message: u32, mouse_data: u32, extra_info: usize) -> Option<InputEvent> {
    if is_own_injection(extra_info) {
        return None;
    }
    let (button, down) = match message {
        WM_MBUTTONDOWN => (3, true),
        WM_MBUTTONUP => (3, false),
        // HIWORD(mouseData): XBUTTON1 (1) is "back", XBUTTON2 (2) "forward".
        WM_XBUTTONDOWN | WM_XBUTTONUP => match mouse_data >> 16 {
            1 => (4, message == WM_XBUTTONDOWN),
            2 => (5, message == WM_XBUTTONDOWN),
            _ => return None,
        },
        _ => return None,
    };
    Some(InputEvent { key: key_id_from_mouse_button(button)?, down })
}

#[cfg(test)]
mod tests {
    use flow_core::hotkeys::{KEY_IDS, MOUSE_BUTTONS, SIDED_MODIFIERS, parse_binding};
    use windows::Win32::UI::WindowsAndMessaging::{
        LLKHF_INJECTED, LLKHF_UP, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONUP,
    };

    use super::*;
    use crate::injected::INJECT_TAG;
    use crate::vk::{is_extended_key, vk_from_key_id};

    #[test]
    fn maps_every_key_ids_entry_or_lists_it_as_unsupported() {
        let unaccounted: Vec<&str> =
            KEY_IDS.iter().copied().filter(|k| !is_mapped_key_id(k) && !UNSUPPORTED_KEY_IDS.contains(k)).collect();
        assert_eq!(unaccounted, Vec::<&str>::new());
    }

    #[test]
    fn lists_nothing_as_unsupported_that_is_in_fact_mapped_and_only_real_key_ids() {
        for key in UNSUPPORTED_KEY_IDS {
            assert!(KEY_IDS.contains(&key));
            assert!(!is_mapped_key_id(key));
        }
    }

    #[test]
    fn round_trips_every_keyboard_key_through_its_virtual_key() {
        let keyboard: Vec<&str> =
            KEY_IDS.iter().copied().filter(|k| !MOUSE_BUTTONS.contains(k) && is_mapped_key_id(k)).collect();
        assert!(keyboard.len() > 100);
        for key in keyboard {
            let vk = vk_from_key_id(key).unwrap_or_else(|| panic!("{key} has no virtual key"));
            // A physical press carries LLKHF_EXTENDED exactly for the keys injected with KEYEVENTF_EXTENDEDKEY.
            assert_eq!(key_id_from_vk(u32::from(vk), is_extended_key(key), 0), Some(key), "{key}");
        }
    }

    #[test]
    fn only_produces_ids_the_binding_grammar_understands() {
        for vk in 0..=0xffff {
            for extended in [false, true] {
                for scan in [0, SCAN_RSHIFT] {
                    let Some(key) = key_id_from_vk(vk, extended, scan) else { continue };
                    assert!(KEY_IDS.contains(&key), "{key}");
                    assert_eq!(parse_binding(key).map(|b| b.tokens), Some(vec![key.to_string()]));
                }
            }
        }
    }

    #[test]
    fn reports_sided_modifiers_never_generic_ones() {
        assert_eq!(key_id_from_vk(0xa2, false, 0x1d), Some("LCtrl"));
        assert_eq!(key_id_from_vk(0xa3, true, 0x1d), Some("RCtrl"));
        assert_eq!(key_id_from_vk(0xa4, false, 0x38), Some("LAlt"));
        assert_eq!(key_id_from_vk(0xa5, true, 0x38), Some("RAlt"));
        assert_eq!(key_id_from_vk(0xa0, false, 0x2a), Some("LShift"));
        assert_eq!(key_id_from_vk(0xa1, false, 0x36), Some("RShift"));
        assert_eq!(key_id_from_vk(0x5b, true, 0x5b), Some("LWin"));
        assert_eq!(key_id_from_vk(0x5c, true, 0x5c), Some("RWin"));
        // Generic VK_CONTROL/VK_MENU/VK_SHIFT, as some software injects them.
        assert_eq!(key_id_from_vk(0x11, false, 0x1d), Some("LCtrl"));
        assert_eq!(key_id_from_vk(0x11, true, 0x1d), Some("RCtrl"));
        assert_eq!(key_id_from_vk(0x12, false, 0x38), Some("LAlt"));
        assert_eq!(key_id_from_vk(0x12, true, 0x38), Some("RAlt"));
        assert_eq!(key_id_from_vk(0x10, false, 0x2a), Some("LShift"));
        assert_eq!(key_id_from_vk(0x10, false, 0x36), Some("RShift"));
        for key in SIDED_MODIFIERS {
            assert!(is_mapped_key_id(key));
        }
    }

    #[test]
    fn maps_mouse_buttons_3_4_5_and_ignores_left_and_right_click() {
        assert_eq!(key_id_from_mouse_button(1), None);
        assert_eq!(key_id_from_mouse_button(2), None);
        assert_eq!(key_id_from_mouse_button(3), Some("Mouse3"));
        assert_eq!(key_id_from_mouse_button(4), Some("Mouse4"));
        assert_eq!(key_id_from_mouse_button(5), Some("Mouse5"));
        assert_eq!(key_id_from_mouse_button(6), None);
        assert_eq!(key_id_from_mouse_button(0), None);
        let xbutton = |n: u32| n << 16;
        assert_eq!(map_mouse_event(WM_MBUTTONDOWN, 0, 0), Some(InputEvent { key: "Mouse3", down: true }));
        assert_eq!(map_mouse_event(WM_MBUTTONUP, 0, 0), Some(InputEvent { key: "Mouse3", down: false }));
        assert_eq!(map_mouse_event(WM_XBUTTONDOWN, xbutton(1), 0), Some(InputEvent { key: "Mouse4", down: true }));
        assert_eq!(map_mouse_event(WM_XBUTTONUP, xbutton(1), 0), Some(InputEvent { key: "Mouse4", down: false }));
        assert_eq!(map_mouse_event(WM_XBUTTONDOWN, xbutton(2), 0), Some(InputEvent { key: "Mouse5", down: true }));
        assert_eq!(map_mouse_event(WM_XBUTTONDOWN, xbutton(3), 0), None);
        assert_eq!(map_mouse_event(WM_LBUTTONDOWN, 0, 0), None);
        assert_eq!(map_mouse_event(WM_RBUTTONUP, 0, 0), None);
        assert_eq!(map_mouse_event(WM_MOUSEMOVE, 0, 0), None);
        assert_eq!(map_mouse_event(WM_MOUSEWHEEL, xbutton(120), 0), None);
        assert_eq!(vk_from_key_id("Mouse4").map(|vk| key_id_from_vk(u32::from(vk), false, 0)), Some(None));
    }

    #[test]
    fn maps_the_numpad_the_same_with_numlock_on_or_off() {
        // NumLock off: the numpad reports navigation keys without the extended flag.
        assert_eq!(key_id_from_vk(0x61, false, 0x4f), Some("Numpad1"));
        assert_eq!(key_id_from_vk(0x23, false, 0x4f), Some("Numpad1"));
        assert_eq!(key_id_from_vk(0x60, false, 0x52), Some("Numpad0"));
        assert_eq!(key_id_from_vk(0x2d, false, 0x52), Some("Numpad0"));
        assert_eq!(key_id_from_vk(0x0c, false, 0x4c), Some("Numpad5"));
        // The dedicated navigation keys stay themselves.
        assert_eq!(key_id_from_vk(0x23, true, 0x4f), Some("End"));
        assert_eq!(key_id_from_vk(0x26, true, 0x48), Some("Up"));
        assert_eq!(key_id_from_vk(0x0d, true, 0x1c), Some("Enter"));
    }

    #[test]
    fn returns_none_for_unknown_codes_including_unicode_packets_and_the_mask_key() {
        assert_eq!(key_id_from_vk(0, false, 0), None);
        assert_eq!(key_id_from_vk(0x5d, true, 0x5d), None); // context menu
        assert_eq!(key_id_from_vk(0xad, true, 0x20), None); // volume mute
        assert_eq!(key_id_from_vk(0xe7, false, 0), None); // VK_PACKET (injected Unicode)
        assert_eq!(key_id_from_vk(0xe8, false, 0), None); // the Start-menu mask key
        assert_eq!(key_id_from_vk(0x2e, false, 0x53), None); // the numpad's Delete
        assert_eq!(key_id_from_vk(0x6a, false, 0x37), None); // numpad *
        assert_eq!(key_id_from_vk(u32::MAX, false, 0), None);
        assert_eq!(vk_from_key_id("toString"), None);
        assert_eq!(vk_from_key_id("Ctrl"), None);
        assert!(!is_mapped_key_id("constructor"));
    }

    #[test]
    fn maps_key_messages_to_downs_and_ups() {
        let ext = LLKHF_EXTENDED.0;
        assert_eq!(map_keyboard_event(WM_KEYDOWN, 0x41, 0x1e, 0, 0), Some(InputEvent { key: "A", down: true }));
        assert_eq!(map_keyboard_event(WM_KEYUP, 0x41, 0x1e, LLKHF_UP.0, 0), Some(InputEvent { key: "A", down: false }));
        // Alt chords arrive as WM_SYSKEY*.
        assert_eq!(map_keyboard_event(WM_SYSKEYDOWN, 0xa5, 0x38, ext, 0), Some(InputEvent { key: "RAlt", down: true }));
        assert_eq!(
            map_keyboard_event(WM_SYSKEYUP, 0xa5, 0x38, ext | LLKHF_UP.0, 0),
            Some(InputEvent { key: "RAlt", down: false })
        );
        assert_eq!(map_keyboard_event(WM_KEYDOWN, 0x0d, 0x1c, ext, 0), Some(InputEvent { key: "Enter", down: true }));
        assert_eq!(map_keyboard_event(0x0102, 0x41, 0x1e, 0, 0), None); // WM_CHAR is never sent to the hook
    }

    #[test]
    fn drops_our_own_injected_input_but_not_other_softwares() {
        let injected = LLKHF_INJECTED.0;
        assert_eq!(map_keyboard_event(WM_KEYDOWN, 0x56, 0x2f, injected, INJECT_TAG), None);
        assert_eq!(map_keyboard_event(WM_KEYDOWN, 0xe8, 0, injected, INJECT_TAG), None);
        assert_eq!(map_mouse_event(WM_MBUTTONDOWN, 0, INJECT_TAG), None);
        assert_eq!(map_keyboard_event(WM_KEYDOWN, 0x56, 0x2f, injected, 0), Some(InputEvent { key: "V", down: true }));
        assert_eq!(
            map_keyboard_event(WM_KEYDOWN, 0x56, 0x2f, injected, 0xFFC3_C3C3),
            Some(InputEvent { key: "V", down: true })
        );
    }
}
