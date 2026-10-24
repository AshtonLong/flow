//! A Windows-style toggle with its On/Off state written beside it
//! (`components/Toggle.tsx`, `.switch`), plus `.checkbox` and `.radio`.

use egui::epaint::StrokeKind;
use egui::{Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::design::text::{self, TextStyle};
use crate::design::{Icon, focus, icons, motion, shadow, tokens};

/// The switch: 42×24, an 18 px thumb that slides 18 px over 250 ms
/// (`cubic-bezier(0.16, 1, 0.3, 1)`), the track fading over 200 ms.
pub struct Toggle {
    checked: bool,
    disabled: bool,
    hide_state: bool,
}

/// What a toggle reported.
pub struct ToggleResponse {
    pub response: Response,
    /// The new value when it was flipped this frame.
    pub changed: Option<bool>,
}

impl Toggle {
    pub fn new(checked: bool) -> Self {
        Self {
            checked,
            disabled: false,
            hide_state: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Drops the On/Off text where space is tight.
    pub fn hide_state(mut self) -> Self {
        self.hide_state = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> ToggleResponse {
        let t = tokens(ui.ctx());
        let state = if self.checked { "On" } else { "Off" };
        let state_color = if self.disabled { t.fg_3 } else { t.fg };
        let galley = (!self.hide_state)
            .then(|| text::galley(ui, state, TextStyle::BODY, state_color, f32::INFINITY));
        let text_w = galley.as_ref().map(|g| g.size().x + 12.0).unwrap_or(0.0);
        let (rect, _) = ui.allocate_exact_size(vec2(text_w + 42.0, 24.0), Sense::hover());
        if let Some(galley) = galley {
            let pos = pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(pos, galley, state_color);
        }
        let track = Rect::from_min_size(pos2(rect.right() - 42.0, rect.top()), vec2(42.0, 24.0));
        let id = ui.auto_id_with("flow-toggle");
        let sense = if self.disabled {
            Sense::hover()
        } else {
            Sense::click()
        };
        let response = ui.interact(track, id, sense);
        let changed = response.clicked().then_some(!self.checked);
        let checked = changed.unwrap_or(self.checked);
        paint_switch(ui, &response, track, checked, self.disabled);
        ToggleResponse { response, changed }
    }
}

/// Paints the `.switch` in `track` (42×24).
pub fn paint_switch(ui: &Ui, response: &Response, track: Rect, checked: bool, disabled: bool) {
    let t = tokens(ui.ctx());
    let id = response.id;
    let fill_t = motion::toggle(
        ui,
        id.with("flow-switch-fill"),
        checked,
        200.0,
        motion::ease,
    );
    let thumb_t = motion::toggle(
        ui,
        id.with("flow-switch-thumb"),
        checked,
        250.0,
        motion::snappy,
    );
    let opacity = if disabled { 0.45 } else { 1.0 };
    let fill = motion::mix(t.switch_off, t.accent_fill, fill_t).gamma_multiply(opacity);
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::from(12.0), fill);
    if t.high_contrast {
        // `--switch-off: Canvas` would vanish; the border shows the shape.
        painter.rect_stroke(
            track,
            CornerRadius::from(12.0),
            Stroke::new(1.0, t.switch_line),
            StrokeKind::Inside,
        );
    }
    let knob = if checked { t.on_accent } else { t.switch_knob }.gamma_multiply(opacity);
    let thumb = Rect::from_min_size(
        pos2(track.left() + 3.0 + 18.0 * thumb_t, track.top() + 3.0),
        Vec2::splat(18.0),
    );
    shadow::paint_box_shadows(
        painter,
        thumb,
        9.0,
        &[crate::design::BoxShadow::drop(
            1.0,
            3.0,
            0.0,
            Color32::from_black_alpha(89).gamma_multiply(opacity),
        )],
    );
    painter.rect_filled(thumb, CornerRadius::from(9.0), knob);
    focus::ring(ui, response, track, 12.0);
}

/// `.checkbox`: 20×20, 4 px radius; checked fills with the accent and shows a tick.
pub fn checkbox(ui: &mut Ui, checked: bool, disabled: bool) -> (Response, Option<bool>) {
    let t = tokens(ui.ctx());
    let sense = if disabled {
        Sense::hover()
    } else {
        Sense::click()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), sense);
    let changed = response.clicked().then_some(!checked);
    let checked = changed.unwrap_or(checked);
    let painter = ui.painter();
    let r = CornerRadius::from(4.0);
    if checked {
        painter.rect_filled(rect, r, t.accent_fill);
        icons::paint(ui, Icon::Check, rect, 14.0, 3.0, t.on_accent);
    } else {
        if response.hovered() && !disabled {
            painter.rect_filled(rect, r, t.subtle_hover);
        }
        let border = if disabled {
            t.fg_disabled
        } else {
            t.switch_line
        };
        painter.rect_stroke(rect, r, Stroke::new(1.0, border), StrokeKind::Inside);
    }
    focus::ring(ui, &response, rect, 4.0);
    (response, changed)
}

/// `.radio`: 20×20 circle; checked fills with the accent around a 10 px dot.
pub fn radio(ui: &mut Ui, checked: bool, disabled: bool) -> Response {
    let t = tokens(ui.ctx());
    let sense = if disabled {
        Sense::hover()
    } else {
        Sense::click()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), sense);
    let painter = ui.painter();
    let r = CornerRadius::from(10.0);
    if checked {
        painter.rect_filled(rect, r, t.accent_fill);
        painter.rect_filled(
            Rect::from_center_size(rect.center(), Vec2::splat(10.0)),
            CornerRadius::from(5.0),
            t.on_accent,
        );
    } else {
        if response.hovered() && !disabled {
            painter.rect_filled(rect, r, t.subtle_hover);
        }
        painter.rect_stroke(rect, r, Stroke::new(1.0, t.switch_line), StrokeKind::Inside);
    }
    focus::ring(ui, &response, rect, 10.0);
    response
}
