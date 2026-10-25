//! The Cloud section of the Models page: one card per provider with its key
//! (connect, test, replace, remove) and its models with their list prices.
//! Port of `pages/models/CloudProviders.tsx`.

use std::collections::HashMap;

use egui::{Align, Id, Key, Layout, Margin, Sense, Ui, Vec2, pos2, vec2};
use flow_core::catalog::{CloudModelEntry, ProviderEntry};
use flow_core::protocol::settings::{KeyTestResult, Request};

use super::{Act, Cross, Inline, flex_row, flow, padded, rule, spinner_icon};
use crate::backend::Pending;
use crate::components::{BadgeTone, Button, ConfirmDialog, Input, Notice, Tone, badge, card};
use crate::design::text::{self, Rich, TextStyle};
use crate::design::{Weight, tokens};
use crate::pages::Cx;
use crate::util::format::{error_message, format_date, format_price, sentence};
use crate::util::models::{cloud_by_provider, provider_connected};

/// A key being stored, then tested.
enum Submit {
    Saving(Pending<()>),
    Testing(Pending<KeyTestResult>),
}

/// What a [`KeyForm`] reported this frame.
#[derive(Debug, Default)]
pub struct KeyFormResponse {
    /// The key was stored (or storing failed); the test result.
    pub done: Option<KeyTestResult>,
    pub cancelled: bool,
}

/// Key entry for one provider: store it, test it, report the result
/// (`KeyForm`). Keep it in the page struct while it is shown.
#[derive(Default)]
pub struct KeyForm {
    key: String,
    busy: Option<Submit>,
}

impl KeyForm {
    /// True while the key is being stored or tested.
    pub fn busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Advances a submission in flight; returns the result once it is in.
    /// Called by [`KeyForm::show`]; call it yourself on frames the form is hidden.
    pub fn pump(&mut self, cx: &mut Cx<'_>, provider: &str) -> Option<KeyTestResult> {
        let failed = |err: String| KeyTestResult { ok: false, message: error_message(&err) };
        match self.busy.as_mut()? {
            Submit::Saving(pending) => match pending.take()? {
                Ok(()) => {
                    self.busy = Some(Submit::Testing(cx.link.call(Request::SecretsTest(provider.to_string()))));
                    // An instant backend answers at once.
                    self.pump(cx, provider)
                }
                Err(err) => {
                    self.busy = None;
                    Some(failed(err))
                }
            },
            Submit::Testing(pending) => {
                let result = pending.take()?;
                self.busy = None;
                Some(match result {
                    Ok(result) => {
                        self.key.clear();
                        result
                    }
                    Err(err) => failed(err),
                })
            }
        }
    }

    fn submit(&mut self, cx: &mut Cx<'_>, provider: &str) {
        let trimmed = self.key.trim();
        if trimmed.is_empty() || self.busy.is_some() {
            return;
        }
        let request = Request::SecretsSet { provider: provider.to_string(), key: trimmed.to_string() };
        self.busy = Some(Submit::Saving(cx.link.call(request)));
    }

    /// The form: label, the key box with "Save and test" (and "Cancel" when
    /// `cancellable`), and where to get a key.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        cx: &mut Cx<'_>,
        provider: &ProviderEntry,
        cancellable: bool,
    ) -> KeyFormResponse {
        let t = tokens(ui.ctx());
        let mut response = KeyFormResponse::default();
        let label =
            format!("{} API key. It is encrypted on this PC and never written to the config file.", provider.name);
        text::paragraph(ui, label, TextStyle::CAPTION, t.fg_2);
        ui.add_space(8.0);
        let busy = self.busy.is_some();
        let can_submit = !self.key.trim().is_empty() && !busy;
        let id = Id::unique(("flow-key-form", &provider.id));
        let key = &mut self.key;
        let (entered, (clicked, cancelled)) = flex_row(
            ui,
            id,
            8.0,
            Cross::Center(0.0),
            8.0,
            |ui| {
                let width = ui.available_width().max(220.0);
                let field = Input::new(("key", &provider.id), key).width(width).mono().password();
                let field = field.placeholder("Paste the key").show(ui);
                field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
            },
            |ui| {
                let mut save =
                    Button::new(if busy { "Testing…" } else { "Save and test" }).accent().disabled(!can_submit);
                if busy {
                    save = save.icon_spec(spinner_icon(ui));
                }
                let clicked = save.show(ui).clicked();
                let cancelled = cancellable && Button::new("Cancel").subtle().show(ui).clicked();
                (clicked, cancelled)
            },
        );
        if entered || clicked {
            self.submit(cx, &provider.id);
        }
        response.cancelled = cancelled;
        if let Some(url) = provider.key_url.as_deref().filter(|u| u.starts_with("https://")) {
            ui.add_space(8.0);
            let rich = Rich::new(TextStyle::CAPTION, t.fg_2)
                .text("No key yet? ")
                .link(format!("Get a key from {}", provider.name));
            if rich.show(ui).clicked.is_some() {
                cx.link.send(Request::AppOpenExternal(url.to_string()));
            }
        }
        response.done = self.pump(cx, &provider.id);
        response
    }
}

/// One provider's card state (`ProviderCard`'s `useState`s).
#[derive(Default)]
struct CardState {
    editing: bool,
    testing: Option<Pending<KeyTestResult>>,
    result: Option<KeyTestResult>,
    confirm: bool,
    form: KeyForm,
}

/// The Cloud section. Card state is kept per provider id.
#[derive(Default)]
pub struct CloudProviders {
    cards: HashMap<String, CardState>,
}

impl CloudProviders {
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, act: &mut Act) {
        let t = tokens(ui.ctx());
        let Some(snapshot) = cx.catalog.snapshot().cloned() else {
            return;
        };
        // `<section className="mb-8">`, its header `mb-2`.
        text::paragraph(ui, "Cloud", TextStyle::BODY.weight(Weight::Semibold).heading(), t.fg);
        ui.add_space(2.0);
        let max = 78.0 * text::ch(ui, TextStyle::CAPTION);
        text::paragraph_max(
            ui,
            "Uses your own API key, billed by the provider at its list price. Audio goes straight from this PC to the provider.",
            TextStyle::CAPTION,
            t.fg_2,
            max,
        );
        ui.add_space(8.0);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 3.0);
            for group in cloud_by_provider(&snapshot) {
                let state = self.cards.entry(group.provider.id.clone()).or_default();
                let has_key = snapshot.keys.get(&group.provider.id).copied().unwrap_or(false);
                let connected = provider_connected(&snapshot, &group.provider.id);
                let active = |id: &str| snapshot.status.get(id).is_some_and(|s| s.active);
                provider_card(ui, cx, act, state, group.provider, &group.models, has_key, connected, &active);
            }
        });
        ui.add_space(8.0);
        let footer = format!(
            "Prices are list prices in US dollars, read on {}. Check the provider\u{2019}s page before relying on them.",
            format_date(&snapshot.catalog.as_of)
        );
        text::paragraph(ui, footer, TextStyle::CAPTION, t.fg_2);
        ui.add_space(32.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn provider_card(
    ui: &mut Ui,
    cx: &mut Cx<'_>,
    act: &mut Act,
    state: &mut CardState,
    provider: &ProviderEntry,
    models: &[&CloudModelEntry],
    has_key: bool,
    connected: bool,
    active: &dyn Fn(&str) -> bool,
) {
    let t = tokens(ui.ctx());
    let id = Id::unique(("flow-models-provider", &provider.id));
    if let Some(result) = state.testing.as_mut().and_then(Pending::take) {
        state.testing = None;
        state.result = Some(result.unwrap_or_else(|err| KeyTestResult { ok: false, message: error_message(&err) }));
    }
    // A submission keeps going if the form was cancelled meanwhile, as the TSX's promise does.
    if !state.editing
        && let Some(result) = state.form.pump(cx, &provider.id)
    {
        state.result = Some(result);
        cx.catalog.reload();
    }

    let mut activate = None;
    card(ui, Margin::ZERO, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        padded(ui, 16.0, |ui| {
            ui.add_space(12.0);
            let main = |ui: &mut Ui| {
                let mut items = vec![Inline::text(ui, &provider.name, TextStyle::BODY.weight(Weight::Semibold), t.fg)];
                if provider.keyless {
                    items.push(Inline::Badge("No key needed".into(), BadgeTone::Neutral));
                } else if has_key {
                    match &state.result {
                        Some(result) if !result.ok => {
                            items.push(Inline::Badge("Key not working".into(), BadgeTone::Warn))
                        }
                        _ => items.push(Inline::Badge("Connected".into(), BadgeTone::Ok)),
                    }
                } else {
                    let hint = format!("Add your {} key to use these models.", provider.name);
                    items.push(Inline::text(ui, &hint, TextStyle::CAPTION, t.fg_2));
                }
                flow(ui, vec2(12.0, 12.0), items);
            };
            let editing = state.editing;
            let testing = state.testing.is_some();
            let side = |ui: &mut Ui| {
                let (mut connect, mut test, mut replace, mut remove) = (false, false, false, false);
                if !provider.keyless && !has_key && !editing {
                    connect = Button::new("Connect").show(ui).clicked();
                }
                if has_key && !editing {
                    test = Button::new(if testing { "Testing…" } else { "Test key" })
                        .subtle()
                        .disabled(testing)
                        .show(ui)
                        .clicked();
                    replace = Button::new("Replace key").subtle().show(ui).clicked();
                    remove = Button::new("Remove key").subtle().show(ui).clicked();
                }
                (connect || replace, test, remove)
            };
            let ((), (edit, test, remove)) = flex_row(ui, id.with("head"), 12.0, Cross::Center(32.0), 4.0, main, side);
            if edit {
                state.editing = true;
                state.form.key.clear();
            }
            if test {
                state.result = None;
                state.testing = Some(cx.link.call(Request::SecretsTest(provider.id.clone())));
            }
            if remove {
                state.confirm = true;
            }
            if state.editing {
                ui.add_space(8.0);
                let response = state.form.show(ui, cx, provider, true);
                if response.cancelled {
                    state.editing = false;
                }
                if let Some(result) = response.done {
                    state.result = Some(result);
                    state.editing = false;
                    cx.catalog.reload();
                }
            }
            if let Some(result) = &state.result {
                ui.add_space(8.0);
                let (tone, message) = if result.ok {
                    let message =
                        if result.message.is_empty() { "The key works.".to_string() } else { result.message.clone() };
                    (Tone::Ok, message)
                } else {
                    (Tone::Danger, format!("The key was saved, but the test failed: {}", result.message))
                };
                Notice::new(tone, &message).show(ui);
            }
            ui.add_space(12.0);
        });
        for entry in models {
            if let Some(id) = model_row(ui, entry, connected, active(&entry.id)) {
                activate = Some(id);
            }
        }
    });
    if let Some(id) = activate {
        act.call(cx, Request::ModelsActivate(id));
    }

    if state.confirm {
        let title = format!("Remove the {} key?", provider.name);
        let description =
            format!("Flow forgets the key. {} models stop working until you connect again.", provider.name);
        let dialog = ConfirmDialog::new(("models-remove-key", &provider.id), &title, &description, "Remove key");
        if dialog.show(ui.ctx(), &mut state.confirm) {
            state.result = None;
            act.call(cx, Request::SecretsDelete(provider.id.clone()));
        }
    }
}

/// One cloud model (`ModelRow`). Returns its id when "Set active" was clicked.
fn model_row(ui: &mut Ui, entry: &CloudModelEntry, connected: bool, active: bool) -> Option<String> {
    let t = tokens(ui.ctx());
    rule(ui);
    let id = Id::unique(("flow-models-cloud", &entry.id));
    padded(ui, 16.0, |ui| {
        ui.add_space(12.0);
        let main = |ui: &mut Ui| {
            let mut items = vec![Inline::text(ui, &entry.name, TextStyle::BODY, t.fg)];
            if entry.recommended {
                items.push(Inline::Badge("Recommended".into(), BadgeTone::Accent));
            }
            flow(ui, vec2(8.0, 8.0), items);
            ui.add_space(2.0);
            let mut description = sentence(&entry.description);
            if let Some(seconds) = entry.min_billed_seconds.filter(|s| *s != 0.0) {
                description.push_str(&format!(" Billed for at least {seconds} seconds per request."));
            }
            text::paragraph(ui, description, TextStyle::CAPTION, t.fg_2);
        };
        let side = |ui: &mut Ui| {
            // The price, right-aligned over "per hour".
            let price = text::galley(ui, format_price(entry.price_per_hour), TextStyle::BODY, t.fg, f32::INFINITY);
            let per = text::galley(ui, "per hour", TextStyle::CAPTION, t.fg_2, f32::INFINITY);
            let width = price.size().x.max(per.size().x);
            let (rect, _) = ui.allocate_exact_size(vec2(width, price.size().y + per.size().y), Sense::hover());
            let per_top = rect.top() + price.size().y;
            ui.painter().galley(pos2(rect.right() - per.size().x, per_top), per, t.fg_2);
            ui.painter().galley(pos2(rect.right() - price.size().x, rect.top()), price, t.fg);
            // `flex w-[96px] shrink-0 justify-end`
            let height = if active { 20.0 } else { 32.0 };
            ui.allocate_ui_with_layout(vec2(96.0, height), Layout::right_to_left(Align::Center), |ui| {
                if active {
                    badge(ui, "Active", BadgeTone::Accent);
                    false
                } else {
                    Button::new("Set active").disabled(!connected).show(ui).clicked()
                }
            })
            .inner
        };
        let ((), clicked) = flex_row(ui, id, 24.0, Cross::Center(0.0), 24.0, main, side);
        ui.add_space(12.0);
        clicked.then(|| entry.id.clone())
    })
}
