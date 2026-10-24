//! Hotkeys. PORT: `src/renderer/settings/pages/Hotkeys.tsx` — not ported yet (stub).
//!
//! To port: keep this module and the `Hotkeys` struct, put the page's local
//! state in the struct, and draw it in `ui` following `general.rs`. The TSX
//! uses: `HotkeyRecorder` (pass `cx.link` and `cx.events`), `key_caps`,
//! `Toggle`, `Notice`; conflicts come from `findConflicts` (see
//! `adapters::hotkeys` until `flow_core::hotkeys` lands).

use egui::Ui;

use super::{Cx, Page};

#[derive(Default)]
pub struct Hotkeys {}

impl Page for Hotkeys {
    fn ui(&mut self, ui: &mut Ui, _cx: &mut Cx<'_>) {
        super::not_ported(ui, "Hotkeys", "pages/Hotkeys.tsx");
    }
}
