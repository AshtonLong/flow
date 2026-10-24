//! Advanced. PORT: `src/renderer/settings/pages/Advanced.tsx` — not ported yet (stub).
//!
//! To port: keep this module and the `Advanced` struct, put the page's local
//! state in the struct, and draw it in `ui` following `general.rs`. The TSX
//! uses: `cx.app_info`, `Button`, `Notice`, `Toggle`, `NumberField`,
//! `SettingRow::new_with` for the path lines, and file dialogs for export and
//! import (`rfd`, then `Request::ConfigExport(path)` / `ConfigImport(path)`).

use egui::Ui;

use super::{Cx, Page};

#[derive(Default)]
pub struct Advanced {}

impl Page for Advanced {
    fn ui(&mut self, ui: &mut Ui, _cx: &mut Cx<'_>) {
        super::not_ported(ui, "Advanced", "pages/Advanced.tsx");
    }
}
