//! The parts of the hotkey grammar the settings window needs (key caps, the
//! recorder, the mock's capture): `src/shared/hotkeys.ts`, from `flow_core`.

pub use flow_core::hotkeys::{
    Binding, binding_from_keys, display_tokens, format_binding, normalize_binding, parse_binding,
};
