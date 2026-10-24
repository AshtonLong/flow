//! Cleanup. PORT: `src/renderer/settings/pages/Cleanup.tsx` — not ported yet (stub).
//!
//! To port: keep this module and the `Cleanup` struct, put the page's local
//! state in the struct, and draw it in `ui` following `general.rs`. The TSX
//! uses: `Toggle`, `Select`, `TextArea`, `TextField`, `ChipsInput`, `badge`,
//! `Modal`, `ConfirmDialog`, `util::list_draft::ListDraft` (dictionary and
//! snippets) and `util::format::{prompt_label, prompt_key}`.

use egui::Ui;

use super::{Cx, Page};

#[derive(Default)]
pub struct Cleanup {}

impl Page for Cleanup {
    fn ui(&mut self, ui: &mut Ui, _cx: &mut Cx<'_>) {
        super::not_ported(ui, "Cleanup", "pages/Cleanup.tsx");
    }
}
