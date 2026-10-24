//! The 220 px navigation rail: the "flow|" mark, the page list (vertical
//! tabs, arrow keys move between them) and the status footer. Port of the
//! left column of `App.tsx` and the `.nav-item` / `.logo` rules.

use egui::epaint::StrokeKind;
use egui::{CornerRadius, Id, Key, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use flow_core::catalog::is_cloud_model_id;

use super::chrome::TITLE_H;
use crate::components::button::link;
use crate::design::text::{self, TextStyle};
use crate::design::{Weight, focus, icons, motion, tokens};
use crate::pages::PageId;

pub const RAIL_W: f32 = 220.0;

/// The mark: "flow" in the logo face, then the caret that types it.
pub fn logo(ui: &Ui, left: f32, center_y: f32) {
    let t = tokens(ui.ctx());
    // `.logo`: 19 px, weight 880, letter-spacing -0.04em, line-height 1.
    let style = TextStyle::new(19.0, 19.0).logo().tracking(-0.04);
    let galley = text::galley(ui, "flow", style, t.fg, f32::INFINITY);
    let top = center_y - 19.0 / 2.0;
    let baseline = top + text::baseline(&galley);
    let word_w = galley.size().x;
    ui.painter().galley(pos2(left, top), galley, t.fg);
    // `.logo-caret`: 0.11em wide (at least 2 px), 0.78em tall, 0.09em after the
    // word, its bottom 0.04em below the baseline.
    let em: f32 = 19.0;
    let caret_w = (0.11 * em).max(2.0);
    let caret_h = 0.78 * em;
    let x = left + word_w + 0.09 * em;
    let bottom = baseline + 0.04 * em;
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(x, bottom - caret_h), pos2(x + caret_w, bottom)),
        0.0,
        t.fg,
    );
}

/// The page list. Returns the page to open when one was picked.
pub fn nav_items(ui: &mut Ui, rect: Rect, current: PageId) -> Option<PageId> {
    let t = tokens(ui.ctx());
    let mut picked = None;
    let mut y = rect.top() + 8.0;
    let mut focused_index = None;
    let mut responses = Vec::new();
    for (i, page) in PageId::ALL.into_iter().enumerate() {
        let item = Rect::from_min_size(pos2(rect.left() + 8.0, y), vec2(rect.width() - 16.0, 36.0));
        let response = ui.interact(
            item,
            Id::unique(("flow-nav", page.as_str())),
            Sense::click(),
        );
        let active = page == current;
        let hover = motion::toggle(
            ui,
            response.id.with("hover"),
            response.hovered() && !active,
            200.0,
            motion::ease,
        );
        let painter = ui.painter();
        let r = CornerRadius::from(9.0);
        if active {
            painter.rect_filled(item, r, t.accent_fill);
            if let Some(outline) = t.hc_outline {
                painter.rect_stroke(item, r, Stroke::new(2.0, outline), StrokeKind::Inside);
            }
        } else if hover > 0.0 {
            painter.rect_filled(item, r, t.subtle_hover.gamma_multiply(hover));
        }
        let color = if active { t.on_accent } else { t.fg };
        let icon_rect = Rect::from_min_size(
            pos2(item.left() + 12.0, item.center().y - 8.0),
            Vec2::splat(16.0),
        );
        icons::paint(ui, page.icon(), icon_rect, 16.0, 1.75, color);
        let weight = if active {
            Weight::Strong
        } else {
            Weight::Medium
        };
        let galley = text::galley(
            ui,
            page.label(),
            TextStyle::BODY.weight(weight),
            color,
            f32::INFINITY,
        );
        ui.painter().galley(
            pos2(
                icon_rect.right() + 12.0,
                item.center().y - galley.size().y / 2.0,
            ),
            galley,
            color,
        );
        focus::ring(ui, &response, item, 9.0);
        if response.clicked() && !active {
            picked = Some(page);
        }
        if response.has_focus() {
            focused_index = Some(i);
        }
        responses.push(response);
        y += 36.0 + 4.0;
    }
    // Vertical tabs: Up and Down move to the neighbouring page and open it.
    if let Some(i) = focused_index {
        let (up, down) = ui.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            )
        });
        let next = if up {
            i.checked_sub(1)
        } else if down {
            (i + 1 < PageId::ALL.len()).then_some(i + 1)
        } else {
            None
        };
        if let Some(next) = next {
            responses[next].request_focus();
            picked = Some(PageId::ALL[next]);
        }
    }
    picked
}

/// What the status footer shows.
pub struct Status<'a> {
    pub paused: bool,
    pub active_model: &'a str,
    pub model_label: String,
}

/// The footer: active model (or paused) and the Pause/Resume link. Returns
/// true when the link was clicked.
pub fn status_footer(ui: &mut Ui, rect: Rect, status: &Status<'_>) -> bool {
    let t = tokens(ui.ctx());
    // `mx-2 border-t border-line px-2 pt-3 pb-3`
    let top = rect.bottom() - footer_height();
    ui.painter().line_segment(
        [
            pos2(rect.left() + 8.0, top + 0.5),
            pos2(rect.right() - 8.0, top + 0.5),
        ],
        Stroke::new(1.0, t.line),
    );
    let left = rect.left() + 16.0;
    let width = rect.width() - 32.0;
    let mut y = top + 1.0 + 12.0;
    let dot = if status.paused { t.warn } else { t.ok };
    ui.painter()
        .circle_filled(pos2(left + 4.0, y + 8.0), 4.0, dot);
    let line = if status.paused {
        "Dictation paused"
    } else if is_cloud_model_id(status.active_model) {
        "Active model (cloud)"
    } else {
        "Active model"
    };
    let caption = text::galley(ui, line, TextStyle::CAPTION, t.fg_2, f32::INFINITY);
    ui.painter()
        .galley(pos2(left + 8.0 + 8.0, y), caption, t.fg_2);
    y += 16.0 + 4.0;
    let model = text::galley_truncated(
        ui,
        status.model_label.as_str(),
        TextStyle::BODY,
        t.fg,
        width,
    );
    let model_rect = Rect::from_min_size(pos2(left, y), model.size());
    ui.painter().galley(model_rect.min, model, t.fg);
    // `title=` on the truncated name.
    let hover = ui.interact(model_rect, Id::unique("flow-footer-model"), Sense::hover());
    crate::components::tooltip::show(ui, &hover, &status.model_label);
    y += 20.0 + 4.0;
    let mut child = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(left, y), vec2(width, 16.0))),
    );
    let label = if status.paused {
        "Resume dictation"
    } else {
        "Pause dictation"
    };
    link(&mut child, label, TextStyle::CAPTION).clicked()
}

/// `1 + 12 + 16 + 4 + 20 + 22 + 12`: the link is an inline-block on a 20 px
/// body line, so its line box is 22 px (4 px margin, 12 px above the baseline
/// of the caption text, the body strut 5.5 px below it).
pub fn footer_height() -> f32 {
    87.0
}

/// The logo strip at the top of the rail (`drag h-10 px-4`).
pub fn logo_strip(ui: &Ui, rail: Rect) {
    logo(ui, rail.left() + 16.0, rail.top() + TITLE_H / 2.0);
}
