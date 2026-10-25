//! The Custom endpoint section of the Models page: OpenAI-compatible servers
//! kept in `[providers.<id>]` tables (`util::models::custom_endpoints`), with
//! an inline add form and a confirm before removal. Port of
//! `pages/models/CustomEndpoints.tsx`.

use std::collections::BTreeSet;

use egui::{Id, Margin, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use flow_core::catalog::CUSTOM_PREFIX;
use flow_core::config::ConfigPatch;
use flow_core::path;
use flow_core::protocol::settings::Request;
use serde_json::json;

use super::{Act, Cross, Inline, Step, flex_row, flow};
use crate::components::{BadgeTone, Button, ConfirmDialog, Group, IconButton, Input, badge, card, empty_state};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, tokens};
use crate::pages::Cx;
use crate::util::models::{CustomEndpoint, custom_endpoints};

/// `^[a-z0-9][a-z0-9._-]*$`
fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// JavaScript's `\s` and `trim()`: Unicode white space plus the BOM.
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// What the user types becomes the table name in `[providers.<id>]`.
pub fn endpoint_id(name: &str) -> String {
    let lowered = name.trim_matches(js_space).to_lowercase();
    let mut dashed = String::with_capacity(lowered.len());
    let mut in_space = false;
    for c in lowered.chars() {
        if js_space(c) {
            if !in_space {
                dashed.push('-');
            }
            in_space = true;
        } else {
            dashed.push(c);
            in_space = false;
        }
    }
    let kept: String = dashed
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        .collect();
    kept.trim_start_matches(['.', '_', '-']).chars().take(64).collect()
}

/// `new URL(text)` succeeds with an `http:` or `https:` protocol. A port of the
/// parts of the WHATWG parser that decide this: the scheme, a non-empty host
/// without forbidden characters, and a numeric port.
pub fn is_http_url(text: &str) -> bool {
    let text: String = text.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let text = text.trim_matches(|c: char| c <= ' ');
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let rest = rest.trim_start_matches(['/', '\\']);
    let authority = &rest[..rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len())];
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let (host, port) = if let Some(inner) = host_port.strip_prefix('[') {
        let Some((address, after)) = inner.split_once(']') else {
            return false;
        };
        if address.is_empty() || !address.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.')) {
            return false;
        }
        match after {
            "" => (address, ""),
            _ => match after.strip_prefix(':') {
                Some(port) => (address, port),
                None => return false,
            },
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, port),
            None => (host_port, ""),
        }
    };
    const FORBIDDEN: &[char] = &[' ', '#', '/', ':', '<', '>', '?', '@', '[', '\\', ']', '^', '|'];
    if host.is_empty() || (!host_port.starts_with('[') && host.chars().any(|c| FORBIDDEN.contains(&c) || c < ' ')) {
        return false;
    }
    port.is_empty() || (port.chars().all(|c| c.is_ascii_digit()) && port.parse::<u32>().is_ok_and(|p| p <= 65535))
}

/// The inline "Add a custom endpoint" form (`AddForm`).
#[derive(Default)]
struct AddForm {
    name: String,
    base_url: String,
    model: String,
    key: String,
    /// Errors show once the user has tried to submit.
    tried: bool,
    /// The act storing the endpoint; the form closes when it is done.
    submitting: Option<u64>,
}

impl AddForm {
    /// The first problem, in field order, or `None` when the form can be saved.
    fn first_error(&self, taken: &BTreeSet<String>) -> Option<String> {
        let id = endpoint_id(&self.name);
        let name_error = if id.is_empty() {
            Some("Give the endpoint a short name.".to_string())
        } else if !valid_id(&id) {
            Some("Use letters, digits, dots and dashes.".to_string())
        } else if taken.contains(&id) {
            Some(format!("\u{201c}{id}\u{201d} is already in use."))
        } else {
            None
        };
        let url_error = (!is_http_url(self.base_url.trim()))
            .then(|| "Enter a full URL, such as http://192.168.1.20:8000/v1.".to_string());
        let model_error = self.model.trim().is_empty().then(|| "Enter the model name the server expects.".to_string());
        name_error.or(url_error).or(model_error)
    }

    fn submit(&mut self, cx: &mut Cx<'_>, act: &mut Act, taken: &BTreeSet<String>) {
        self.tried = true;
        if self.first_error(taken).is_some() || self.submitting.is_some() {
            return;
        }
        let id = endpoint_id(&self.name);
        let secret = self.key.trim().to_string();
        let mut table = json!({
            "base_url": self.base_url.trim().trim_end_matches('/'),
            "model": self.model.trim(),
        });
        if !secret.is_empty() {
            table["api_key"] = json!(format!("secret:{id}"));
        }
        let mut steps = Vec::new();
        if !secret.is_empty() {
            steps.push(Step::Call(Request::SecretsSet { provider: id.clone(), key: secret }));
        }
        steps.push(Step::Patch(vec![ConfigPatch::set(path!["providers", id.as_str()], table)]));
        self.submitting = Some(act.run(cx, steps));
    }

    /// Draws the form; returns true when it should close.
    fn show(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, act: &mut Act, taken: &BTreeSet<String>) -> bool {
        let t = tokens(ui.ctx());
        let id = endpoint_id(&self.name);
        let name_hint = if !id.is_empty() && id != self.name {
            format!("Saved as \u{201c}{id}\u{201d} in the config file.")
        } else {
            "A short id for this server.".to_string()
        };
        let mut entered = false;
        let mut close = false;
        let mut submit = false;
        card(ui, Margin::same(16), |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // `grid grid-cols-2 gap-x-4 gap-y-3`
            let width = ui.available_width();
            let col_w = ((width - 16.0) / 2.0).max(0.0);
            let Self { name, base_url, model, key, .. } = self;
            let rows: [[Field<'_>; 2]; 2] = [
                [
                    Field {
                        salt: "name",
                        label: "Name",
                        text: name,
                        hint: &name_hint,
                        placeholder: Some("custom-lan"),
                        mono: false,
                        password: false,
                    },
                    Field {
                        salt: "model",
                        label: "Model name",
                        text: model,
                        hint: "Sent to the server with each request.",
                        placeholder: Some("whisper-1"),
                        mono: false,
                        password: false,
                    },
                ],
                [
                    Field {
                        salt: "url",
                        label: "Base URL",
                        text: base_url,
                        hint: "The address up to and including /v1.",
                        placeholder: Some("http://192.168.1.20:8000/v1"),
                        mono: true,
                        password: false,
                    },
                    Field {
                        salt: "key",
                        label: "API key (optional)",
                        text: key,
                        hint: "Encrypted on this PC, not saved in the file.",
                        placeholder: None,
                        mono: true,
                        password: true,
                    },
                ],
            ];
            for (n, row) in rows.into_iter().enumerate() {
                if n > 0 {
                    ui.add_space(12.0);
                }
                let top = ui.cursor().min;
                let mut bottom = top.y;
                for (i, field) in row.into_iter().enumerate() {
                    let rect =
                        Rect::from_min_size(pos2(top.x + i as f32 * (col_w + 16.0), top.y), vec2(col_w, f32::INFINITY));
                    let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
                    entered |= field.show(&mut child);
                    bottom = bottom.max(child.min_rect().bottom());
                }
                ui.advance_cursor_after_rect(Rect::from_min_max(top, pos2(top.x + width, bottom)));
            }
            ui.add_space(12.0);
            // `flex items-center gap-2`
            let error = if self.tried { self.first_error(taken) } else { None };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                submit = Button::new("Add endpoint").accent().show(ui).clicked();
                close = Button::new("Cancel").subtle().show(ui).clicked();
                if let Some(error) = error {
                    let galley = text::galley(ui, error, TextStyle::CAPTION, t.danger, f32::INFINITY);
                    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x, 32.0), Sense::hover());
                    ui.painter().galley(pos2(rect.left(), rect.center().y - galley.size().y / 2.0), galley, t.danger);
                }
            });
        });
        if submit || entered {
            self.submit(cx, act, taken);
        }
        if let Some(job) = self.submitting
            && !act.running(job)
        {
            close = true;
        }
        close
    }
}

/// One labelled box of the add form (`flex flex-col gap-1`).
struct Field<'a> {
    salt: &'static str,
    label: &'static str,
    text: &'a mut String,
    hint: &'a str,
    placeholder: Option<&'static str>,
    mono: bool,
    password: bool,
}

impl Field<'_> {
    /// Returns true when Enter was pressed in the box (the form submits).
    fn show(self, ui: &mut Ui) -> bool {
        let t = tokens(ui.ctx());
        let salt = ("custom-endpoint", self.salt);
        // `<label htmlFor>`: a click on the label focuses the box.
        let label = text::label(ui, self.label, TextStyle::BODY, t.fg).rect;
        ui.add_space(4.0);
        let mut input = Input::new(salt, self.text).width(ui.available_width());
        if let Some(placeholder) = self.placeholder {
            input = input.placeholder(placeholder);
        }
        if self.mono {
            input = input.mono();
        }
        if self.password {
            input = input.password();
        }
        let response = input.show(ui);
        // After the box, which gives up focus on a click elsewhere.
        if ui.interact(label, Id::unique(salt).with("label"), Sense::click()).clicked() {
            ui.memory_mut(|m| m.request_focus(Id::unique(("flow-input", salt))));
        }
        ui.add_space(4.0);
        text::paragraph(ui, self.hint, TextStyle::CAPTION, t.fg_2);
        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
    }
}

/// The Custom endpoint section.
#[derive(Default)]
pub struct CustomEndpoints {
    adding: bool,
    form: AddForm,
    /// The endpoint whose "Remove?" dialog is open.
    confirm: Option<String>,
}

impl CustomEndpoints {
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx<'_>, act: &mut Act) {
        let config = cx.config.config().clone();
        let snapshot = cx.catalog.snapshot().cloned();
        let endpoints = custom_endpoints(&config, snapshot.as_ref());
        let mut taken: BTreeSet<String> = config.providers.keys().cloned().collect();
        if let Some(snapshot) = &snapshot {
            taken.extend(snapshot.catalog.providers.iter().map(|p| p.id.clone()));
        }
        let has_key = |id: &str| snapshot.as_ref().is_some_and(|s| s.keys.get(id).copied().unwrap_or(false));

        let mut add = false;
        let mut group = Group::new("Custom endpoint").description(
            "Any server that speaks the OpenAI transcription API: a self-hosted Whisper, or a GPU machine on your network.",
        );
        if !self.adding {
            group = group.action(|ui| add = Button::new("Add endpoint").icon(Icon::Plus, 15.0).show(ui).clicked());
        }
        group.show(ui, |ui| {
            for endpoint in &endpoints {
                let model_id = format!("{CUSTOM_PREFIX}{}", endpoint.id);
                let active = config.model.active == model_id;
                match endpoint_row(ui, endpoint, active, has_key(&endpoint.id)) {
                    Some(RowAction::Activate) => {
                        act.call(cx, Request::ModelsActivate(model_id));
                    }
                    Some(RowAction::Remove) => self.confirm = Some(endpoint.id.clone()),
                    None => {}
                }
            }
            if endpoints.is_empty() && !self.adding {
                empty_state(ui, "No custom endpoints yet.", |_| {});
            }
            if self.adding && self.form.show(ui, cx, act, &taken) {
                self.adding = false;
                self.form = AddForm::default();
            }
        });
        if add {
            self.adding = true;
        }

        // The confirm dialog for the endpoint being removed.
        let Some(id) = self.confirm.clone() else {
            return;
        };
        let Some(endpoint) = endpoints.iter().find(|e| e.id == id) else {
            self.confirm = None;
            return;
        };
        let model_id = format!("{CUSTOM_PREFIX}{}", endpoint.id);
        let active = config.model.active == model_id;
        let title = format!("Remove {}?", endpoint.id);
        let description = if active {
            "This endpoint is the active model. Flow switches to your fallback model and forgets the endpoint and its key."
        } else {
            "Flow forgets this endpoint and its key."
        };
        let mut open = true;
        if ConfirmDialog::new(("models-remove-endpoint", &id), &title, description, "Remove endpoint")
            .show(ui.ctx(), &mut open)
        {
            let mut steps = Vec::new();
            if active {
                steps.push(Step::Call(Request::ModelsActivate(config.model.fallback.clone())));
            }
            steps.push(Step::Patch(vec![ConfigPatch::remove(path!["providers", id.as_str()])]));
            if has_key(&id) {
                steps.push(Step::Call(Request::SecretsDelete(id.clone())));
            }
            act.run(cx, steps);
        }
        if !open {
            self.confirm = None;
        }
    }
}

enum RowAction {
    Activate,
    Remove,
}

/// One endpoint (`EndpointRow`).
fn endpoint_row(ui: &mut Ui, endpoint: &CustomEndpoint, active: bool, key_stored: bool) -> Option<RowAction> {
    let t = tokens(ui.ctx());
    let id = Id::unique(("flow-models-endpoint", &endpoint.id));
    card(ui, Margin::symmetric(16, 12), |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let main = |ui: &mut Ui| {
            let mut items = vec![Inline::text(ui, &endpoint.id, TextStyle::BODY.weight(Weight::Semibold), t.fg)];
            if !endpoint.model.is_empty() {
                items.push(Inline::text(ui, &endpoint.model, TextStyle::BODY, t.fg_2));
            }
            if key_stored {
                items.push(Inline::Badge("Key stored".into(), BadgeTone::Neutral));
            }
            flow(ui, vec2(8.0, 8.0), items);
            ui.add_space(2.0);
            // `selectable truncate font-mono`
            let galley =
                text::galley_truncated(ui, &endpoint.base_url, TextStyle::CAPTION.mono(), t.fg_2, ui.available_width());
            ui.scope(|ui| {
                ui.style_mut().interaction.selectable_labels = true;
                ui.add(egui::Label::new(galley).selectable(true));
            });
        };
        let side = |ui: &mut Ui| {
            let mut action = None;
            if active {
                badge(ui, "Active", BadgeTone::Accent);
            } else if Button::new("Set active").show(ui).clicked() {
                action = Some(RowAction::Activate);
            }
            let label = format!("Remove {}", endpoint.id);
            if IconButton::new(label, Icon::Trash2, 15.0).show(ui).clicked() {
                action = Some(RowAction::Remove);
            }
            action
        };
        flex_row(ui, id, 24.0, Cross::Center(0.0), 4.0, main, side).1
    })
    .0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_ids_follow_the_provider_table_rules() {
        assert_eq!(endpoint_id("  My  LAN box "), "my-lan-box");
        assert_eq!(endpoint_id("--Whisper.Server_1"), "whisper.server_1");
        assert_eq!(endpoint_id("Ünïcode!"), "ncode");
        assert_eq!(endpoint_id("   "), "");
        assert_eq!(endpoint_id(&"a".repeat(80)).len(), 64);
        assert!(valid_id("custom-lan") && !valid_id("") && !valid_id("-x"));
    }

    #[test]
    fn accepts_only_http_urls() {
        assert!(is_http_url("http://192.168.1.20:8000/v1"));
        assert!(is_http_url("https://example.com"));
        assert!(is_http_url("HTTPS://user:pass@example.com:443/v1?x#y"));
        assert!(is_http_url("http://[::1]:8000/v1"));
        assert!(!is_http_url(""));
        assert!(!is_http_url("192.168.1.20:8000/v1"));
        assert!(!is_http_url("ftp://example.com"));
        assert!(!is_http_url("http://"));
        assert!(!is_http_url("http://exa mple.com"));
        assert!(!is_http_url("http://example.com:99999"));
        assert!(!is_http_url("http://example.com:port"));
    }
}
