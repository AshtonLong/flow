//! Profiles: per-app overrides, matched by program name or window title.
//! Port of `pages/Profiles.tsx`; the editor dialog's body is in `editor`.

mod editor;

use egui::{Align, Id, Layout, Margin, Rect, Ui, UiBuilder, Vec2, pos2, vec2};
use flow_core::config::{Profile, prompt_names};
use flow_core::path;
use serde_json::Value;

use super::{Cx, Page};
use crate::components::{
    BadgeTone, Button, ConfirmDialog, Group, IconButton, Modal, SelectOption, badge, card, empty_state, measure,
    page_header,
};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, tokens};
use crate::util::format::prompt_label;
use crate::util::models::{model_label, ready_models};
use crate::util::profiles::{
    Starter, get_override, match_summary, override_summary, serialize_profiles, starters, unique_name,
};

#[derive(Default)]
pub struct Profiles {
    /// The profile open in the editor.
    editing: Option<usize>,
    /// The profile the delete confirmation asks about.
    removing: Option<usize>,
    /// Counts editor openings, so each one starts with fresh field drafts (a remount in React).
    session: u64,
}

/// What a profile card asked for.
enum CardAction {
    Edit,
    Delete,
}

/// Writes the list, leaving defaults and empty sections out of the file.
fn write(cx: &mut Cx<'_>, next: &[Profile]) {
    cx.config.set(path!["profiles"], serialize_profiles(next));
}

/// Appends `profile` under a free name; returns its index.
fn add(cx: &mut Cx<'_>, profiles: &[Profile], profile: Profile) -> usize {
    let mut next = profiles.to_vec();
    next.push(Profile { name: unique_name(profiles, &profile.name), ..profile });
    write(cx, &next);
    profiles.len()
}

fn new_profile() -> Profile {
    Profile {
        name: "New profile".into(),
        match_process: Vec::new(),
        match_title: Vec::new(),
        enabled: true,
        audio: None,
        model: None,
        cleanup: None,
        insert: None,
        overlay: None,
        history: None,
    }
}

impl Profiles {
    fn open_editor(&mut self, index: usize) {
        self.editing = Some(index);
        self.session += 1;
    }
}

impl Page for Profiles {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let profiles = cx.config.config().profiles.clone();
        let summaries: Vec<(String, Option<String>)> = profiles
            .iter()
            .map(|p| (match_summary(p), p.enabled.then(|| override_summary(p, cx.catalog.snapshot()))))
            .collect();
        let starters_left: Vec<Starter> = starters()
            .into_iter()
            .filter(|s| !profiles.iter().any(|p| p.name.to_lowercase() == s.profile.name.to_lowercase()))
            .collect();

        page_header(
            ui,
            "Profiles",
            Some("Change how Flow behaves in particular apps, matched by program name or window title."),
        );

        let mut add_new = false;
        let mut card_action: Option<(usize, CardAction)> = None;
        Group::new("Your profiles")
            .action(|ui| {
                add_new = Button::new("Add profile").icon(Icon::Plus, 15.0).show(ui).clicked();
            })
            .show(ui, |ui| {
                if profiles.is_empty() {
                    empty_state(ui, "No profiles yet. Add one, or start from a suggestion below.", |_| {});
                }
                for (index, (profile, (matches, overrides))) in profiles.iter().zip(&summaries).enumerate() {
                    if let Some(action) = profile_card(ui, index, profile, matches, overrides.as_deref()) {
                        card_action = Some((index, action));
                    }
                }
            });

        let mut add_starter: Option<Profile> = None;
        if !starters_left.is_empty() {
            Group::new("Suggestions").description("Common setups. Add one, then adjust the app list.").show(ui, |ui| {
                for starter in &starters_left {
                    if starter_card(ui, starter) {
                        add_starter = Some(starter.profile.clone());
                    }
                }
            });
        }

        if add_new {
            let index = add(cx, &profiles, new_profile());
            self.open_editor(index);
        }
        if let Some(profile) = add_starter {
            add(cx, &profiles, profile);
        }
        match card_action {
            Some((index, CardAction::Edit)) => self.open_editor(index),
            Some((index, CardAction::Delete)) => self.removing = Some(index),
            None => {}
        }

        // The dialogs read the list again: an add above has already landed in the optimistic config.
        let profiles = cx.config.config().profiles.clone();
        self.editor(ui, cx, &profiles);
        self.confirm_delete(ui, cx, &profiles);
    }
}

impl Profiles {
    fn editor(&mut self, ui: &Ui, cx: &mut Cx<'_>, profiles: &[Profile]) {
        let Some(index) = self.editing else { return };
        let Some(profile) = profiles.get(index) else {
            self.editing = None;
            return;
        };
        let snapshot = cx.catalog.snapshot();
        let mut models: Vec<SelectOption<String>> = snapshot
            .map(|s| {
                ready_models(s)
                    .iter()
                    .map(|m| SelectOption::new(m.id().to_string(), model_label(Some(s), m.id())))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(Value::String(current)) = get_override(profile, "model", "active")
            && !models.iter().any(|o| &o.value == current)
        {
            models.push(SelectOption::new(current.clone(), model_label(snapshot, current)).hint("Not ready"));
        }
        let prompts: Vec<SelectOption<String>> = prompt_names(cx.config.config())
            .into_iter()
            .map(|p| SelectOption::new(p.clone(), prompt_label(&p)))
            .collect();
        let name_taken = |name: &str| {
            profiles.iter().enumerate().any(|(i, p)| i != index && p.name.to_lowercase() == name.to_lowercase())
        };
        let options = editor::Options { models: &models, prompts: &prompts, name_taken: &name_taken };

        let mut open = true;
        let title = format!("Edit {}", profile.name);
        let session = self.session;
        let changed = Modal::new("profiles.editor", &title)
            .description("Changes are saved as you make them.")
            .width(640.0)
            .show(ui.ctx(), &mut open, |ui| editor::show(ui, profile, session, &options))
            .flatten();
        if let Some(next) = changed {
            let mut list = profiles.to_vec();
            list[index] = next;
            write(cx, &list);
        }
        if !open {
            self.editing = None;
        }
    }

    fn confirm_delete(&mut self, ui: &Ui, cx: &mut Cx<'_>, profiles: &[Profile]) {
        let Some(index) = self.removing else { return };
        let Some(doomed) = profiles.get(index) else {
            self.removing = None;
            return;
        };
        let mut open = true;
        let title = format!("Delete {}?", doomed.name);
        let confirmed = ConfirmDialog::new(
            "profiles.delete",
            &title,
            "The profile is removed from the config file. The apps it matched go back to your main settings.",
            "Delete profile",
        )
        .show(ui.ctx(), &mut open);
        if confirmed {
            let next: Vec<Profile> =
                profiles.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, p)| p.clone()).collect();
            write(cx, &next);
        }
        if !open {
            self.removing = None;
        }
    }
}

/// A row of one-line texts on the left (`min-w-0 flex-1`), actions on the
/// right, centred (`card flex items-center gap-4 px-4 py-3`). `text` draws
/// the left column into the given width and returns its height.
fn row_card<R>(ui: &mut Ui, id: Id, text: impl FnOnce(&mut Ui, f32) -> f32, actions: impl FnOnce(&mut Ui) -> R) -> R {
    card(ui, Margin::symmetric(16, 12), |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let ctx = ui.ctx().clone();
        let width = ui.available_width();
        let top = ui.cursor().min;
        let actions_size = measure::remembered(&ctx, id).unwrap_or(vec2(0.0, 32.0));
        let text_w = (width - 16.0 - actions_size.x).max(40.0);
        let mut left = ui.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_size(top, vec2(text_w, f32::INFINITY)))
                .layout(Layout::top_down(Align::Min)),
        );
        let text_h = text(&mut left, text_w);
        let row_h = text_h.max(actions_size.y);
        let slot = Rect::from_min_size(
            pos2(top.x + width - actions_size.x, top.y + ((row_h - actions_size.y) / 2.0).round()),
            actions_size,
        );
        let mut right = ui.new_child(UiBuilder::new().max_rect(slot).layout(Layout::left_to_right(Align::Center)));
        right.spacing_mut().item_spacing = vec2(4.0, 0.0);
        let result = actions(&mut right);
        measure::record(&ctx, id, right.min_rect().size());
        ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, row_h)));
        result
    })
    .0
}

/// `mt-0.5 truncate text-caption text-fg-2`. Returns the height used.
fn caption_line(ui: &mut Ui, text: &str, width: f32) -> f32 {
    let t = tokens(ui.ctx());
    ui.add_space(2.0);
    let galley = text::galley_truncated(ui, text, TextStyle::CAPTION, t.fg_2, width);
    text::paint_galley(ui, galley);
    18.0
}

fn profile_card(
    ui: &mut Ui,
    index: usize,
    profile: &Profile,
    matches: &str,
    overrides: Option<&str>,
) -> Option<CardAction> {
    let id = Id::unique(("flow-profile-card-actions", index));
    row_card(
        ui,
        id,
        |ui, width| {
            let t = tokens(ui.ctx());
            // `flex flex-wrap items-center gap-2`: the name, then "Dictation off" if it fits.
            let top = ui.cursor().min;
            let name = text::galley(ui, profile.name.as_str(), TextStyle::BODY.weight(Weight::Semibold), t.fg, width);
            let name_size = name.size();
            ui.painter().galley(top, name, t.fg);
            let mut height = name_size.y;
            if !profile.enabled {
                let label = "Dictation off";
                let badge_w = text::width(ui, label, TextStyle::CAPTION.weight(Weight::Semibold)) + 16.0;
                let last_line_w = name_size.x.min(width);
                let pos = if name_size.y <= 20.0 && last_line_w + 8.0 + badge_w <= width {
                    pos2(top.x + last_line_w + 8.0, top.y)
                } else {
                    height += 8.0 + 20.0;
                    pos2(top.x, top.y + name_size.y + 8.0)
                };
                badge(
                    &mut ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos, vec2(badge_w, 20.0)))),
                    label,
                    BadgeTone::Warn,
                );
            }
            ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, height)));
            height += caption_line(ui, matches, width);
            if let Some(overrides) = overrides {
                height += caption_line(ui, overrides, width);
            }
            height
        },
        |ui| {
            let edit = Button::new("Edit").icon(Icon::Pencil, 14.0).show(ui).clicked();
            let delete = IconButton::new(format!("Delete {}", profile.name), Icon::Trash2, 15.0).show(ui).clicked();
            if edit {
                Some(CardAction::Edit)
            } else if delete {
                Some(CardAction::Delete)
            } else {
                None
            }
        },
    )
}

/// A suggestion: its name, what it does and matches, and Add.
fn starter_card(ui: &mut Ui, starter: &Starter) -> bool {
    let id = Id::unique(("flow-starter-card", starter.id));
    row_card(
        ui,
        id,
        |ui, width| {
            let t = tokens(ui.ctx());
            let name = text::galley(ui, starter.profile.name.as_str(), TextStyle::BODY, t.fg, width);
            let mut height = name.size().y;
            text::paint_galley(ui, name);
            let line = format!("{}. Matches {}.", starter.description, starter.profile.match_process.join(", "));
            height += caption_line(ui, &line, width);
            height
        },
        |ui| Button::new("Add").show(ui).clicked(),
    )
}

#[cfg(test)]
mod tests;
