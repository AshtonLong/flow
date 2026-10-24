//! `.tooltip`: Radix tooltips with the window's `delayDuration={500}`. Shown
//! above the trigger (6 px away), flipped below when there is no room; once
//! one has shown, moving to another trigger within 300 ms shows it at once.
//! A press on the trigger hides it until the pointer leaves.

use egui::{Area, Id, Order, Rect, Response, Ui, pos2, vec2};

use crate::design::text::{self, TextStyle};
use crate::design::{shadow, tokens};

const DELAY: f64 = 0.5;
const SKIP_DELAY: f64 = 0.3;
const SIDE_OFFSET: f32 = 6.0;

#[derive(Clone, Copy, Default)]
struct State {
    trigger: Option<Id>,
    since: f64,
    shown: bool,
    suppressed: bool,
    last_closed: f64,
}

fn state_id() -> Id {
    Id::unique("flow-tooltip-state")
}

/// Shows `text` as a tooltip for `response` after the delay.
pub fn show(ui: &Ui, response: &Response, text: &str) {
    let ctx = ui.ctx();
    let now = ctx.input(|i| i.time);
    let mut state: State = ctx.data(|d| d.get_temp(state_id())).unwrap_or_default();
    let id = response.id;
    let active = response.hovered() || (response.has_focus() && crate::design::focus::visible(ctx));
    if active {
        if state.trigger != Some(id) {
            if state.shown {
                state.last_closed = now;
            }
            state = State {
                trigger: Some(id),
                since: now,
                shown: false,
                suppressed: false,
                last_closed: state.last_closed,
            };
        }
        if response.is_pointer_button_down_on() {
            state.suppressed = true;
        }
        let delay = if now - state.last_closed < SKIP_DELAY {
            0.0
        } else {
            DELAY
        };
        let due = now - state.since >= delay;
        if due && !state.suppressed {
            state.shown = true;
            paint(ui, response.rect, text);
        } else if !due {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                delay - (now - state.since),
            ));
        }
    } else if state.trigger == Some(id) {
        if state.shown {
            state.last_closed = now;
        }
        state.trigger = None;
        state.shown = false;
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), state));
}

fn paint(ui: &Ui, anchor: Rect, label: &str) {
    let ctx = ui.ctx();
    let t = tokens(ctx);
    let galley = text::galley(ui, label, TextStyle::CAPTION, t.fg, 320.0);
    let size = galley.size() + vec2(16.0, 10.0);
    let screen = ctx.content_rect();
    let mut x = anchor.center().x - size.x / 2.0;
    x = x.clamp(
        screen.left() + 8.0,
        (screen.right() - 8.0 - size.x).max(screen.left() + 8.0),
    );
    let mut y = anchor.top() - SIDE_OFFSET - size.y;
    if y < screen.top() + 4.0 {
        y = anchor.bottom() + SIDE_OFFSET;
    }
    let rect = Rect::from_min_size(pos2(x, y), size).round_ui();
    Area::new(Id::unique("flow-tooltip"))
        .order(Order::Tooltip)
        .fixed_pos(rect.min)
        .interactable(false)
        .show(ctx, |ui| {
            let painter = ui.painter();
            shadow::paint_surface(painter, rect, 4.0, t.flyout, &t.shadow_flyout, None);
            painter.galley(pos2(rect.left() + 8.0, rect.top() + 5.0), galley, t.fg);
            ui.allocate_rect(rect, egui::Sense::hover());
        });
}

trait RoundUi {
    fn round_ui(self) -> Self;
}

impl RoundUi for Rect {
    fn round_ui(self) -> Self {
        Rect::from_min_size(
            pos2(self.min.x.round(), self.min.y.round()),
            vec2(self.width().round(), self.height().round()),
        )
    }
}
