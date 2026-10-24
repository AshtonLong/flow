//! Slim warning shown on every page while `config.toml` has invalid values.
//! Port of `components/IssuesBanner.tsx`.

use egui::epaint::StrokeKind;
use egui::{
    Align, CornerRadius, Id, Layout, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use flow_core::config::ConfigIssue;
use flow_core::protocol::settings::Request;

use super::button::{Button, IconSpec};
use super::measure;
use crate::backend::Link;
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, icons, tokens};
use crate::util::format::plural;
use crate::util::patch::format_path;

/// Nothing when there are no issues.
pub fn issues_banner(ui: &mut Ui, issues: &[ConfigIssue], link: &Link) {
    if issues.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let id = Id::unique("flow-issues-banner");
    let mut open: bool = ctx.data(|d| d.get_temp(id)).unwrap_or(false);
    let slot = ui.painter().add(Shape::Noop);
    let width = ui.available_width();
    let top = ui.cursor().min;

    // Header: `py-1.5 pr-1.5 pl-3 gap-2.5`.
    let buttons_size = measure::remembered(&ctx, id.with("buttons")).unwrap_or(vec2(0.0, 28.0));
    let header_h = buttons_size.y.max(20.0) + 12.0;
    let cy = top.y + 1.0 + header_h / 2.0;
    icons::paint(
        ui,
        Icon::TriangleAlert,
        Rect::from_center_size(pos2(top.x + 1.0 + 12.0 + 8.0, cy), Vec2::splat(16.0)),
        16.0,
        2.0,
        t.warn,
    );
    let message = format!("config.toml has {}", plural(issues.len(), "problem", None));
    let text_left = top.x + 1.0 + 12.0 + 16.0 + 10.0;
    let text_w = width - 2.0 - 12.0 - 16.0 - 10.0 - 10.0 - buttons_size.x - 6.0;
    let galley = text::galley_truncated(ui, message, TextStyle::BODY, t.fg, text_w.max(40.0));
    ui.painter()
        .galley(pos2(text_left, cy - galley.size().y / 2.0), galley, t.fg);
    let area = Rect::from_min_max(
        pos2(top.x + width - 1.0 - 6.0 - buttons_size.x, top.y + 7.0),
        pos2(top.x + width - 7.0, top.y + 7.0 + buttons_size.y),
    );
    let mut buttons = ui.new_child(
        UiBuilder::new()
            .max_rect(area)
            .layout(Layout::left_to_right(Align::Center)),
    );
    buttons.spacing_mut().item_spacing = vec2(8.0, 0.0);
    if Button::new("Open config file")
        .small()
        .subtle()
        .show(&mut buttons)
        .clicked()
    {
        link.send(Request::ConfigOpenFile);
    }
    let rotation = crate::design::motion::toggle(
        ui,
        id.with("chevron"),
        open,
        150.0,
        crate::design::motion::ease,
    ) * std::f32::consts::PI;
    let chevron = IconSpec {
        rotation,
        ..IconSpec::new(Icon::ChevronDown, 14.0)
    };
    if Button::new(if open { "Hide" } else { "Show" })
        .small()
        .subtle()
        .trailing(chevron)
        .show(&mut buttons)
        .clicked()
    {
        open = !open;
    }
    measure::record(&ctx, id.with("buttons"), buttons.min_rect().size());
    let mut bottom = top.y + 1.0 + header_h;

    if open {
        // `border-t border-line px-3 py-2 text-caption`.
        let list_top = bottom;
        ui.painter().line_segment(
            [
                pos2(top.x, list_top + 0.5),
                pos2(top.x + width, list_top + 0.5),
            ],
            Stroke::new(1.0, t.line),
        );
        let rect = Rect::from_min_size(
            pos2(top.x + 1.0 + 12.0, list_top + 1.0 + 8.0),
            vec2(width - 2.0 - 24.0, f32::INFINITY),
        );
        let mut list = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
        );
        text::paragraph(
            &mut list,
            "Each of these falls back to its default until the file is fixed.",
            TextStyle::CAPTION,
            t.fg_2,
        );
        list.add_space(4.0);
        for issue in issues {
            list.add_space(4.0);
            let row_top = list.cursor().min;
            let inner_w = list.available_width();
            let path = text::galley(
                &list,
                format_path(&issue.path),
                TextStyle::CAPTION.mono(),
                t.fg,
                220.0,
            );
            let line = issue.line.map(|l| format!("line {l}")).unwrap_or_default();
            let line_g = text::galley(&list, line, TextStyle::CAPTION, t.fg_2, 56.0);
            let message_w = (inner_w - 220.0 - 56.0 - 32.0).max(40.0);
            let message = text::galley(
                &list,
                issue.message.as_str(),
                TextStyle::CAPTION,
                t.fg,
                message_w,
            );
            let h = path.size().y.max(message.size().y).max(line_g.size().y);
            let painter = list.painter();
            painter.galley(row_top, path, t.fg);
            painter.galley(pos2(row_top.x + 220.0 + 16.0, row_top.y), message, t.fg);
            painter.galley(
                pos2(row_top.x + inner_w - line_g.size().x, row_top.y),
                line_g,
                t.fg_2,
            );
            list.advance_cursor_after_rect(Rect::from_min_size(row_top, vec2(inner_w, h)));
            list.add_space(4.0);
        }
        bottom = list.min_rect().bottom() + 8.0;
    }
    let rect = Rect::from_min_max(top, pos2(top.x + width, bottom + 1.0));
    ui.painter().set(
        slot,
        Shape::Vec(vec![
            Shape::rect_filled(rect, CornerRadius::from(6.0), t.warn_bg),
            Shape::rect_stroke(
                rect,
                CornerRadius::from(6.0),
                Stroke::new(1.0, t.line),
                StrokeKind::Inside,
            ),
        ]),
    );
    ui.allocate_rect(rect, Sense::hover());
    ui.add_space(20.0);
    ctx.data_mut(|d| d.insert_temp(id, open));
}
