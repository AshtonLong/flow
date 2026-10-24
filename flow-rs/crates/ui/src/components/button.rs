//! Buttons: the keycap `.btn` (a top face and a 3 px wall it presses into),
//! `.btn-accent` (the dark key), `.btn-subtle` (flat), sizes, icon buttons with
//! a tooltip, and `.link`. Port of `components/Button.tsx` and the `.btn*` /
//! `.link` rules in `styles.css`.

use std::borrow::Cow;

use egui::epaint::StrokeKind;
use egui::{
    Color32, CornerRadius, CursorIcon, Painter, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2,
};

use super::tooltip;
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, focus, icons, motion, tokens};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Variant {
    /// The keycap.
    #[default]
    Standard,
    /// The dark keycap: primary actions.
    Accent,
    /// Flat: quiet actions stay flat, only real buttons are keys.
    Subtle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Size {
    /// 32 px tall, 12 px padding.
    #[default]
    Md,
    /// 28 px tall, 10 px padding.
    Sm,
}

impl Size {
    pub fn height(self) -> f32 {
        match self {
            Self::Md => 32.0,
            Self::Sm => 28.0,
        }
    }
    fn padding(self) -> f32 {
        match self {
            Self::Md => 12.0,
            Self::Sm => 10.0,
        }
    }
}

/// An icon inside a button: lucide `size` and `strokeWidth`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconSpec {
    pub icon: Icon,
    pub size: f32,
    pub stroke: f32,
    /// Radians; the IssuesBanner chevron turns 180° when open.
    pub rotation: f32,
}

impl IconSpec {
    pub fn new(icon: Icon, size: f32) -> Self {
        Self {
            icon,
            size,
            stroke: 2.0,
            rotation: 0.0,
        }
    }
}

/// How a keycap is drawn this frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Keycap {
    pub top: Color32,
    pub wall: Color32,
    /// The inset 1 px line.
    pub line: Color32,
    /// Wall height: 3 px at rest, 1 px pressed, 0 disabled.
    pub lift: f32,
    /// The press moves the key 2 px down.
    pub shift: f32,
    /// Draw the wall and its dark edge at all (`box-shadow: none` turns them off).
    pub raised: bool,
    pub inset: bool,
}

/// `box-shadow: inset 0 0 0 1px line, 0 lift 0 wall, 0 lift+1 0 rgb(0 0 0 / .22)`.
pub(crate) fn paint_keycap(painter: &Painter, rect: Rect, radius: f32, key: &Keycap) {
    let face = rect.translate(vec2(0.0, key.shift));
    let r = CornerRadius::from(radius);
    if key.raised && key.lift > 0.0 {
        painter.rect_filled(
            face.translate(vec2(0.0, key.lift + 1.0)),
            r,
            Color32::from_black_alpha(56),
        );
        if key.wall.a() > 0 {
            painter.rect_filled(face.translate(vec2(0.0, key.lift)), r, key.wall);
        }
    } else if key.raised {
        painter.rect_filled(
            face.translate(vec2(0.0, 1.0)),
            r,
            Color32::from_black_alpha(56),
        );
    }
    painter.rect_filled(face, r, key.top);
    if key.inset && key.line.a() > 0 {
        painter.rect_stroke(face, r, Stroke::new(1.0, key.line), StrokeKind::Inside);
    }
}

/// The animated look of a `.btn` for this frame, and its text colour.
pub(crate) fn keycap_state(
    ui: &Ui,
    response: &Response,
    variant: Variant,
    danger: bool,
    disabled: bool,
) -> (Keycap, Color32) {
    let t = tokens(ui.ctx());
    let id = response.id;
    let pressed = !disabled && response.is_pointer_button_down_on();
    let hovered = !disabled && response.hovered();
    let hover = motion::toggle(ui, id.with("flow-btn-hover"), hovered, 83.0, motion::linear);
    let press = motion::toggle(
        ui,
        id.with("flow-btn-press"),
        pressed,
        110.0,
        motion::snappy,
    );
    let text = if disabled {
        t.fg_disabled
    } else if danger {
        t.danger
    } else {
        t.fg
    };
    match variant {
        Variant::Standard => {
            if disabled {
                (
                    Keycap {
                        top: t.ctl_disabled,
                        wall: t.ctl_wall,
                        line: t.ctl_line,
                        lift: 0.0,
                        shift: 0.0,
                        raised: false,
                        inset: true,
                    },
                    text,
                )
            } else {
                let top = motion::mix(t.ctl, t.ctl_hover, hover);
                (
                    Keycap {
                        top,
                        wall: t.ctl_wall,
                        line: t.ctl_line,
                        lift: 3.0 - 2.0 * press,
                        shift: 2.0 * press,
                        raised: true,
                        inset: true,
                    },
                    text,
                )
            }
        }
        Variant::Accent => {
            if disabled {
                (
                    Keycap {
                        top: t.fg_disabled,
                        wall: Color32::TRANSPARENT,
                        line: Color32::TRANSPARENT,
                        lift: 0.0,
                        shift: 0.0,
                        raised: false,
                        inset: false,
                    },
                    t.mica,
                )
            } else {
                let top = motion::mix(t.key_ink_top, t.key_ink_hover, hover);
                let text = if danger { t.danger } else { t.on_accent };
                (
                    Keycap {
                        top,
                        wall: t.key_ink_wall,
                        line: t.ctl_line,
                        lift: 3.0 - 2.0 * press,
                        shift: 2.0 * press,
                        raised: true,
                        inset: true,
                    },
                    text,
                )
            }
        }
        Variant::Subtle => {
            let top = if disabled {
                Color32::TRANSPARENT
            } else if pressed {
                t.subtle_press
            } else {
                motion::mix(Color32::TRANSPARENT, t.subtle_hover, hover)
            };
            (
                Keycap {
                    top,
                    wall: Color32::TRANSPARENT,
                    line: Color32::TRANSPARENT,
                    lift: 0.0,
                    shift: 0.0,
                    raised: false,
                    inset: false,
                },
                text,
            )
        }
    }
}

/// A text button, optionally with a leading icon.
///
/// ```ignore
/// if Button::new("Record").show(ui).clicked() { ... }
/// Button::new("Remove model").variant(Variant::Accent).min_width(120.0).show(ui);
/// ```
pub struct Button<'a> {
    label: Cow<'a, str>,
    icon: Option<IconSpec>,
    trailing: Option<IconSpec>,
    variant: Variant,
    size: Size,
    danger: bool,
    disabled: bool,
    min_width: f32,
    width: Option<f32>,
}

impl<'a> Button<'a> {
    pub fn new(label: impl Into<Cow<'a, str>>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            trailing: None,
            variant: Variant::Standard,
            size: Size::Md,
            danger: false,
            disabled: false,
            min_width: 0.0,
            width: None,
        }
    }
    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }
    pub fn accent(self) -> Self {
        self.variant(Variant::Accent)
    }
    pub fn subtle(self) -> Self {
        self.variant(Variant::Subtle)
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    pub fn small(self) -> Self {
        self.size(Size::Sm)
    }
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// A leading icon (`icon={<Download size={15} />}`).
    pub fn icon(mut self, icon: Icon, size: f32) -> Self {
        self.icon = Some(IconSpec::new(icon, size));
        self
    }
    pub fn icon_spec(mut self, icon: IconSpec) -> Self {
        self.icon = Some(icon);
        self
    }
    /// An icon after the label (the IssuesBanner's chevron).
    pub fn trailing(mut self, icon: IconSpec) -> Self {
        self.trailing = Some(icon);
        self
    }
    /// `min-w-[120px]`
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
    /// A fixed width (`w-full` style layouts).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    fn style(&self) -> TextStyle {
        TextStyle::BODY.weight(if self.variant == Variant::Subtle {
            Weight::Medium
        } else {
            Weight::Strong
        })
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let style = self.style();
        let galley = text::galley(
            ui,
            self.label.as_ref(),
            style,
            Color32::PLACEHOLDER,
            f32::INFINITY,
        );
        let gap = 8.0;
        let icon_w = self.icon.map(|i| i.size - 2.0 + gap).unwrap_or(0.0);
        let trailing_w = self.trailing.map(|i| gap + i.size).unwrap_or(0.0);
        let content = icon_w + galley.size().x + trailing_w;
        let height = self.size.height();
        let width = self
            .width
            .unwrap_or(content + 2.0 * self.size.padding())
            .max(self.min_width);
        let sense = if self.disabled {
            Sense::hover()
        } else {
            Sense::click()
        };
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
        if ui.is_rect_visible(rect.expand(4.0)) {
            let (key, color) =
                keycap_state(ui, &response, self.variant, self.danger, self.disabled);
            let painter = ui.painter();
            paint_keycap(painter, rect, 8.0, &key);
            let face = rect.translate(vec2(0.0, key.shift));
            let mut x = face.center().x - content / 2.0;
            if let Some(icon) = self.icon {
                // `-ml-0.5`: the icon sits 2 px into the padding.
                let icon_rect = Rect::from_min_size(
                    pos2(x - 2.0, face.center().y - icon.size / 2.0),
                    Vec2::splat(icon.size),
                );
                icons::paint_rotated(
                    ui,
                    icon.icon,
                    icon_rect,
                    icon.size,
                    icon.stroke,
                    color,
                    icon.rotation,
                );
                x += icon_w;
            }
            let text_pos = pos2(x, face.center().y - galley.size().y / 2.0);
            x += galley.size().x;
            painter.galley(text_pos, galley, color);
            if let Some(icon) = self.trailing {
                let icon_rect = Rect::from_min_size(
                    pos2(x + gap, face.center().y - icon.size / 2.0),
                    Vec2::splat(icon.size),
                );
                icons::paint_rotated(
                    ui,
                    icon.icon,
                    icon_rect,
                    icon.size,
                    icon.stroke,
                    color,
                    icon.rotation,
                );
            }
            focus::ring(ui, &response, face, 8.0);
        }
        response
    }
}

/// A square button with only an icon. The label is its tooltip (and its
/// accessible name). Subtle by default, like `IconButton` in the TSX.
pub struct IconButton<'a> {
    label: Cow<'a, str>,
    icon: IconSpec,
    variant: Variant,
    size: Size,
    danger: bool,
    disabled: bool,
}

impl<'a> IconButton<'a> {
    /// `icon_size` is lucide's `size` (e.g. `<X size={14} />`).
    pub fn new(label: impl Into<Cow<'a, str>>, icon: Icon, icon_size: f32) -> Self {
        Self {
            label: label.into(),
            icon: IconSpec::new(icon, icon_size),
            variant: Variant::Subtle,
            size: Size::Md,
            danger: false,
            disabled: false,
        }
    }
    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }
    pub fn small(mut self) -> Self {
        self.size = Size::Sm;
        self
    }
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn stroke(mut self, stroke: f32) -> Self {
        self.icon.stroke = stroke;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let side = self.size.height();
        let sense = if self.disabled {
            Sense::hover()
        } else {
            Sense::click()
        };
        let (rect, response) = ui.allocate_exact_size(vec2(side, side), sense);
        if ui.is_rect_visible(rect.expand(4.0)) {
            let (key, color) =
                keycap_state(ui, &response, self.variant, self.danger, self.disabled);
            paint_keycap(ui.painter(), rect, 8.0, &key);
            let face = rect.translate(vec2(0.0, key.shift));
            icons::paint_rotated(
                ui,
                self.icon.icon,
                face,
                self.icon.size,
                self.icon.stroke,
                color,
                self.icon.rotation,
            );
            focus::ring(ui, &response, face, 8.0);
        }
        // A disabled button gets no pointer events in Chromium, so Radix shows no tooltip.
        if !self.disabled {
            tooltip::show(ui, &response, &self.label);
        }
        response
    }
}

/// `.link`: inline text, weight 620, underlined (thicker on hover), pointer cursor.
pub fn link(ui: &mut Ui, label: &str, style: TextStyle) -> Response {
    let t = tokens(ui.ctx());
    link_colored(ui, label, style, t.fg)
}

/// [`link`] in a given colour (`color: inherit`).
pub fn link_colored(ui: &mut Ui, label: &str, style: TextStyle, color: Color32) -> Response {
    let style = style.weight(Weight::Strong);
    let galley = text::galley(ui, label, style, color, f32::INFINITY);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let baseline = rect.top() + text::baseline(&galley);
    let thickness = style.size * if response.hovered() { 0.16 } else { 0.09 };
    let underline_top = baseline + 0.22 * style.size;
    ui.painter().rect_filled(
        Rect::from_min_max(
            pos2(rect.left(), underline_top),
            pos2(rect.right(), underline_top + thickness.max(1.0)),
        ),
        0.0,
        color,
    );
    ui.painter().galley(rect.min, galley, color);
    focus::ring(ui, &response, rect, 2.0);
    response
}
