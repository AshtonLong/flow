//! Cleanup: how the raw transcript is tidied before it is typed. The local
//! rules, the dictionary and snippets, AI cleanup and its prompts. Port of
//! `pages/Cleanup.tsx`; the sections are submodules, as they are separate
//! components in the TSX.

mod ai;
mod lists;
mod prompts;

use egui::Ui;
use flow_core::path;

use super::{Cx, Page};
use crate::components::{Group, SettingRow, Toggle, page_header};

#[derive(Default)]
pub struct Cleanup {
    dictionary: lists::Dictionary,
    snippets: lists::Snippets,
    ai: ai::AiCleanup,
    prompts: prompts::Prompts,
}

impl Page for Cleanup {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let cleanup = cx.config.config().cleanup.clone();
        page_header(
            ui,
            "Cleanup",
            Some(
                "How the raw transcript is tidied before it is typed. The local rules are instant and need no network.",
            ),
        );

        Group::new("Local rules").show(ui, |ui| {
            let rules = [
                (
                    "Remove filler words",
                    "Strips “um”, “uh” and accidentally repeated words.",
                    "filler_removal",
                    cleanup.filler_removal,
                ),
                (
                    "Spoken formatting",
                    "Saying “new line” or “new paragraph” inserts the break instead of the words.",
                    "spoken_formatting",
                    cleanup.spoken_formatting,
                ),
                (
                    "Spoken punctuation",
                    "Saying “comma”, “full stop” or “question mark” types the symbol.",
                    "spoken_punctuation",
                    cleanup.spoken_punctuation,
                ),
            ];
            for (label, description, key, on) in rules {
                SettingRow::new(label, description).show(ui, |ui| {
                    if let Some(v) = Toggle::new(on).show(ui).changed {
                        cx.config.set(path!["cleanup", key], v);
                    }
                });
            }
        });

        self.dictionary.ui(ui, cx);
        self.snippets.ui(ui, cx);
        self.ai.ui(ui, cx);
        self.prompts.ui(ui, cx);
    }
}

#[cfg(test)]
mod tests;
