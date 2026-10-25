//! Key entry for one provider: store it, test it, show the result. Port of
//! `KeyForm` in `pages/models/CloudProviders.tsx`, which the cloud choice uses.
//! The keys go one way only: into the main process (`SecretsSet`).

use egui::{Align, Key, Layout, Ui, vec2};
use flow_core::catalog::ProviderEntry;
use flow_core::protocol::settings::{KeyTestResult, Request};

use crate::backend::{Link, Pending};
use crate::components::{Button, IconSpec, password_input};
use crate::design::text::{self, Rich, TextStyle};
use crate::design::{Icon, motion, tokens};
use crate::util::format::error_message;

enum Submit {
    Storing(Pending<()>),
    Testing(Pending<KeyTestResult>),
}

#[derive(Default)]
pub struct KeyForm {
    key: String,
    submit: Option<Submit>,
}

impl KeyForm {
    fn busy(&self) -> bool {
        self.submit.is_some()
    }

    /// Draws the form. Returns the test result once a key was stored (`onDone`).
    pub fn show(&mut self, ui: &mut Ui, link: &Link, provider: &ProviderEntry) -> Option<KeyTestResult> {
        let t = tokens(ui.ctx());
        let done = self.poll(link, &provider.id);

        // `flex flex-col gap-2`
        let label =
            format!("{} API key. It is encrypted on this PC and never written to the config file.", provider.name);
        text::paragraph(ui, label, TextStyle::CAPTION, t.fg_2);
        ui.add_space(8.0);

        let busy = self.busy();
        let caption = if busy { "Testing…" } else { "Save and test" };
        let mut button = Button::new(caption).accent().disabled(self.key.trim().is_empty() || busy);
        let mut button_w = 24.0 + text::width(ui, caption, TextStyle::BODY.weight(crate::design::Weight::Strong));
        if busy {
            // `<LoaderCircle className="spin" />`: one turn per 900 ms.
            let rotation = if motion::enabled(ui) {
                ui.ctx().request_repaint();
                (ui.input(|i| i.time) % 0.9 / 0.9) as f32 * std::f32::consts::TAU
            } else {
                0.0
            };
            button = button.icon_spec(IconSpec { rotation, ..IconSpec::new(Icon::LoaderCircle, 15.0) });
            button_w += 15.0 - 2.0 + 8.0;
        }
        let mut submit = false;
        let width = ui.available_width();
        ui.allocate_ui_with_layout(vec2(width, 32.0), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // `input min-w-[220px] flex-1 font-mono text-[13px]`
            let input_w = (width - 8.0 - button_w).max(220.0);
            let input =
                password_input(ui, ("onboarding.key", &provider.id), &mut self.key, input_w, Some("Paste the key"));
            if input.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
            }
            if button.show(ui).clicked() {
                submit = true;
            }
        });
        if submit {
            self.start(link, &provider.id);
        }

        if let Some(url) = provider.key_url.as_deref().filter(|u| u.starts_with("https://")) {
            ui.add_space(8.0);
            let clicked = Rich::new(TextStyle::CAPTION, t.fg_2)
                .text("No key yet? ")
                .link(format!("Get a key from {}", provider.name))
                .show(ui)
                .clicked;
            if clicked.is_some() {
                link.send(Request::AppOpenExternal(url.to_string()));
            }
        }
        done
    }

    fn start(&mut self, link: &Link, provider: &str) {
        let trimmed = self.key.trim();
        if trimmed.is_empty() || self.busy() {
            return;
        }
        let request = Request::SecretsSet { provider: provider.to_string(), key: trimmed.to_string() };
        self.submit = Some(Submit::Storing(link.call(request)));
    }

    /// Moves the submission along: store, then test.
    fn poll(&mut self, link: &Link, provider: &str) -> Option<KeyTestResult> {
        match self.submit.as_mut()? {
            Submit::Storing(pending) => match pending.take()? {
                Ok(()) => {
                    self.submit = Some(Submit::Testing(link.call(Request::SecretsTest(provider.to_string()))));
                    None
                }
                Err(err) => {
                    self.submit = None;
                    Some(KeyTestResult { ok: false, message: error_message(&err) })
                }
            },
            Submit::Testing(pending) => {
                let result = pending.take()?;
                self.submit = None;
                Some(match result {
                    Ok(result) => {
                        self.key.clear();
                        result
                    }
                    Err(err) => KeyTestResult { ok: false, message: error_message(&err) },
                })
            }
        }
    }
}
