//! A Windows-style combo box: a keycap trigger and a Radix-like flyout
//! (`components/Select.tsx`, `.flyout`, `.flyout-item`).
//!
//! The flyout opens on press below the trigger (above when there is more room
//! there), at least as wide as the trigger. The current value is marked with
//! the accent bar; the pointer or the arrow keys highlight an item; Enter or
//! Space picks it, Escape or a press outside closes. While open, the rest of
//! the window does not react (Radix makes the page inert the same way).

use std::hash::Hash;

use egui::{
    Area, Color32, CornerRadius, EventFilter, Id, Key, Modifiers, Order, Pos2, Rect, Response, Sense, Ui, Vec2, pos2,
    vec2,
};

use super::button::{Variant, keycap_state, paint_keycap};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, focus, icons, shadow, tokens};

/// One entry of a [`Select`].
#[derive(Debug, Clone, PartialEq)]
pub struct SelectOption<T> {
    pub value: T,
    pub label: String,
    /// Shown after the label in the list, e.g. a price or "Recommended".
    pub hint: Option<String>,
    pub disabled: bool,
}

impl<T> SelectOption<T> {
    pub fn new(value: T, label: impl Into<String>) -> Self {
        Self { value, label: label.into(), hint: None, disabled: false }
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

#[derive(Debug, Clone, Default)]
struct State {
    open: bool,
    highlighted: Option<usize>,
    /// The pass the flyout opened in: its own press must not close it.
    opened_pass: u64,
    scroll: f32,
    typed: String,
    typed_at: f64,
    /// The press that opened the flyout has not been released yet.
    opening_press: bool,
}

/// What a select reported.
pub struct SelectResponse<T> {
    pub response: Response,
    /// The newly picked value.
    pub changed: Option<T>,
}

pub struct Select<'a, T> {
    id: Id,
    value: Option<&'a T>,
    options: &'a [SelectOption<T>],
    width: f32,
    disabled: bool,
    placeholder: Option<&'a str>,
}

/// Opens the select with this `id_salt` as if it had been pressed (tests, demos).
pub fn open_select(ctx: &egui::Context, id_salt: impl Hash + std::fmt::Debug) {
    let id = Id::unique(("flow-select", id_salt));
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(id, State { open: true, opened_pass: pass, ..State::default() }));
}

const ITEM_H: f32 = 34.0;
const PAD: f32 = 4.0;
const SIDE_OFFSET: f32 = 4.0;
const COLLISION: f32 = 12.0;

impl<'a, T: Clone + PartialEq> Select<'a, T> {
    /// `id_salt` must be unique among the selects of a page (use the setting's path).
    pub fn new(id_salt: impl Hash + std::fmt::Debug, value: Option<&'a T>, options: &'a [SelectOption<T>]) -> Self {
        Self {
            id: Id::unique(("flow-select", id_salt)),
            value,
            options,
            width: 200.0,
            disabled: false,
            placeholder: None,
        }
    }

    /// Trigger width in points (default 200).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn placeholder(mut self, placeholder: &'a str) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    pub fn show(self, ui: &mut Ui) -> SelectResponse<T> {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let sense = if self.disabled { Sense::hover() } else { Sense::click() };
        let (rect, _) = ui.allocate_exact_size(vec2(self.width, 32.0), Sense::hover());
        let trigger = ui.interact(rect, self.id, sense);
        let mut state: State = ctx.data(|d| d.get_temp(self.id)).unwrap_or_default();
        let selected = self.value.and_then(|v| self.options.iter().position(|o| &o.value == v));

        // Open on press, or Enter / Space / arrows on the focused trigger.
        if !self.disabled && !state.open {
            let pressed = trigger.hovered() && ctx.input(|i| i.pointer.primary_pressed());
            let keyed = trigger.has_focus()
                && ctx.input(|i| {
                    [Key::Enter, Key::Space, Key::ArrowDown, Key::ArrowUp].iter().any(|k| i.key_pressed(*k))
                });
            if pressed || keyed {
                state = State {
                    open: true,
                    highlighted: selected.or_else(|| self.first_enabled(0, 1)),
                    opened_pass: ctx.cumulative_pass_nr(),
                    opening_press: pressed,
                    ..State::default()
                };
                trigger.request_focus();
            }
        }
        if self.disabled {
            state.open = false;
        }

        // The trigger: a keycap, the value on the left, the chevron on the right.
        if ui.is_rect_visible(rect.expand(4.0)) {
            let (key, color) = keycap_state(ui, &trigger, Variant::Standard, false, self.disabled);
            paint_keycap(ui.painter(), rect, 8.0, &key);
            let face = rect.translate(vec2(0.0, key.shift));
            let label = selected.map(|i| self.options[i].label.as_str()).or(self.placeholder).unwrap_or("");
            let text_color = if selected.is_none() && !self.disabled { t.fg_3 } else { color };
            let max_w = self.width - 24.0 - 12.0 - 14.0;
            let galley = text::galley_truncated(ui, label, TextStyle::BODY.weight(Weight::Strong), text_color, max_w);
            ui.painter().galley(pos2(face.left() + 12.0, face.center().y - galley.size().y / 2.0), galley, text_color);
            let chevron = Rect::from_center_size(pos2(face.right() - 12.0 - 7.0, face.center().y), Vec2::splat(14.0));
            icons::paint(ui, Icon::ChevronDown, chevron, 14.0, 2.0, t.fg_2);
            focus::ring(ui, &trigger, face, 8.0);
        }

        let mut changed = None;
        if state.open {
            ctx.memory_mut(|m| {
                m.set_focus_lock_filter(
                    self.id,
                    EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                )
            });
            changed = self.flyout(ui, rect, &mut state);
            if !state.open {
                trigger.request_focus();
            }
        }
        ctx.data_mut(|d| d.insert_temp(self.id, state));
        SelectResponse { response: trigger, changed }
    }

    fn first_enabled(&self, from: usize, step: isize) -> Option<usize> {
        let mut i = from as isize;
        while i >= 0 && (i as usize) < self.options.len() {
            if !self.options[i as usize].disabled {
                return Some(i as usize);
            }
            i += step;
        }
        None
    }

    fn flyout(&self, ui: &Ui, trigger_rect: Rect, state: &mut State) -> Option<T> {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let screen = ctx.content_rect();
        let label_style = TextStyle::BODY;
        let rows: Vec<_> = self
            .options
            .iter()
            .map(|o| {
                let color = if o.disabled { t.fg_disabled } else { t.fg };
                let label = text::galley(ui, o.label.as_str(), label_style, color, f32::INFINITY);
                let hint = o.hint.as_deref().map(|h| text::galley(ui, h, TextStyle::CAPTION, t.fg_2, f32::INFINITY));
                (label, hint)
            })
            .collect();
        let natural = rows
            .iter()
            .map(|(l, h)| 24.0 + l.size().x + h.as_ref().map(|h| 16.0 + h.size().x).unwrap_or(0.0))
            .fold(0.0, f32::max);
        let width = (natural + 2.0 * PAD).max(trigger_rect.width()).min(screen.width() - 2.0 * COLLISION);
        let content_h = self.options.len() as f32 * ITEM_H + 2.0 * PAD;
        let below = screen.bottom() - COLLISION - (trigger_rect.bottom() + SIDE_OFFSET);
        let above = trigger_rect.top() - SIDE_OFFSET - (screen.top() + COLLISION);
        let place_above = content_h > below && above > below;
        let height = content_h.min(if place_above { above } else { below }).max(ITEM_H + 2.0 * PAD);
        let top =
            if place_above { trigger_rect.top() - SIDE_OFFSET - height } else { trigger_rect.bottom() + SIDE_OFFSET };
        let left = trigger_rect.left().min(screen.right() - COLLISION - width).max(screen.left() + COLLISION);
        let flyout = Rect::from_min_size(pos2(left, top), vec2(width, height)).round();
        let max_scroll = (content_h - height).max(0.0);

        // Keyboard (not in the pass that opened the list: its Enter or Space is spent).
        let now = ctx.input(|i| i.time);
        let mut pick: Option<usize> = None;
        let opening = state.opened_pass == ctx.cumulative_pass_nr();
        ctx.input_mut(|i| {
            if opening {
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                state.open = false;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                let from = state.highlighted.map(|h| h + 1).unwrap_or(0);
                state.highlighted = self.first_enabled(from, 1).or(state.highlighted);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                let from = state.highlighted.map(|h| h as isize - 1).unwrap_or(self.options.len() as isize - 1);
                if from >= 0 {
                    state.highlighted = self.first_enabled(from as usize, -1).or(state.highlighted);
                }
            }
            if i.consume_key(Modifiers::NONE, Key::Home) {
                state.highlighted = self.first_enabled(0, 1);
            }
            if i.consume_key(Modifiers::NONE, Key::End) && !self.options.is_empty() {
                state.highlighted = self.first_enabled(self.options.len() - 1, -1);
            }
            // Tab does nothing while the list is open, as in Radix.
            i.consume_key(Modifiers::NONE, Key::Tab);
            if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Space) {
                pick = state.highlighted;
            }
            // Typeahead: jump to the first item that starts with what was typed.
            let typed: String = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(text) if text != " " => Some(text.clone()),
                    _ => None,
                })
                .collect();
            if !typed.is_empty() {
                if now - state.typed_at > 1.0 {
                    state.typed.clear();
                }
                state.typed.push_str(&typed.to_lowercase());
                state.typed_at = now;
                if let Some(found) =
                    self.options.iter().position(|o| !o.disabled && o.label.to_lowercase().starts_with(&state.typed))
                {
                    state.highlighted = Some(found);
                }
            }
        });
        // Keep the highlighted item in view.
        if let Some(h) = state.highlighted {
            let item_top = PAD + h as f32 * ITEM_H;
            if item_top < state.scroll + PAD {
                state.scroll = item_top - PAD;
            } else if item_top + ITEM_H > state.scroll + height - PAD {
                state.scroll = item_top + ITEM_H - height + PAD;
            }
        }
        state.scroll = state.scroll.clamp(0.0, max_scroll);

        let pass = ctx.cumulative_pass_nr();
        let mut picked_by_pointer = None;
        Area::new(self.id.with("flyout")).order(Order::Foreground).fixed_pos(screen.min).show(&ctx, |ui| {
            // The backdrop takes every press outside the flyout.
            ui.allocate_rect(screen, Sense::click());
            let pointer: Option<Pos2> = ctx.input(|i| i.pointer.interact_pos());
            let pressed = ctx.input(|i| i.pointer.any_pressed());
            if pressed && pass != state.opened_pass && pointer.is_some_and(|p| !flyout.contains(p)) {
                state.open = false;
            }
            if ctx.input(|i| i.pointer.any_released()) {
                state.opening_press = false;
            }

            if flyout.contains(pointer.unwrap_or(Pos2::new(-1.0, -1.0))) {
                let delta = ctx.input(|i| i.smooth_scroll_delta.y);
                state.scroll = (state.scroll - delta).clamp(0.0, max_scroll);
            }
            let painter = ui.painter().clone();
            shadow::paint_surface(&painter, flyout, 10.0, t.flyout, &t.shadow_flyout, t.hc_border);
            let clip = flyout.shrink(1.0);
            let painter = painter.with_clip_rect(clip);
            let mut hovered_any = None;
            for (i, (label, hint)) in rows.into_iter().enumerate() {
                let item = Rect::from_min_size(
                    pos2(flyout.left() + PAD, flyout.top() + PAD + i as f32 * ITEM_H - state.scroll),
                    vec2(width - 2.0 * PAD, ITEM_H),
                );
                if !item.intersects(clip) {
                    continue;
                }
                let option = &self.options[i];
                let response = ui.interact(item.intersect(clip), self.id.with(("item", i)), Sense::click());
                if response.hovered() && !option.disabled {
                    hovered_any = Some(i);
                }
                if response.clicked() && !option.disabled && !state.opening_press {
                    picked_by_pointer = Some(i);
                }
                let checked = self.value.is_some_and(|v| &option.value == v);
                let highlighted = state.highlighted == Some(i);
                if checked || highlighted {
                    painter.rect_filled(item, CornerRadius::from(7.0), t.subtle_hover);
                }
                if let Some(outline) = t.hc_outline
                    && (checked || highlighted)
                {
                    painter.rect_stroke(
                        item.shrink(2.0),
                        CornerRadius::from(5.0),
                        egui::Stroke::new(2.0, outline),
                        egui::StrokeKind::Inside,
                    );
                }
                if checked {
                    let bar = Rect::from_min_size(pos2(item.left(), item.center().y - 8.0), vec2(3.0, 16.0));
                    painter.rect_filled(bar, CornerRadius::from(2.0), t.accent_fill);
                }
                let y = item.center().y;
                if let Some(hint) = hint {
                    painter.galley(pos2(item.right() - 12.0 - hint.size().x, y - hint.size().y / 2.0), hint, t.fg_2);
                }
                painter.galley(pos2(item.left() + 12.0, y - label.size().y / 2.0), label, Color32::PLACEHOLDER);
            }
            // The pointer highlights what it is over; leaving the items clears it.
            let pointer_moved = ctx.input(|i| i.pointer.delta() != Vec2::ZERO);
            if pointer_moved && pointer.is_some_and(|p| flyout.contains(p)) {
                state.highlighted = hovered_any;
            }
        });
        if let Some(i) = pick.or(picked_by_pointer) {
            state.open = false;
            let option = &self.options[i];
            if self.value != Some(&option.value) {
                return Some(option.value.clone());
            }
        }
        None
    }
}
