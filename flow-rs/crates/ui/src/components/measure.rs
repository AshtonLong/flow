//! Two-pass layout for flexbox-like rows. egui lays out in one pass, so a row
//! that centres items of unknown size remembers their size from the previous
//! pass; when it changes, the pass is discarded and egui immediately runs
//! another one (`request_discard`), so nothing is ever shown misplaced.

use egui::{Context, Id, Vec2};

/// The size `id` had last pass, if it was measured before.
pub fn remembered(ctx: &Context, id: Id) -> Option<Vec2> {
    ctx.data(|d| d.get_temp::<Vec2>(id))
}

/// Records the size measured this pass. Returns true when it changed, after
/// asking egui to redo the pass with the new size.
pub fn record(ctx: &Context, id: Id, size: Vec2) -> bool {
    let previous = remembered(ctx, id);
    let changed = previous.is_none_or(|p| (p - size).length() > 0.25);
    if changed {
        ctx.data_mut(|d| d.insert_temp(id, size));
        ctx.request_discard("flow-ui: a row was measured");
        // If the discard is declined (too many passes), the next frame corrects it.
        ctx.request_repaint();
    }
    changed
}
