//! AI cleanup: the optional language-model pass, its model, prompt and timeout.

use egui::{Align, Id, Layout, Rect, Ui, UiBuilder, pos2, vec2};
use flow_core::catalog::split_model_id;
use flow_core::config::prompt_names;
use flow_core::path;

use crate::components::{
    Button, Group, Notice, NumberField, Select, SelectOption, SettingRow, TextField, Toggle, Tone, measure,
};
use crate::design::text::{self, TextStyle};
use crate::design::tokens;
use crate::pages::{Cx, PageId};
use crate::util::format::{format_price, prompt_label, sentence};
use crate::util::models::{find_provider, provider_connected};

/// The select's entry for a model that is not in the catalog.
const OTHER: &str = "__other__";

#[derive(Default)]
pub(super) struct AiCleanup {
    /// "Local or custom endpoint" was picked, before a model was written for it.
    other_picked: bool,
}

/// A local or custom model id must read `provider/model`.
fn validate_other(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() || split_model_id(v).is_some() { None } else { Some("Write it as provider/model.".into()) }
}

impl AiCleanup {
    pub(super) fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let config = cx.config.config();
        let cleanup = config.cleanup.clone();
        let prompts: Vec<SelectOption<String>> =
            prompt_names(config).into_iter().map(|name| SelectOption::new(name.clone(), prompt_label(&name))).collect();
        let snapshot = cx.catalog.snapshot();
        let entries = snapshot.map(|s| s.catalog.cleanup.as_slice()).unwrap_or_default();
        let in_catalog = entries.iter().any(|e| e.id == cleanup.llm_model);
        let other = self.other_picked || (snapshot.is_some() && !in_catalog);

        let mut models: Vec<SelectOption<String>> = entries
            .iter()
            .map(|e| {
                let provider = find_provider(snapshot, &e.provider).map(|p| p.name.as_str()).unwrap_or(&e.provider);
                let hint = format!(
                    "{} in / {} out{}",
                    format_price(e.price_in),
                    format_price(e.price_out),
                    if e.recommended { ", recommended" } else { "" }
                );
                SelectOption::new(e.id.clone(), format!("{provider}: {}", e.name)).hint(hint)
            })
            .collect();
        models.push(SelectOption::new(OTHER.to_string(), "Local or custom endpoint").hint("Free"));

        let model_description = match entries.iter().find(|e| e.id == cleanup.llm_model) {
            Some(selected) => format!(
                "{} {} in and {} out per million tokens; a 100-word dictation is about 600 tokens.",
                sentence(&selected.description),
                format_price(selected.price_in),
                format_price(selected.price_out)
            ),
            None => "A model on your own machine or network, written as provider/model.".into(),
        };
        let provider = split_model_id(&cleanup.llm_model).and_then(|(id, _)| find_provider(snapshot, id));
        let missing_key = match (snapshot, provider) {
            (Some(snapshot), Some(provider)) if !provider_connected(snapshot, &provider.id) => Some(format!(
                "{} is not connected. Add its key on the Models page, or cleanup is skipped.",
                provider.name
            )),
            _ => None,
        };
        let model_value = if other { OTHER.to_string() } else { cleanup.llm_model.clone() };
        let other_value = if in_catalog { String::new() } else { cleanup.llm_model.clone() };

        let mut picked: Option<String> = None;
        let mut typed: Option<String> = None;
        let mut open_models = false;

        Group::new("AI cleanup")
            .description(
                "An optional second pass: a small language model rewrites the transcript using a prompt you \
                 control. If it is slow or fails, the text from the local rules is typed instead.",
            )
            .show(ui, |ui| {
                SettingRow::new(
                    "Use AI cleanup",
                    "Off by default. When on, the transcript text (never the audio) is sent to the model below.",
                )
                .show(ui, |ui| {
                    if let Some(v) = Toggle::new(cleanup.llm_enabled).show(ui).changed {
                        cx.config.set(path!["cleanup", "llm_enabled"], v);
                    }
                });

                SettingRow::new("Cleanup model", model_description).show_with(
                    ui,
                    |ui| {
                        picked =
                            Select::new("cleanup.llm_model", Some(&model_value), &models).width(260.0).show(ui).changed;
                    },
                    |ui| {
                        if other {
                            ui.add_space(12.0);
                            typed = other_model_row(ui, &other_value);
                        }
                        if let Some(text) = &missing_key {
                            ui.add_space(12.0);
                            open_models = Notice::new(Tone::Warn, text)
                                .show_with_action(ui, |ui| Button::new("Open Models").small().show(ui).clicked());
                        }
                    },
                );

                SettingRow::new("Prompt", "The style the model rewrites into. Edit the prompts below.").show(
                    ui,
                    |ui| {
                        if let Some(v) =
                            Select::new("cleanup.llm_prompt", Some(&cleanup.llm_prompt), &prompts).show(ui).changed
                        {
                            cx.config.set(path!["cleanup", "llm_prompt"], v);
                        }
                    },
                );

                SettingRow::new(
                    "Timeout",
                    "If the model has not answered by then, Flow types the uncleaned text so nothing is lost.",
                )
                .show(ui, |ui| {
                    let field =
                        NumberField::new("cleanup.llm_timeout_ms", cleanup.llm_timeout_ms, 500..=30000).unit("ms");
                    if let Some(v) = field.show(ui).committed {
                        cx.config.set(path!["cleanup", "llm_timeout_ms"], v);
                    }
                });
            });

        if let Some(v) = picked {
            if v == OTHER {
                self.other_picked = true;
            } else {
                self.other_picked = false;
                cx.config.set(path!["cleanup", "llm_model"], v);
            }
        }
        if let Some(v) = typed {
            let v = v.trim();
            if !v.is_empty() {
                cx.config.set(path!["cleanup", "llm_model"], v);
            }
        }
        if open_models {
            cx.go_to(PageId::Models);
        }
    }
}

/// `mt-3 flex items-center justify-between gap-6`: what to write on the left,
/// the `provider/model` box on the right. Returns the committed text.
fn other_model_row(ui: &mut Ui, value: &str) -> Option<String> {
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let id = Id::unique("flow-cleanup-other-model");
    let width = ui.available_width();
    let top = ui.cursor().min;
    // The box grows when it shows an error under it; centre on last pass's height.
    let field_h = measure::remembered(&ctx, id).map(|s| s.y).unwrap_or(32.0);
    let galley = text::galley(
        ui,
        "The endpoint and model, for example ollama/llama3.2 or lmstudio/qwen2.5-7b.",
        TextStyle::CAPTION,
        t.fg_2,
        (width - 24.0 - 260.0).max(40.0),
    );
    let row_h = galley.size().y.max(field_h);
    ui.painter().galley(pos2(top.x, top.y + ((row_h - galley.size().y) / 2.0).round()), galley, t.fg_2);
    let area = Rect::from_min_size(
        pos2(top.x + width - 260.0, top.y + ((row_h - field_h) / 2.0).round()),
        vec2(260.0, f32::INFINITY),
    );
    let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
    let committed = TextField::new("cleanup.llm_model.other", value)
        .width(260.0)
        .mono()
        .placeholder("ollama/llama3.2")
        .validate(&validate_other)
        .show(&mut child)
        .committed;
    measure::record(&ctx, id, child.min_rect().size());
    ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(width, row_h)));
    committed
}
