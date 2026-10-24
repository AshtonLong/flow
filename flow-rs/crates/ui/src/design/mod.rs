//! The design system: tokens (colours and shadows per theme), the Recursive
//! families, text styles, painted box shadows, lucide icons, motion and focus
//! rings. Components in [`crate::components`] are built from these.
//!
//! Widgets read the current tokens with [`tokens`]`(ui.ctx())`.

pub mod focus;
pub mod fonts;
pub mod icons;
pub mod motion;
pub mod shadow;
pub mod text;
pub mod tokens;

use std::sync::Arc;

use egui::epaint::FontColorTransferFunction;
use egui::style::{ScrollStyle, Selection, TextCursorStyle};
use egui::{Context, Id, Margin, Stroke, Visuals};

pub use fonts::Weight;
pub use icons::Icon;
pub use text::{Rich, TextStyle};
pub use tokens::{BoxShadow, Tokens};

fn tokens_id() -> Id {
    Id::unique("flow-tokens")
}

/// The tokens of the current theme.
pub fn tokens(ctx: &Context) -> Arc<Tokens> {
    ctx.data(|d| d.get_temp::<Arc<Tokens>>(tokens_id()))
        .unwrap_or_else(|| Arc::new(Tokens::light()))
}

/// Installs new tokens and the egui style that goes with them. Cheap to call
/// every frame: does nothing when the tokens did not change.
pub fn apply(ctx: &Context, tokens: Tokens) {
    if *self::tokens(ctx) == tokens
        && ctx.data(|d| d.get_temp::<Arc<Tokens>>(tokens_id()).is_some())
    {
        return;
    }
    let visuals = visuals(&tokens);
    let theme = if tokens.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.data_mut(|d| d.insert_temp(tokens_id(), Arc::new(tokens)));
    // egui keeps a dark and a light style; pin the one in use and give both the same look.
    ctx.set_theme(theme);
    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();
        style.spacing.item_spacing = egui::Vec2::ZERO;
        style.spacing.button_padding = egui::Vec2::ZERO;
        style.spacing.window_margin = Margin::ZERO;
        style.spacing.menu_margin = Margin::ZERO;
        style.spacing.indent = 0.0;
        style.spacing.interact_size = egui::vec2(0.0, 0.0);
        style.spacing.scroll = ScrollStyle {
            fade: egui::style::ScrollFadeStyle {
                strength: 0.0,
                size: 0.0,
            },
            ..ScrollStyle::floating()
        };
        style.interaction.selectable_labels = false;
        style.interaction.tooltip_delay = 0.5;
        style.animation_time = 0.0;
        let body = TextStyle::BODY.font_id();
        for font in style.text_styles.values_mut() {
            *font = body.clone();
        }
    });
}

fn visuals(t: &Tokens) -> Visuals {
    let mut v = if t.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = t.mica;
    v.window_fill = t.flyout;
    v.extreme_bg_color = t.ctl;
    v.faint_bg_color = t.inset;
    v.hyperlink_color = t.fg;
    v.override_text_color = None;
    v.selection = Selection {
        bg_fill: t.voice,
        stroke: Stroke::new(1.0, egui::Color32::BLACK),
    };
    v.text_cursor = TextCursorStyle {
        stroke: Stroke::new(1.0, t.fg),
        ..TextCursorStyle::default()
    };
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.window_stroke = Stroke::NONE;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.fg);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.fg);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.fg);
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.fg);
    v.text_options.color_transfer_function = if t.dark {
        FontColorTransferFunction::DARK_MODE_DEFAULT
    } else {
        FontColorTransferFunction::LIGHT_MODE_DEFAULT
    };
    v
}
