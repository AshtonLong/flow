//! Step 3: choose a model (`StepModel`). On this PC (the default local model,
//! downloaded here with progress) or in the cloud (the recommended cloud model,
//! with a key form). The choice becomes the active model as soon as it can be
//! used, and then the dictation test appears under it.

use egui::{Align, Key, Layout, Margin, Rect, Response, Sense, Ui, UiBuilder, pos2, vec2};
use flow_core::catalog::{CatalogSnapshot, CloudModelEntry, DownloadState, LocalModelEntry};
use flow_core::config::DEFAULT_LOCAL_MODEL;
use flow_core::protocol::settings::{KeyTestResult, Request};

use super::step_heading;
use super::try_it::TestDictation;
use crate::backend::Pending;
use crate::components::{BadgeTone, Button, Notice, Tone, badge, card, progress_bar, radio};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, tokens};
use crate::pages::Cx;
use crate::pages::models::cloud_providers::KeyForm;
use crate::util::format::{error_message, format_bytes, format_price, format_speed};
use crate::util::models::{find_provider, provider_connected};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    #[default]
    Local,
    Cloud,
}

/// One `models.activate` call. Its error is dropped once the effect that made
/// it was cleaned up (the choice, its readiness or the active model changed).
struct Activation {
    pending: Pending<()>,
    cancelled: bool,
}

/// The local choice's own state (`LocalChoice`); kept while the other choice is picked.
#[derive(Default)]
struct LocalChoice {
    error: Option<String>,
    download: Option<Pending<()>>,
}

/// The cloud choice's own state (`CloudChoice`).
#[derive(Default)]
struct CloudChoice {
    result: Option<KeyTestResult>,
    form: KeyForm,
}

#[derive(Default)]
pub struct StepModel {
    choice: Choice,
    error: Option<String>,
    activations: Vec<Activation>,
    /// The activation effect's dependencies when it last ran: chosen id, ready, active model.
    effect: Option<(Option<String>, bool, String)>,
    local: LocalChoice,
    cloud: CloudChoice,
    test: Option<TestDictation>,
    /// Preview only: the dictation test shows a made-up dictation when it appears.
    simulate: Option<String>,
}

/// The default local model, else the first one.
fn local_entry(snapshot: &CatalogSnapshot) -> Option<&LocalModelEntry> {
    let local = &snapshot.catalog.local;
    local.iter().find(|m| m.id == DEFAULT_LOCAL_MODEL).or_else(|| local.first())
}

/// The recommended cloud model, else the first one.
fn cloud_entry(snapshot: &CatalogSnapshot) -> Option<&CloudModelEntry> {
    let cloud = &snapshot.catalog.cloud;
    cloud.iter().find(|m| m.recommended).or_else(|| cloud.first())
}

impl StepModel {
    pub fn choose(&mut self, choice: Choice) {
        self.choice = choice;
    }

    /// Preview only (`--demo try`): the dictation test shows `text` as if it had been dictated.
    pub fn simulate_dictation(&mut self, text: &str) {
        self.simulate = Some(text.to_string());
    }

    #[cfg(test)]
    pub fn choice(&self) -> Choice {
        self.choice
    }

    #[cfg(test)]
    pub fn test(&self) -> Option<&TestDictation> {
        self.test.as_ref()
    }

    /// The chosen model's id, and whether it can be used now.
    fn chosen(&self, snapshot: Option<&CatalogSnapshot>) -> (Option<String>, bool) {
        let Some(snapshot) = snapshot else {
            return (None, false);
        };
        match self.choice {
            Choice::Local => match local_entry(snapshot) {
                Some(m) => (Some(m.id.clone()), snapshot.status.get(&m.id).is_some_and(|s| s.ready)),
                None => (None, false),
            },
            Choice::Cloud => match cloud_entry(snapshot) {
                Some(m) => (Some(m.id.clone()), provider_connected(snapshot, &m.provider)),
                None => (None, false),
            },
        }
    }

    /// The choice becomes the active model as soon as it can be used (a React
    /// effect on the chosen id, its readiness and the active model).
    fn activate(&mut self, cx: &mut Cx<'_>, chosen: &Option<String>, ready: bool) {
        let active = cx.config.config().model.active.clone();
        let deps = (chosen.clone(), ready, active.clone());
        if self.effect.as_ref() != Some(&deps) {
            for activation in &mut self.activations {
                activation.cancelled = true;
            }
            if let Some(id) = chosen
                && ready
                && *id != active
            {
                let pending = cx.link.call(Request::ModelsActivate(id.clone()));
                self.activations.push(Activation { pending, cancelled: false });
            }
            self.effect = Some(deps);
        }
        let error = &mut self.error;
        let catalog = &mut *cx.catalog;
        self.activations.retain_mut(|activation| match activation.pending.take() {
            None => true,
            Some(Ok(())) => {
                catalog.reload();
                false
            }
            Some(Err(err)) => {
                if !activation.cancelled {
                    *error = Some(error_message(&err));
                }
                false
            }
        });
    }

    /// `LocalChoice.start`: clears the old error and progress, then downloads.
    fn start_download(&mut self, cx: &mut Cx<'_>, id: &str) {
        self.local.error = None;
        cx.catalog.clear_progress(id);
        self.local.download = Some(cx.link.call(Request::ModelsDownload(id.to_string())));
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        if let Some(result) = self.local.download.as_mut().and_then(Pending::take) {
            self.local.download = None;
            match result {
                Ok(()) => cx.catalog.reload(),
                Err(err) => self.local.error = Some(error_message(&err)),
            }
        }
        let (chosen, ready) = self.chosen(cx.catalog.snapshot());
        self.activate(cx, &chosen, ready);

        let Some(snapshot) = cx.catalog.snapshot().cloned() else {
            match cx.catalog.error() {
                Some(error) => {
                    Notice::new(Tone::Danger, &format!("The model catalog could not be loaded: {error}")).show(ui);
                }
                None => {
                    text::paragraph(ui, "Loading the catalog…", TextStyle::BODY, t.fg_2);
                }
            }
            self.test = None;
            return;
        };

        step_heading(
            ui,
            "Choose a model",
            "The model turns your voice into text. You can switch at any time on the Models page.",
        );
        ui.add_space(20.0);

        // The radio group: cards 3 px apart (`flex flex-col gap-[3px]`).
        let mut radios: Vec<(Choice, Response)> = Vec::new();
        if let Some(local) = local_entry(&snapshot).cloned() {
            let selected = self.choice == Choice::Local;
            let description = format!(
                "Free and offline. Your audio never leaves this PC. A one-time {} download.",
                format_bytes(local.size_bytes as f64)
            );
            let label = |ui: &mut Ui| {
                text::label(ui, format!("On this PC: {}", local.name), TextStyle::BODY.weight(Weight::Semibold), t.fg);
                badge(ui, "Recommended", BadgeTone::Accent);
            };
            let (response, picked) = choice_card(ui, "local", selected, label, &description, |ui| {
                if selected {
                    self.local_choice(ui, cx, &snapshot, &local);
                }
            });
            if picked {
                self.choice = Choice::Local;
            }
            radios.push((Choice::Local, response));
        }
        if let Some(cloud) = cloud_entry(&snapshot).cloned() {
            if !radios.is_empty() {
                ui.add_space(3.0);
            }
            let selected = self.choice == Choice::Cloud;
            let provider = find_provider(Some(&snapshot), &cloud.provider).cloned();
            let provider_name = provider.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| cloud.provider.clone());
            let description = format!(
                "Nothing to download, and fast on any PC. Needs your own {provider_name} key; about {} per hour of speech, billed by them. Audio is sent to the provider.",
                format_price(cloud.price_per_hour)
            );
            let label = |ui: &mut Ui| {
                let style = TextStyle::BODY.weight(Weight::Semibold);
                text::paragraph(ui, format!("In the cloud: {provider_name} {}", cloud.name), style, t.fg);
            };
            let (response, picked) = choice_card(ui, "cloud", selected, label, &description, |ui| {
                if selected && let Some(provider) = &provider {
                    self.cloud_choice(ui, cx, &snapshot, provider);
                }
            });
            if picked {
                self.choice = Choice::Cloud;
            }
            radios.push((Choice::Cloud, response));
        }
        self.radio_keys(ui, &radios);

        if let Some(error) = &self.error {
            ui.add_space(12.0);
            Notice::new(Tone::Danger, error).show(ui);
        }

        // `{chosen && chosenReady && <TestDictation key={chosen.id} />}`
        let (chosen, ready) = self.chosen(Some(&snapshot));
        match chosen.filter(|_| ready) {
            Some(id) => {
                if self.test.as_ref().is_none_or(|test| test.model_id() != id) {
                    let mut test = TestDictation::new(&id);
                    if let Some(text) = self.simulate.take() {
                        test.simulate(&text);
                    }
                    self.test = Some(test);
                }
                if let Some(test) = &mut self.test {
                    ui.add_space(20.0);
                    test.ui(ui, cx);
                }
            }
            None => self.test = None,
        }
    }

    /// Radix's radio group: the arrow keys move the choice between the cards.
    fn radio_keys(&mut self, ui: &Ui, radios: &[(Choice, Response)]) {
        let Some(focused) = radios.iter().position(|(_, r)| r.has_focus()) else {
            return;
        };
        let (back, forward) = ui.input_mut(|i| {
            let none = egui::Modifiers::NONE;
            (
                i.consume_key(none, Key::ArrowUp) || i.consume_key(none, Key::ArrowLeft),
                i.consume_key(none, Key::ArrowDown) || i.consume_key(none, Key::ArrowRight),
            )
        });
        let next = if back {
            (focused + radios.len() - 1) % radios.len()
        } else if forward {
            (focused + 1) % radios.len()
        } else {
            return;
        };
        self.choice = radios[next].0;
        radios[next].1.request_focus();
    }

    /// Under the local choice: Installed, the download button, or its progress.
    fn local_choice(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, snapshot: &CatalogSnapshot, entry: &LocalModelEntry) {
        let t = tokens(ui.ctx());
        let ready = snapshot.status.get(&entry.id).is_some_and(|s| s.ready);
        let download = cx.catalog.download_of(&entry.id).cloned();
        let busy =
            download.as_ref().is_some_and(|d| matches!(d.state, DownloadState::Downloading | DownloadState::Verifying));
        let failed = download.as_ref().is_some_and(|d| d.state == DownloadState::Error);

        // `mt-3`
        ui.add_space(12.0);
        if ready {
            badge(ui, "Installed", BadgeTone::Ok);
        }
        if !ready && !busy {
            let label = if failed {
                "Retry download".to_string()
            } else {
                format!("Download {}", format_bytes(entry.size_bytes as f64))
            };
            if Button::new(label).icon(Icon::Download, 15.0).show(ui).clicked() {
                self.start_download(cx, &entry.id);
            }
        }
        if busy && let Some(download) = &download {
            let verifying = download.state == DownloadState::Verifying;
            let value = (!verifying && download.total_bytes > 0)
                .then(|| download.received_bytes as f32 / download.total_bytes as f32);
            progress_bar(ui, value);
            ui.add_space(6.0);
            let line = if verifying {
                "Verifying the download…".to_string()
            } else {
                let percent = if download.total_bytes > 0 {
                    (download.received_bytes as f64 / download.total_bytes as f64 * 100.0).floor()
                } else {
                    0.0
                };
                let total = if download.total_bytes > 0 { download.total_bytes } else { entry.size_bytes };
                let speed =
                    if download.speed > 0.0 { format!(", {}", format_speed(download.speed)) } else { String::new() };
                format!("{percent}% of {}{speed}", format_bytes(total as f64))
            };
            // `mt-1.5 flex items-center justify-between text-caption text-fg-2 tabular-nums`
            let width = ui.available_width();
            ui.allocate_ui_with_layout(vec2(width, 28.0), Layout::left_to_right(Align::Center), |ui| {
                text::label(ui, line, TextStyle::CAPTION, t.fg_2);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::new("Cancel").small().subtle().show(ui).clicked() {
                        cx.link.send(Request::ModelsCancel(entry.id.clone()));
                    }
                });
            });
        }
        if self.local.error.is_some() || failed {
            let detail = self.local.error.clone().or_else(|| download.as_ref().and_then(|d| d.error.clone()));
            let message = match detail {
                Some(detail) if !detail.is_empty() => format!("The download failed: {detail}"),
                _ => "The download failed.".to_string(),
            };
            ui.add_space(8.0);
            Notice::new(Tone::Danger, &message).show(ui);
        }
    }

    /// Under the cloud choice: "<Provider> connected", or the key form.
    fn cloud_choice(
        &mut self,
        ui: &mut Ui,
        cx: &mut Cx<'_>,
        snapshot: &CatalogSnapshot,
        provider: &flow_core::catalog::ProviderEntry,
    ) {
        // `mt-3`
        ui.add_space(12.0);
        if provider_connected(snapshot, &provider.id) {
            badge(ui, &format!("{} connected", provider.name), BadgeTone::Ok);
        } else if let Some(result) = self.cloud.form.show(ui, cx, provider, false).done {
            self.cloud.result = Some(result);
            cx.catalog.reload();
        }
        if let Some(result) = self.cloud.result.as_ref().filter(|r| !r.ok) {
            ui.add_space(8.0);
            Notice::new(Tone::Danger, &format!("The key was saved, but the test failed: {}", result.message)).show(ui);
        }
    }
}

/// One choice: `card px-4 py-3`, the radio (`mt-0.5`) beside the label, its
/// description (`mt-0.5 text-caption text-fg-2`) and the choice's own content.
/// The label picks the choice too, like `<label htmlFor>`. Returns the radio,
/// and whether the radio or its label was clicked.
fn choice_card(
    ui: &mut Ui,
    id_salt: &str,
    selected: bool,
    label: impl FnOnce(&mut Ui),
    description: &str,
    body: impl FnOnce(&mut Ui),
) -> (Response, bool) {
    let t = tokens(ui.ctx());
    card(ui, Margin::symmetric(16, 12), |ui| {
        let top = ui.cursor().min;
        let width = ui.available_width();
        let mut radio_ui = ui.new_child(
            UiBuilder::new()
                .id_salt(("flow-onboarding-radio", id_salt))
                .max_rect(Rect::from_min_size(pos2(top.x, top.y + 2.0), vec2(20.0, 20.0))),
        );
        let response = radio(&mut radio_ui, selected, false);
        // `flex items-start gap-3`, then `min-w-0 flex-1`.
        let content = Rect::from_min_size(pos2(top.x + 32.0, top.y), vec2(width - 32.0, f32::INFINITY));
        let mut col = ui.new_child(UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)));
        // `label.flex.flex-wrap.items-center.gap-2`
        let row = col
            .allocate_ui_with_layout(vec2(width - 32.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                label(ui);
            })
            .response;
        let label_clicked = col.interact(row.rect, response.id.with("label"), Sense::click()).clicked();
        col.add_space(2.0);
        text::paragraph(&mut col, description, TextStyle::CAPTION, t.fg_2);
        body(&mut col);
        let bottom = col.min_rect().bottom().max(top.y + 22.0);
        ui.advance_cursor_after_rect(Rect::from_min_max(top, pos2(top.x + width, bottom)));
        let picked = response.clicked() || label_clicked;
        (response, picked)
    })
    .0
}
