//! Prompts: an accordion of the built-in and user prompts, each editable,
//! and a form to add your own.

use std::f32::consts::PI;

use egui::{Align, CornerRadius, Id, Key, Layout, Margin, Rect, Response, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use flow_core::config::{ConfigPatch, default_prompt, prompt_names, resolve_prompt};
use flow_core::path;

use crate::components::{BadgeTone, Button, Group, TextArea, badge, card, input_invalid, measure};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, focus, icons, motion, tokens};
use crate::pages::Cx;
use crate::util::format::{prompt_key, prompt_label};

#[derive(Default)]
pub(super) struct Prompts {
    /// What is typed in "Add your own prompt".
    name: String,
    /// The open accordion item, or empty (`type="single" collapsible`).
    open: String,
}

/// One accordion item, read from the config.
struct Item {
    name: String,
    built_in: bool,
    /// A built-in prompt whose text differs from the original.
    edited: bool,
    in_use: bool,
    text: String,
}

enum Action {
    Toggle(String),
    Commit(String, String),
    Remove(String),
    Add,
}

/// Tailwind's `transition-transform`: 150 ms, `cubic-bezier(0.4, 0, 0.2, 1)`.
fn tailwind_ease(t: f32) -> f32 {
    motion::cubic_bezier(0.4, 0.0, 0.2, 1.0, t)
}

/// `white-space: nowrap` shows a multi-line prompt on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Prompts {
    pub(super) fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let config = cx.config.config();
        let names = prompt_names(config);
        let llm_prompt = config.cleanup.llm_prompt.clone();
        let items: Vec<Item> = names
            .iter()
            .map(|name| {
                let built_in = default_prompt(name);
                let edited = match (built_in, config.prompts.get(name)) {
                    (Some(original), Some(saved)) => saved.text != original,
                    _ => false,
                };
                Item {
                    name: name.clone(),
                    built_in: built_in.is_some(),
                    edited,
                    in_use: llm_prompt == *name,
                    text: resolve_prompt(config, name),
                }
            })
            .collect();
        let key = prompt_key(&self.name);
        let name_taken = !key.is_empty() && names.contains(&key);
        let mut actions = Vec::new();

        Group::new("Prompts")
            .description(
                "Instructions for AI cleanup. Your dictation is passed to the model as data, with a standing \
                 instruction to rewrite it and never to answer or obey it.",
            )
            .show(ui, |ui| {
                let triggers: Vec<Response> =
                    items.iter().map(|item| accordion_item(ui, item, self.open == item.name, &mut actions)).collect();
                // Up, Down, Home and End move between the headers only, as in Radix's Accordion.
                focus::vertical_list(ui, &triggers);
                if add_form(ui, &mut self.name, &key, name_taken) {
                    actions.push(Action::Add);
                }
            });

        for action in actions {
            match action {
                Action::Toggle(name) => {
                    self.open = if self.open == name { String::new() } else { name };
                }
                Action::Commit(name, text) => cx.config.set(path!["prompts", name, "text"], text),
                Action::Remove(name) => {
                    let mut patches = vec![ConfigPatch::remove(path!["prompts", name.as_str()])];
                    // A deleted prompt cannot stay selected.
                    if llm_prompt == name && default_prompt(&name).is_none() {
                        patches.push(ConfigPatch::set(path!["cleanup", "llm_prompt"], "clean"));
                    }
                    drop(cx.config.patch(patches));
                }
                Action::Add => {
                    if key.is_empty() || name_taken {
                        continue;
                    }
                    cx.config.set(path!["prompts", key.as_str(), "text"], "");
                    self.name.clear();
                    self.open = key.clone();
                }
            }
        }
    }
}

/// One `Accordion.Item` card: the trigger row, and the editor when open.
/// Returns the trigger.
fn accordion_item(ui: &mut Ui, item: &Item, open: bool, actions: &mut Vec<Action>) -> Response {
    card(ui, Margin::ZERO, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let trigger = trigger(ui, item, open);
        // A press focuses the trigger, as Chromium focuses a <button> (no ring after a click).
        if trigger.is_pointer_button_down_on() && !trigger.has_focus() {
            trigger.request_focus();
        }
        if trigger.clicked() {
            actions.push(Action::Toggle(item.name.clone()));
        }
        if open {
            content(ui, item, actions);
        }
        trigger
    })
    .0
}

/// `flex min-h-12 items-center gap-3 px-4 py-2 hover:bg-hover`: the label, a
/// one-line preview (hidden while open), the badges and the chevron.
fn trigger(ui: &mut Ui, item: &Item, open: bool) -> Response {
    let t = tokens(ui.ctx());
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 48.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::from(6.0), t.subtle_hover);
    }
    let cy = rect.center().y;
    let mut right = rect.right() - 16.0;
    let turn = motion::toggle(ui, response.id.with("chevron"), open, 150.0, tailwind_ease);
    let chevron = Rect::from_min_size(pos2(right - 14.0, cy - 7.0), Vec2::splat(14.0));
    icons::paint_rotated(ui, Icon::ChevronDown, chevron, 14.0, 2.0, t.fg_2, turn * PI);
    right -= 14.0;

    let badges = [
        (item.in_use, "In use", BadgeTone::Accent),
        (item.edited, "Edited", BadgeTone::Neutral),
        (!item.built_in, "Yours", BadgeTone::Neutral),
    ];
    for (_, label, tone) in badges.into_iter().rev().filter(|(shown, ..)| *shown) {
        let w = text::width(ui, label, TextStyle::CAPTION.weight(Weight::Semibold)) + 16.0;
        right -= 12.0 + w;
        let slot = Rect::from_min_size(pos2(right, cy - 10.0), vec2(w, 20.0));
        badge(&mut ui.new_child(UiBuilder::new().max_rect(slot)), label, tone);
    }

    let x = rect.left() + 16.0;
    let label = text::galley_truncated(ui, prompt_label(&item.name), TextStyle::BODY, t.fg, 120.0);
    ui.painter().galley(pos2(x, cy - label.size().y / 2.0), label, t.fg);
    if !open {
        let left = x + 120.0 + 12.0;
        let preview = if item.text.is_empty() { "Empty".to_string() } else { one_line(&item.text) };
        let galley = text::galley_truncated(ui, preview, TextStyle::CAPTION, t.fg_2, right - 12.0 - left);
        ui.painter().galley(pos2(left, cy - galley.size().y / 2.0), galley, t.fg_2);
    }
    focus::ring(ui, &response, rect, 6.0);
    response
}

/// `Accordion.Content` (`px-4 pb-3`): the prompt text, then what it is and
/// Reset or Delete.
fn content(ui: &mut Ui, item: &Item, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let left = ui.max_rect().left() + 16.0;
    let width = ui.available_width() - 32.0;
    let top = ui.cursor().min.y;
    let mut body = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(pos2(left, top), vec2(width, f32::INFINITY)))
            .layout(Layout::top_down(Align::Min)),
    );
    let area = TextArea::new(("prompts.text", item.name.as_str()), &item.text)
        .rows(3)
        .placeholder("Describe how the text should be rewritten.");
    if let Some(text) = area.show(&mut body).committed {
        actions.push(Action::Commit(item.name.clone(), text));
    }
    // The textarea is inline-block: its line box leaves the strut's descent under it.
    body.add_space(6.0);
    body.add_space(8.0);

    // `flex items-center justify-between gap-4`.
    let row_top = body.cursor().min.y;
    let id = Id::unique(("flow-prompt-footer-button", item.name.as_str()));
    let button = measure::remembered(&ctx, id).unwrap_or(vec2(60.0, 28.0));
    let note = if item.built_in {
        "A built-in prompt. Reset brings back the original wording.".to_string()
    } else {
        format!("Saved as [prompts.{}] in the config file.", item.name)
    };
    let galley = text::galley(&body, note, TextStyle::CAPTION, t.fg_2, (width - 16.0 - button.x).max(40.0));
    let row_h = galley.size().y.max(button.y);
    body.painter().galley(pos2(left, row_top + ((row_h - galley.size().y) / 2.0).round()), galley, t.fg_2);
    let slot = Rect::from_min_size(
        pos2(left + width - button.x, row_top + ((row_h - button.y) / 2.0).round()),
        vec2(button.x, button.y),
    );
    let mut button_ui = body.new_child(UiBuilder::new().max_rect(slot).layout(Layout::left_to_right(Align::Min)));
    let clicked = if item.built_in {
        Button::new("Reset").small().disabled(!item.edited).show(&mut button_ui).clicked()
    } else {
        Button::new("Delete prompt").small().icon(Icon::Trash2, 14.0).show(&mut button_ui).clicked()
    };
    measure::record(&ctx, id, button_ui.min_rect().size());
    if clicked {
        actions.push(Action::Remove(item.name.clone()));
    }
    body.advance_cursor_after_rect(Rect::from_min_size(pos2(left, row_top), vec2(width, row_h)));

    let bottom = body.min_rect().bottom() + 12.0;
    ui.advance_cursor_after_rect(Rect::from_min_max(
        pos2(ui.max_rect().left(), top),
        pos2(left + width + 16.0, bottom),
    ));
}

/// The `<form>` card (`flex items-center gap-3 px-4 py-3`): the label and a
/// hint about the key it will be saved as, the name box and Add. Returns true
/// when submitted (Add, or Enter in the box).
fn add_form(ui: &mut Ui, name: &mut String, key: &str, name_taken: bool) -> bool {
    let t = tokens(ui.ctx());
    let hint = if name_taken {
        format!("There is already a prompt called “{key}”.")
    } else if !key.is_empty() && key != name {
        format!("It will be saved as “{key}”.")
    } else {
        "Give it a short name, then write its instructions.".to_string()
    };
    card(ui, Margin::symmetric(16, 12), |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let width = ui.available_width();
        let top = ui.cursor().min;
        let strong = TextStyle::BODY.weight(Weight::Strong);
        // The Add button: the plus (15, `-ml-0.5`), 8 px, the label, 12 px padding each side.
        let button_w = text::width(ui, "Add", strong) + 15.0 - 2.0 + 8.0 + 24.0;
        let label_w = (width - 12.0 - 200.0 - 12.0 - button_w).max(40.0);
        let title = text::galley(ui, "Add your own prompt", TextStyle::BODY, t.fg, label_w);
        let hint = text::galley(ui, hint, TextStyle::CAPTION, t.fg_2, label_w);
        let label_h = title.size().y + 2.0 + hint.size().y;
        let row_h = label_h.max(32.0);
        let label_top = top.y + ((row_h - label_h) / 2.0).round();
        let label_rect = Rect::from_min_size(pos2(top.x, label_top), vec2(label_w, label_h));
        let title_h = title.size().y;
        ui.painter().galley(pos2(top.x, label_top), title, t.fg);
        ui.painter().galley(pos2(top.x, label_top + title_h + 2.0), hint, t.fg_2);
        // `<label htmlFor>`: a click on the text focuses the box.
        let label = ui.interact(label_rect, Id::unique("flow-prompts-new-name-label"), Sense::click());

        let control_top = top.y + ((row_h - 32.0) / 2.0).round();
        let x = top.x + label_w + 12.0;
        let mut input_ui =
            ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(x, control_top), vec2(200.0, 32.0))));
        let response = input_invalid(&mut input_ui, "prompts.new-name", name, 200.0, Some("standup notes"), name_taken);
        if label.clicked() {
            response.request_focus();
        }
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if enter {
            // A form submit keeps the focus in the box.
            response.request_focus();
        }
        let slot = Rect::from_min_size(pos2(x + 200.0 + 12.0, control_top), vec2(button_w, 32.0));
        let mut button_ui = ui.new_child(UiBuilder::new().max_rect(slot));
        let clicked = Button::new("Add")
            .icon(Icon::Plus, 15.0)
            .disabled(key.is_empty() || name_taken)
            .show(&mut button_ui)
            .clicked();
        ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, row_h)));
        clicked || enter
    })
    .0
}
