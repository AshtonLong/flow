//! The settings window's components, ported from
//! `src/renderer/settings/components/*.tsx` and the component classes of
//! `styles.css`. Each one draws itself from the design tokens; see the crate
//! README for the patterns.
//!
//! Conventions:
//! - Builders: `Widget::new(...).option(...).show(ui)`.
//! - Controls return what changed (`Option<T>`) instead of taking callbacks;
//!   the page writes the config (`cx.config.set(path, value)`).
//! - Transient state (text drafts, open menus) lives in egui memory under an
//!   id built from the `id_salt` you pass, like React state. Resources (an
//!   open microphone) are structs the page owns, so leaving the page frees them.

pub mod button;
pub mod card;
pub mod chips_input;
pub mod dialog;
pub mod error_boundary;
pub mod hotkey_recorder;
pub mod issues_banner;
pub mod key_caps;
pub mod level_meter;
pub mod measure;
pub mod notice;
pub mod select;
pub mod text_field;
pub mod toggle;
pub mod tooltip;

pub use button::{Button, IconButton, IconSpec, Size, Variant, link};
pub use card::{Group, SettingRow, card, empty_state, page_header, page_header_with};
pub use chips_input::ChipsInput;
pub use dialog::{ConfirmDialog, Modal};
pub use error_boundary::ErrorBoundary;
pub use hotkey_recorder::HotkeyRecorder;
pub use issues_banner::issues_banner;
pub use key_caps::key_caps;
pub use level_meter::{MicMeter, capture_help, level_meter, mic_select};
pub use notice::{BadgeTone, Notice, Tone, badge, progress_bar, spinner};
pub use select::{Select, SelectOption, SelectResponse};
pub use text_field::{
    FieldResponse, NumberField, TextArea, TextField, input, input_area, input_invalid, password_input,
};
pub use toggle::{Toggle, ToggleResponse, checkbox, radio};
