//! The window frame Flow draws itself: the window has no native decorations,
//! so this provides the 40 px drag strip, Windows 11-style minimise / maximise
//! / close buttons (the colours Electron's `titleBarOverlay` used: symbols
//! `#ffffff` dark / `#1b1b1b` light, close hover `#c42b1c`) that Windows
//! treats as caption buttons (snap layouts), and resizing from the window
//! edges. Dragging and resizing hand over to Windows
//! (`WM_NCLBUTTONDOWN`), so Aero Snap works.

use egui::{
    Color32, CursorIcon, Id, LayerId, Order, Painter, Pos2, Rect, ResizeDirection, Sense, Stroke, Ui, ViewportCommand,
    pos2, vec2,
};

use crate::platform;

/// Height of the title strip (`h-10`).
pub const TITLE_H: f32 = 40.0;
const BUTTON_W: f32 = 46.0;
/// The inside band of the window edge that resizes it.
const EDGE: f32 = 5.0;
const CORNER: f32 = 12.0;

/// A strip that moves the window when dragged and maximises it on double-click.
pub fn drag_region(ui: &Ui, rect: Rect, id: Id) {
    let response = ui.interact(rect, id, Sense::click());
    let ctx = ui.ctx();
    if response.double_clicked() {
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        return;
    }
    if response.hovered() && ctx.input(|i| i.pointer.primary_pressed()) {
        // The second press of a double-click must not start a move; its release maximises.
        let now = ctx.input(|i| i.time);
        let key = Id::unique("flow-drag-last-press");
        let last: f64 = ctx.data(|d| d.get_temp(key)).unwrap_or(f64::NEG_INFINITY);
        ctx.data_mut(|d| d.insert_temp(key, now));
        if now - last > 0.35 {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Caption {
    Minimize,
    Maximize,
    Close,
}

/// The three caption buttons at the top right of `window`. Returns their rect.
///
/// On Windows the buttons are non-client areas (see [`platform::caption`]):
/// Windows reports their hover, press and click, shows the snap-layouts
/// flyout over maximise, and egui never sees the pointer there, so they keep
/// working above a dialog's backdrop. They are painted on the topmost layer,
/// over any backdrop, as Electron's native overlay buttons were. Without the
/// subclass (tests) egui handles them.
pub fn caption_buttons(ui: &Ui, window: Rect, dark: bool) -> Rect {
    let ctx = ui.ctx().clone();
    let (maximized, focused) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.focused));
    let all = Rect::from_min_max(
        pos2(window.right() - 3.0 * BUTTON_W, window.top()),
        pos2(window.right(), window.top() + TITLE_H),
    );
    let native = platform::caption::installed();
    let ppp = ctx.pixels_per_point();
    if native {
        let px = |v: f32| (v * ppp).round() as i32;
        // The top edge above the buttons still resizes the window.
        let top = if maximized { all.top() } else { all.top() + EDGE };
        platform::caption::set_strip(Some([px(all.left()), px(top), px(all.right()), px(all.bottom())]));
    }
    let (native_hover, native_pressed) = platform::caption::state();
    let native_click = if native { platform::caption::take_click() } else { None };
    let painter = ctx.layer_painter(LayerId::new(Order::Debug, Id::new("flow-caption-buttons")));
    let symbol = if dark { Color32::WHITE } else { Color32::from_rgb(0x1b, 0x1b, 0x1b) };
    for (i, kind) in [Caption::Minimize, Caption::Maximize, Caption::Close].into_iter().enumerate() {
        let rect = Rect::from_min_size(pos2(all.left() + i as f32 * BUTTON_W, all.top()), vec2(BUTTON_W, TITLE_H));
        let (pressed, hovered, clicked) = if native {
            (native_pressed == Some(i), native_hover == Some(i), native_click == Some(i))
        } else {
            let response = ui.interact(rect, Id::unique(("flow-caption", i)), Sense::click());
            (response.is_pointer_button_down_on(), response.hovered(), response.clicked())
        };
        // Windows 11 caption colours (SubtleFillColorSecondary / Tertiary; close is #c42b1c).
        let (fill, mut color) = match (kind, pressed, hovered) {
            (Caption::Close, true, _) => {
                (Color32::from_rgba_unmultiplied(0xc4, 0x2b, 0x1c, 230), Color32::from_white_alpha(179))
            }
            (Caption::Close, false, true) => (Color32::from_rgb(0xc4, 0x2b, 0x1c), Color32::WHITE),
            (_, true, _) => (
                if dark { Color32::from_white_alpha(11) } else { Color32::from_black_alpha(6) },
                symbol.gamma_multiply(0.6),
            ),
            (_, false, true) => {
                (if dark { Color32::from_white_alpha(15) } else { Color32::from_black_alpha(10) }, symbol)
            }
            _ => (Color32::TRANSPARENT, symbol),
        };
        if !focused && !hovered {
            color = color.gamma_multiply(0.45);
        }
        if fill.a() > 0 {
            painter.rect_filled(rect, 0.0, fill);
        }
        paint_symbol(&painter, ppp, rect.center(), kind, maximized, color);
        if clicked {
            match kind {
                Caption::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                Caption::Maximize => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
                Caption::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
    }
    all
}

/// The Segoe Fluent caption glyphs, drawn as 1 px lines in a 10 px box.
fn paint_symbol(painter: &Painter, ppp: f32, center: Pos2, kind: Caption, maximized: bool, color: Color32) {
    // Snap to the pixel grid so 1 px lines stay crisp.
    let snap = |v: f32| ((v * ppp).floor() + 0.5) / ppp;
    let (cx, cy) = (snap(center.x), snap(center.y));
    let stroke = Stroke::new(1.0 / ppp * ppp.round().max(1.0), color);
    match kind {
        Caption::Minimize => {
            painter.line_segment([pos2(cx - 5.0, cy), pos2(cx + 5.0, cy)], stroke);
        }
        Caption::Maximize if !maximized => {
            let r = Rect::from_min_max(pos2(cx - 5.0, cy - 5.0), pos2(cx + 5.0, cy + 5.0));
            painter.rect_stroke(r, 1.5, stroke, egui::StrokeKind::Middle);
        }
        Caption::Maximize => {
            // Restore: a front window and the corner of the one behind it.
            let front = Rect::from_min_max(pos2(cx - 5.0, cy - 3.0), pos2(cx + 3.0, cy + 5.0));
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Middle);
            painter.line_segment([pos2(cx - 3.0, cy - 5.0), pos2(cx + 4.0, cy - 5.0)], stroke);
            painter.line_segment([pos2(cx + 5.0, cy - 4.0), pos2(cx + 5.0, cy + 3.0)], stroke);
            painter.line_segment([pos2(cx + 4.0, cy - 5.0), pos2(cx + 5.0, cy - 4.0)], stroke);
        }
        Caption::Close => {
            painter.line_segment([pos2(cx - 5.0, cy - 5.0), pos2(cx + 5.0, cy + 5.0)], stroke);
            painter.line_segment([pos2(cx + 5.0, cy - 5.0), pos2(cx - 5.0, cy + 5.0)], stroke);
        }
    }
}

/// Resizing from the window edges. Call last in the frame so the edges win
/// over whatever lies beneath them.
pub fn resize_edges(ui: &Ui, window: Rect) {
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let left = pointer.x - window.left() < EDGE;
    let right = window.right() - pointer.x < EDGE;
    let top = pointer.y - window.top() < EDGE;
    let bottom = window.bottom() - pointer.y < EDGE;
    let near_left = pointer.x - window.left() < CORNER;
    let near_right = window.right() - pointer.x < CORNER;
    let near_top = pointer.y - window.top() < CORNER;
    let near_bottom = window.bottom() - pointer.y < CORNER;
    let direction = if (top && near_left) || (left && near_top) {
        Some(ResizeDirection::NorthWest)
    } else if (top && near_right) || (right && near_top) {
        Some(ResizeDirection::NorthEast)
    } else if (bottom && near_left) || (left && near_bottom) {
        Some(ResizeDirection::SouthWest)
    } else if (bottom && near_right) || (right && near_bottom) {
        Some(ResizeDirection::SouthEast)
    } else if top {
        Some(ResizeDirection::North)
    } else if bottom {
        Some(ResizeDirection::South)
    } else if left {
        Some(ResizeDirection::West)
    } else if right {
        Some(ResizeDirection::East)
    } else {
        None
    };
    let Some(direction) = direction else { return };
    // Claim the edge so widgets underneath do not react to it.
    let edge = Rect::from_center_size(pointer, vec2(1.0, 1.0));
    ui.interact(edge, Id::unique("flow-resize-edge"), Sense::click());
    ctx.set_cursor_icon(match direction {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}
