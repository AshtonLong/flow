//! Models. PORT: `src/renderer/settings/pages/Models.tsx` and its sections in
//! `pages/models/` — not ported yet (stub).
//!
//! To port: keep this module and the `Models` struct, and draw the page in
//! `ui` following `general.rs`. The sections live in their own modules, like
//! the TSX: [`cloud_providers`], [`custom_endpoints`], [`test_bench`]. The TSX
//! uses: `cx.catalog` (`snapshot`, `progress`, `download_of`), `util::models`,
//! `badge`, `progress_bar`, `spinner`, `ConfirmDialog`, `Modal`, `Select`, `TextField`.

pub mod cloud_providers;
pub mod custom_endpoints;
pub mod test_bench;

use egui::Ui;

use super::{Cx, Page};

#[derive(Default)]
pub struct Models {}

impl Page for Models {
    fn ui(&mut self, ui: &mut Ui, _cx: &mut Cx<'_>) {
        super::not_ported(ui, "Models", "pages/Models.tsx");
    }
}
