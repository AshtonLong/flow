//! Profiles. PORT: `src/renderer/settings/pages/Profiles.tsx` — not ported yet (stub).
//!
//! To port: keep this module and the `Profiles` struct, put the page's local
//! state in the struct, and draw it in `ui` following `general.rs`. The TSX
//! uses: `Modal`, `ChipsInput`, `Select`, `Toggle`, `badge`, `empty_state`,
//! and `util::profiles` (starters, summaries, tri-state overrides).

use egui::Ui;

use super::{Cx, Page};

#[derive(Default)]
pub struct Profiles {}

impl Page for Profiles {
    fn ui(&mut self, ui: &mut Ui, _cx: &mut Cx<'_>) {
        super::not_ported(ui, "Profiles", "pages/Profiles.tsx");
    }
}
