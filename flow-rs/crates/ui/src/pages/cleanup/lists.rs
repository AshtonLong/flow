//! The dictionary and snippet lists: a card with column headings and one
//! grid row per entry, edited through a [`ListDraft`] so half-typed rows stay
//! on screen without being written to the file.

use egui::{Align, Layout, Margin, Rect, Ui, UiBuilder, Vec2, pos2, vec2};
use flow_core::config::{DictionaryEntry, Snippet};
use flow_core::path;
use serde::Serialize;

use crate::components::{Button, ChipsInput, Group, IconButton, TextArea, TextField, card, empty_state};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, icons, tokens};
use crate::pages::Cx;
use crate::util::list_draft::ListDraft;

fn dictionary_complete(row: &DictionaryEntry) -> bool {
    !row.heard.is_empty() && !row.write.trim().is_empty()
}

fn snippet_complete(row: &Snippet) -> bool {
    !row.trigger.trim().is_empty()
}

/// What the list asked for this frame.
enum Edit<T> {
    Add(T),
    Update(u64, T),
    Remove(u64),
}

/// Applies the frame's edits to the draft and writes what became complete.
fn apply<T: Clone + Serialize>(cx: &mut Cx<'_>, draft: &mut ListDraft<T>, key: &'static str, edits: Vec<Edit<T>>) {
    for edit in edits {
        let rows = match edit {
            Edit::Add(value) => draft.add(value),
            Edit::Update(row, value) => draft.update(row, value),
            Edit::Remove(row) => draft.remove(row),
        };
        if let Some(rows) = rows {
            cx.config.set(path![key], rows);
        }
    }
}

/// The columns of `grid-cols-[…] gap-2 px-4`, resolved against the card width.
/// `None` is the `1fr` column.
fn columns(left: f32, width: f32, spec: [Option<f32>; 4]) -> [(f32, f32); 4] {
    let inner = width - 32.0;
    let fixed: f32 = spec.iter().flatten().sum();
    let fr = (inner - fixed - 3.0 * 8.0).max(0.0);
    let mut x = left + 16.0;
    spec.map(|w| {
        let w = w.unwrap_or(fr);
        let column = (x, w);
        x += w + 8.0;
        column
    })
}

/// A child `Ui` for one grid cell (`items-start`: the cell's content starts at the row's top).
fn cell(ui: &mut Ui, (x, w): (f32, f32), top: f32) -> Ui {
    ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(pos2(x, top), vec2(w, f32::INFINITY)))
            .layout(Layout::top_down(Align::Min)),
    )
}

/// The `ArrowRight` between the columns: `mt-[9px] justify-self-center text-fg-3`.
fn arrow(ui: &Ui, (x, w): (f32, f32), top: f32) {
    let t = tokens(ui.ctx());
    let rect = Rect::from_min_size(pos2(x + (w - 14.0) / 2.0, top + 9.0), Vec2::splat(14.0));
    icons::paint(ui, Icon::ArrowRight, rect, 14.0, 2.0, t.fg_3);
}

/// The list card: column headings (`pt-2.5 pb-1`, caption, `text-fg-2`), then
/// rows (`py-1.5`, the last `pb-3`). `row` draws one row at `top` into the
/// columns and returns its height.
fn list_card(
    ui: &mut Ui,
    spec: [Option<f32>; 4],
    headings: [(usize, &str); 2],
    count: usize,
    mut row: impl FnMut(&mut Ui, usize, [(f32, f32); 4], f32) -> f32,
) {
    let t = tokens(ui.ctx());
    card(ui, Margin::ZERO, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let left = ui.max_rect().left();
        let width = ui.available_width();
        let cols = columns(left, width, spec);
        ui.add_space(10.0);
        let top = ui.cursor().min.y;
        for (column, heading) in headings {
            let galley = text::galley(ui, heading, TextStyle::CAPTION, t.fg_2, cols[column].1);
            ui.painter().galley(pos2(cols[column].0, top), galley, t.fg_2);
        }
        ui.advance_cursor_after_rect(Rect::from_min_size(pos2(left, top), vec2(width, 16.0)));
        ui.add_space(4.0);
        for i in 0..count {
            let start = ui.cursor().min.y;
            let top = start + 6.0;
            let height = row(ui, i, cols, top);
            let bottom = top + height + if i + 1 == count { 12.0 } else { 6.0 };
            ui.advance_cursor_after_rect(Rect::from_min_max(pos2(left, start), pos2(left + width, bottom)));
        }
    });
}

/// The "Add word" / "Add snippet" button beside a group heading.
fn add_button<'a>(label: &'a str, clicked: &'a mut bool) -> impl FnOnce(&mut Ui) + 'a {
    move |ui: &mut Ui| {
        if Button::new(label).icon(Icon::Plus, 15.0).show(ui).clicked() {
            *clicked = true;
        }
    }
}

/// Words Flow mishears and what to write instead (`[[dictionary]]`).
#[derive(Default)]
pub(super) struct Dictionary {
    /// Created from the saved list on the first frame (`useListDraft`).
    draft: Option<ListDraft<DictionaryEntry>>,
}

impl Dictionary {
    pub(super) fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let saved = &cx.config.config().dictionary;
        let draft = self.draft.get_or_insert_with(|| ListDraft::new(saved, dictionary_complete));
        draft.sync(saved);
        let rows = draft.rows().to_vec();
        let mut add = false;
        let mut edits = Vec::new();

        Group::new("Dictionary")
            .description(
                "Fix words Flow mishears: names, product terms, acronyms. The right spellings are also given to the \
                 speech model as a hint where it supports one.",
            )
            .action(add_button("Add word", &mut add))
            .show(ui, |ui| {
                if rows.is_empty() {
                    empty_state(ui, "No words yet. Add one when Flow keeps getting a name wrong.", |_| {});
                    return;
                }
                let spec = [None, Some(20.0), Some(200.0), Some(32.0)];
                list_card(ui, spec, [(0, "When Flow hears"), (2, "Write")], rows.len(), |ui, i, cols, top| {
                    let (key, value) = (rows[i].key, &rows[i].value);
                    let mut heard_ui = cell(ui, cols[0], top);
                    let chips = ChipsInput::new(("dictionary.heard", key), &value.heard).placeholder("tory, towery");
                    if let Some(heard) = chips.show(&mut heard_ui) {
                        edits.push(Edit::Update(key, DictionaryEntry { heard, ..value.clone() }));
                    }
                    arrow(ui, cols[1], top);
                    let mut write_ui = cell(ui, cols[2], top);
                    let field =
                        TextField::new(("dictionary.write", key), &value.write).width(cols[2].1).placeholder("Tauri");
                    if let Some(write) = field.show(&mut write_ui).committed {
                        edits.push(Edit::Update(
                            key,
                            DictionaryEntry { write: write.trim().to_string(), ..value.clone() },
                        ));
                    }
                    let mut remove_ui = cell(ui, cols[3], top);
                    let label = format!("Remove {}", if value.write.is_empty() { "this word" } else { &value.write });
                    if IconButton::new(label, Icon::Trash2, 15.0).show(&mut remove_ui).clicked() {
                        edits.push(Edit::Remove(key));
                    }
                    [&heard_ui, &write_ui, &remove_ui].iter().map(|c| c.min_rect().height()).fold(0.0, f32::max)
                });
            });

        if add {
            edits.push(Edit::Add(DictionaryEntry { heard: Vec::new(), write: String::new() }));
        }
        apply(cx, draft, "dictionary", edits);
    }
}

/// Trigger phrases and the text typed in their place (`[[snippets]]`).
#[derive(Default)]
pub(super) struct Snippets {
    draft: Option<ListDraft<Snippet>>,
}

impl Snippets {
    pub(super) fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let saved = &cx.config.config().snippets;
        let draft = self.draft.get_or_insert_with(|| ListDraft::new(saved, snippet_complete));
        draft.sync(saved);
        let rows = draft.rows().to_vec();
        let mut add = false;
        let mut edits = Vec::new();

        Group::new("Snippets")
            .description("Say a trigger phrase and Flow types the saved text in its place.")
            .action(add_button("Add snippet", &mut add))
            .show(ui, |ui| {
                if rows.is_empty() {
                    empty_state(ui, "No snippets yet. Add one for text you type often, like your address.", |_| {});
                    return;
                }
                let spec = [Some(200.0), Some(20.0), None, Some(32.0)];
                list_card(ui, spec, [(0, "When you say"), (2, "Type")], rows.len(), |ui, i, cols, top| {
                    let (key, value) = (rows[i].key, &rows[i].value);
                    let mut trigger_ui = cell(ui, cols[0], top);
                    let field = TextField::new(("snippets.trigger", key), &value.trigger)
                        .width(cols[0].1)
                        .placeholder("my address");
                    if let Some(trigger) = field.show(&mut trigger_ui).committed {
                        edits.push(Edit::Update(key, Snippet { trigger: trigger.trim().to_string(), ..value.clone() }));
                    }
                    arrow(ui, cols[1], top);
                    let mut text_ui = cell(ui, cols[2], top);
                    let area = TextArea::new(("snippets.text", key), &value.text)
                        .rows(2)
                        .placeholder("221B Baker Street, London");
                    if let Some(text) = area.show(&mut text_ui).committed {
                        edits.push(Edit::Update(key, Snippet { text, ..value.clone() }));
                    }
                    let mut remove_ui = cell(ui, cols[3], top);
                    let label =
                        format!("Remove {}", if value.trigger.is_empty() { "this snippet" } else { &value.trigger });
                    if IconButton::new(label, Icon::Trash2, 15.0).show(&mut remove_ui).clicked() {
                        edits.push(Edit::Remove(key));
                    }
                    [&trigger_ui, &text_ui, &remove_ui].iter().map(|c| c.min_rect().height()).fold(0.0, f32::max)
                });
            });

        if add {
            edits.push(Edit::Add(Snippet { trigger: String::new(), text: String::new() }));
        }
        apply(cx, draft, "snippets", edits);
    }
}
