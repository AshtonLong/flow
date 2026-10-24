//! History. PORT: `src/renderer/settings/pages/History.tsx` — not ported yet (stub).
//!
//! To port: keep this module and the `History` struct, put the page's local
//! state in the struct, and draw it in `ui` following `general.rs`. The TSX
//! uses: `Request::HistorySearch` with paging (`cx.link.call`), `Debounced`
//! for the search box, `text_field::input`, `ConfirmDialog`, `Event::DictationEvent`
//! and `util::format::format_when`.

use egui::Ui;

use super::{Cx, Page};

#[derive(Default)]
pub struct History {}

impl Page for History {
    fn ui(&mut self, ui: &mut Ui, _cx: &mut Cx<'_>) {
        super::not_ported(ui, "History", "pages/History.tsx");
    }
}
