//! Advanced: the config file, export and import, how text is inserted,
//! troubleshooting (debug logs, folders, updates) and About with the model
//! licences. Port of `pages/Advanced.tsx`.

use std::path::PathBuf;

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Layout, Margin, Rect, Ui, UiBuilder, pos2, vec2};
use flow_core::catalog::LocalModelEntry;
use flow_core::config::InsertMethod;
use flow_core::path;
use flow_core::protocol::settings::Request;
use flow_core::types::AppInfo;
use serde::de::IgnoredAny;

use super::{Cx, Page};
use crate::backend::{Pending, take_settled};
use crate::components::{
    Button, Group, Notice, Select, SelectOption, SettingRow, Toggle, Tone, card, file_dialog, link, page_header,
};
use crate::design::text::{self, TextStyle};
use crate::design::{Weight, tokens};
use crate::util::format::error_message;

fn methods() -> [SelectOption<InsertMethod>; 2] {
    [SelectOption::new(InsertMethod::Paste, "Paste"), SelectOption::new(InsertMethod::Type, "Type")]
}

const SHORTCUTS: [&str; 3] = ["Ctrl+V", "Ctrl+Shift+V", "Shift+Insert"];

/// The paste shortcuts to pick from, plus the configured one when it is not
/// among them ("From the file").
pub fn shortcut_options(current: &str) -> Vec<SelectOption<String>> {
    let mut options: Vec<_> = SHORTCUTS.iter().map(|s| SelectOption::new(s.to_string(), *s)).collect();
    if !SHORTCUTS.contains(&current) {
        options.push(SelectOption::new(current.to_string(), current).hint("From the file"));
    }
    options
}

const TOML: file_dialog::Filter<'static> = ("TOML", &["toml"]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Export,
    Import,
}

/// The outcome of the last export or import (`{ ok, message }`).
struct TransferResult {
    ok: bool,
    message: String,
}

#[derive(Default)]
pub struct Advanced {
    transfer: Option<TransferResult>,
    /// The file dialog that is open.
    picking: Option<(Transfer, Pending<Option<PathBuf>>)>,
    /// The export or import the main process is doing.
    moving: Option<(Transfer, Pending<IgnoredAny>)>,
    update: Option<String>,
    checking: Option<Pending<String>>,
}

impl Advanced {
    fn run_transfer(&mut self, ctx: &egui::Context, kind: Transfer) {
        if self.picking.is_some() || self.moving.is_some() {
            return;
        }
        let dialog = match kind {
            Transfer::Export => file_dialog::save(ctx, "Export Flow settings", "flow-config.toml", TOML),
            Transfer::Import => file_dialog::open(ctx, "Import Flow settings", TOML),
        };
        self.picking = Some((kind, dialog));
    }

    /// Moves an export or import along: dialog → main process → message.
    fn settle_transfer(&mut self, cx: &Cx<'_>) {
        if let Some((kind, pending)) = self.picking.as_mut()
            && let Some(result) = pending.take()
        {
            let kind = *kind;
            self.picking = None;
            match result {
                Ok(Some(path)) => {
                    let path = path.to_string_lossy().into_owned();
                    let request = match kind {
                        Transfer::Export => Request::ConfigExport(path),
                        Transfer::Import => Request::ConfigImport(path),
                    };
                    self.moving = Some((kind, cx.link.call(request)));
                }
                // Cancelled: the last message goes away.
                Ok(None) => self.transfer = None,
                Err(err) => self.transfer = Some(TransferResult { ok: false, message: error_message(&err) }),
            }
        }
        if let Some((kind, pending)) = self.moving.as_mut()
            && let Some(result) = pending.take()
        {
            let kind = *kind;
            self.moving = None;
            self.transfer = Some(match result {
                Ok(_) => TransferResult {
                    ok: true,
                    message: match kind {
                        Transfer::Export => "Settings exported. API keys were left out.".into(),
                        Transfer::Import => "Settings imported. Reconnect your API keys on the Models page.".into(),
                    },
                },
                Err(err) => TransferResult { ok: false, message: error_message(&err) },
            });
        }
    }
}

/// `PathLine`: a file path under a row's description (`selectable mt-1 block
/// font-mono text-[11px] break-all text-fg-2`). Nothing when the path is unknown.
fn path_line(ui: &mut Ui, path: &str) {
    if path.is_empty() {
        return;
    }
    let t = tokens(ui.ctx());
    ui.add_space(4.0);
    let style = TextStyle::CAPTION.mono().size(11.0, 16.0);
    let mut job = LayoutJob::single_section(path.to_string(), style.format(t.fg_2));
    job.wrap = TextWrapping { max_width: ui.available_width(), break_anywhere: true, ..TextWrapping::default() };
    let galley = text::snap_baselines(ui.painter().layout_job(job), style);
    selectable(ui, galley);
}

/// Selectable text at the cursor, taking exactly its galley's size.
fn selectable(ui: &mut Ui, galley: std::sync::Arc<egui::Galley>) {
    let rect = Rect::from_min_size(ui.cursor().min, galley.size());
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    child.style_mut().interaction.selectable_labels = true;
    child.add(egui::Label::new(galley).selectable(true));
    ui.advance_cursor_after_rect(rect);
}

/// A description with a path line under it.
fn with_path<'a>(sentence: &'a str, path: String) -> impl FnOnce(&mut Ui) + 'a {
    move |ui: &mut Ui| {
        // Not the 3 px between a group's cards.
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let t = tokens(ui.ctx());
        text::paragraph(ui, sentence, TextStyle::CAPTION, t.fg_2);
        path_line(ui, &path);
    }
}

impl Page for Advanced {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        self.settle_transfer(cx);
        if let Some(result) = take_settled(&mut self.checking) {
            self.update = Some(match result {
                Ok(line) => line,
                Err(err) => format!("The update check failed: {}", error_message(&err)),
            });
        }

        let config = cx.config.config();
        let (general, insert) = (config.general.clone(), config.insert.clone());
        let info = cx.app_info.info().cloned();
        let snapshot_path = cx.config.snapshot().path.clone();
        let config_path = if snapshot_path.is_empty() {
            info.as_ref().map(|i| i.config_path.clone()).unwrap_or_default()
        } else {
            snapshot_path
        };
        let typing = insert.method == InsertMethod::Type;

        page_header(ui, "Advanced", None);

        Group::new("Config file")
            .description(
                "Every setting in this window lives in one text file. Edits made in a text editor apply within a \
                 second.",
            )
            .show(ui, |ui| {
                SettingRow::new_with("config.toml", with_path("Open it in your default editor.", config_path)).show(
                    ui,
                    |ui| {
                        if Button::new("Open config file").show(ui).clicked() {
                            cx.link.send(Request::ConfigOpenFile);
                        }
                    },
                );
                let mut clicked = None;
                SettingRow::new(
                    "Export or import settings",
                    "Move your settings to another PC. API keys are never included.",
                )
                .show_with(
                    ui,
                    |ui| {
                        if Button::new("Export").show(ui).clicked() {
                            clicked = Some(Transfer::Export);
                        }
                        if Button::new("Import").show(ui).clicked() {
                            clicked = Some(Transfer::Import);
                        }
                    },
                    |ui| {
                        if let Some(transfer) = &self.transfer {
                            ui.add_space(12.0);
                            Notice::new(if transfer.ok { Tone::Ok } else { Tone::Danger }, &transfer.message).show(ui);
                        }
                    },
                );
                if let Some(kind) = clicked {
                    self.run_transfer(&ctx, kind);
                }
            });

        Group::new("Inserting text").description("How the transcript reaches the app you are typing in.").show(
            ui,
            |ui| {
                SettingRow::new(
                    "Method",
                    "Paste is fast and reliable. Type sends each character as a keystroke, for apps that block paste.",
                )
                .show(ui, |ui| {
                    if let Some(v) = Select::new("insert.method", Some(&insert.method), &methods()).show(ui).changed {
                        cx.config.set(path!["insert", "method"], v);
                    }
                });
                SettingRow::new(
                    "Paste shortcut",
                    "The keys Flow presses to paste. Per-app exceptions go on the Profiles page.",
                )
                .disabled(typing)
                .show(ui, |ui| {
                    let options = shortcut_options(&insert.paste_shortcut);
                    let select =
                        Select::new("insert.paste_shortcut", Some(&insert.paste_shortcut), &options).disabled(typing);
                    if let Some(v) = select.show(ui).changed {
                        cx.config.set(path!["insert", "paste_shortcut"], v);
                    }
                });
                SettingRow::new("Restore clipboard", "Put back whatever you had copied once the text has been pasted.")
                    .disabled(typing)
                    .show(ui, |ui| {
                        if let Some(v) = Toggle::new(insert.restore_clipboard).disabled(typing).show(ui).changed {
                            cx.config.set(path!["insert", "restore_clipboard"], v);
                        }
                    });
                SettingRow::new(
                    "Trailing space",
                    "Add a space after each dictation so the next one does not run into it.",
                )
                .show(ui, |ui| {
                    if let Some(v) = Toggle::new(insert.trailing_space).show(ui).changed {
                        cx.config.set(path!["insert", "trailing_space"], v);
                    }
                });
                SettingRow::new(
                    "Match case",
                    "Start with a lower-case letter when you are dictating into the middle of a sentence.",
                )
                .show(ui, |ui| {
                    if let Some(v) = Toggle::new(insert.match_case).show(ui).changed {
                        cx.config.set(path!["insert", "match_case"], v);
                    }
                });
            },
        );

        Group::new("Troubleshooting").show(ui, |ui| {
            SettingRow::new(
                "Debug logging",
                "Writes detailed logs. While this is on, the logs include the text of your dictations.",
            )
            .show_with(
                ui,
                |ui| {
                    if let Some(v) = Toggle::new(general.debug_logging).show(ui).changed {
                        cx.config.set(path!["general", "debug_logging"], v);
                    }
                },
                |ui| {
                    if general.debug_logging {
                        ui.add_space(12.0);
                        Notice::new(
                            Tone::Warn,
                            "Logs now contain transcript text. Turn this off when you have finished, and check a log \
                             before sharing it.",
                        )
                        .show(ui);
                    }
                },
            );
            let logs_path = info.as_ref().map(|i| i.logs_path.clone()).unwrap_or_default();
            SettingRow::new_with("Logs folder", with_path("Where Flow writes its log files.", logs_path)).show(
                ui,
                |ui| {
                    if Button::new("Open logs folder").show(ui).clicked() {
                        cx.link.send(Request::AppOpenLogs);
                    }
                },
            );
            let models_path = info.as_ref().map(|i| i.models_path.clone()).unwrap_or_default();
            SettingRow::new_with("Models folder", with_path("Where downloaded model files are stored.", models_path))
                .show(ui, |ui| {
                    if Button::new("Open models folder").show(ui).clicked() {
                        cx.link.send(Request::AppOpenModels);
                    }
                });
            let checking = self.checking.is_some();
            let updates = self
                .update
                .clone()
                .unwrap_or_else(|| "Flow checks GitHub Releases and installs signed updates only.".to_string());
            SettingRow::new("Updates", updates).show(ui, |ui| {
                let label = if checking { "Checking…" } else { "Check for updates" };
                if Button::new(label).disabled(checking).show(ui).clicked() {
                    self.checking = Some(cx.link.call(Request::AppCheckUpdates));
                }
            });
        });

        let models = cx.catalog.snapshot().map(|s| s.catalog.local.clone()).unwrap_or_default();
        Group::new("About").show(ui, |ui| {
            about_card(ui, info.as_ref());
            if let Some(info) = info.as_ref().filter(|i| !i.degraded.is_empty()) {
                degraded_card(ui, info);
            }
            if let Some(url) = licences_card(ui, &models) {
                cx.link.send(Request::AppOpenExternal(url));
            }
        });
    }
}

/// `card px-4 py-3`
const CARD: Margin = Margin { left: 16, right: 16, top: 12, bottom: 12 };

/// Flow, its version, and what it is built on.
fn about_card(ui: &mut Ui, info: Option<&AppInfo>) {
    let t = tokens(ui.ctx());
    card(ui, CARD, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // `flex items-baseline gap-3`: "Flow" in the display face, the version beside it.
        let name = text::galley(ui, "Flow", TextStyle::SUBTITLE.weight(Weight::Heavy), t.fg, f32::INFINITY);
        let version = match info {
            Some(info) => format!("Version {}", info.version),
            None => "Version unknown".to_string(),
        };
        let version = text::galley(ui, version, TextStyle::BODY, t.fg_2, f32::INFINITY);
        let (name_base, version_base) = (text::baseline(&name), text::baseline(&version));
        let ascent = name_base.max(version_base);
        let descent = (name.size().y - name_base).max(version.size().y - version_base);
        let top = ui.cursor().min;
        let name_w = name.size().x;
        ui.painter().galley(pos2(top.x, top.y + ascent - name_base), name, t.fg);
        let version_rect =
            Rect::from_min_size(pos2(top.x + name_w + 12.0, top.y + ascent - version_base), version.size());
        let mut version_ui = ui.new_child(UiBuilder::new().max_rect(version_rect).layout(Layout::top_down(Align::Min)));
        version_ui.style_mut().interaction.selectable_labels = true;
        version_ui.add(egui::Label::new(version).selectable(true));
        ui.advance_cursor_after_rect(Rect::from_min_size(top, vec2(ui.available_width(), ascent + descent)));
        ui.add_space(4.0);
        let mut line = "Free and open source under the MIT licence. No account, no telemetry.".to_string();
        if let Some(info) = info {
            line.push_str(&format!(" Built on {}.", info.runtime));
        }
        text::paragraph(ui, line, TextStyle::CAPTION, t.fg_2);
    });
}

/// Parts of Flow that could not start, one warning each.
fn degraded_card(ui: &mut Ui, info: &AppInfo) {
    let t = tokens(ui.ctx());
    card(ui, CARD, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        text::paragraph(ui, "Unavailable features", TextStyle::BODY, t.fg);
        ui.add_space(2.0);
        text::paragraph(
            ui,
            "Parts of Flow that could not start on this PC. Reinstalling usually fixes them.",
            TextStyle::CAPTION,
            t.fg_2,
        );
        // `ul mt-2 flex flex-col gap-1.5`
        ui.add_space(8.0);
        for (i, item) in info.degraded.iter().enumerate() {
            if i > 0 {
                ui.add_space(6.0);
            }
            let module = format!("({})", item.module);
            Notice::new(Tone::Warn, &item.effect).mono_suffix(&module).show(ui);
        }
    });
}

/// `/^cc-by/i`: the licence asks for attribution.
fn needs_attribution(licence: &str) -> bool {
    licence.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("cc-by"))
}

/// The attribution line under the licence table, when any model needs one.
pub fn attribution(models: &[LocalModelEntry]) -> Option<String> {
    let credited: Vec<String> = models
        .iter()
        .filter(|m| needs_attribution(&m.licence))
        .map(|m| format!("{} is used under {}.", m.name, m.licence))
        .collect();
    (!credited.is_empty()).then(|| format!("{} Local models run on transcribe.cpp, MIT licence.", credited.join(" ")))
}

/// Each local model and its licence. Returns the licence page to open when one was clicked.
fn licences_card(ui: &mut Ui, models: &[LocalModelEntry]) -> Option<String> {
    let t = tokens(ui.ctx());
    let mut open = None;
    card(ui, CARD, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        text::paragraph(ui, "Model licences", TextStyle::BODY, t.fg);
        ui.add_space(2.0);
        text::paragraph(
            ui,
            "Each speech model is published by its authors under its own licence.",
            TextStyle::CAPTION,
            t.fg_2,
        );
        ui.add_space(8.0);
        if models.is_empty() {
            text::paragraph(ui, "The catalog has not loaded yet.", TextStyle::CAPTION, t.fg_2);
            return;
        }
        // `table w-full text-caption`: rows `border-t border-line`, cells `py-1.5`,
        // the name on the left (`pr-4 font-normal`), the licence right-aligned on one line.
        let width = ui.available_width();
        let left = ui.cursor().min.x;
        let mut y = ui.cursor().min.y;
        for model in models {
            ui.painter().hline(left..=left + width, y + 0.5, egui::Stroke::new(1.0, t.line));
            y += 1.0 + 6.0;
            let note = needs_attribution(&model.licence)
                .then(|| text::galley(ui, "Attribution required", TextStyle::CAPTION, t.fg_2, f32::INFINITY));
            let url = model.licence_url.as_deref().filter(|u| u.starts_with("https://"));
            let licence_style =
                if url.is_some() { TextStyle::CAPTION.weight(Weight::Strong) } else { TextStyle::CAPTION };
            let licence_w = text::width(ui, &model.licence, licence_style);
            let note_w = note.as_ref().map(|g| g.size().x + 8.0).unwrap_or(0.0);
            let cell_w = note_w + licence_w;
            let name_w = (width - cell_w - 16.0).max(40.0);
            let name = text::galley(ui, &model.name, TextStyle::CAPTION.weight(Weight::Normal), t.fg, name_w);
            let row_h = name.size().y.max(16.0);
            ui.painter().galley(pos2(left, y + (row_h - name.size().y) / 2.0), name, t.fg);
            let cell_top = y + (row_h - 16.0) / 2.0;
            let mut x = left + width - cell_w;
            if let Some(note) = note {
                ui.painter().galley(pos2(x, cell_top), note, t.fg_2);
                x += note_w;
            }
            let at = Rect::from_min_size(pos2(x, cell_top), vec2(licence_w, 16.0));
            let mut cell = ui.new_child(UiBuilder::new().max_rect(at).layout(Layout::left_to_right(Align::Min)));
            match url {
                Some(url) => {
                    if link(&mut cell, &model.licence, TextStyle::CAPTION).clicked() {
                        open = Some(url.to_string());
                    }
                }
                None => {
                    text::label(&mut cell, &model.licence, TextStyle::CAPTION, t.fg);
                }
            }
            y += row_h + 6.0;
        }
        ui.advance_cursor_after_rect(Rect::from_min_max(ui.cursor().min, pos2(left + width, y)));
        if let Some(line) = attribution(models) {
            // `selectable mt-2 border-t border-line pt-2`
            ui.add_space(8.0);
            let top = ui.cursor().min.y;
            ui.painter().hline(left..=left + width, top + 0.5, egui::Stroke::new(1.0, t.line));
            ui.add_space(1.0 + 8.0);
            let galley = text::galley(ui, line, TextStyle::CAPTION, t.fg_2, width);
            selectable(ui, galley);
        }
    });
    open
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use flow_core::protocol::settings::{Reply, Request};
    use serde_json::json;

    use super::*;
    use crate::pages::PageId;
    use crate::testing::PageHarness;

    fn page(raw: serde_json::Value) -> PageHarness {
        let mut p = PageHarness::new(PageId::Advanced, raw);
        p.h.screen = egui::vec2(1000.0, 3000.0);
        settle(&mut p);
        p
    }

    fn settle(p: &mut PageHarness) {
        for _ in 0..4 {
            p.frame();
        }
    }

    #[test]
    fn offers_the_configured_shortcut_when_it_is_not_a_preset() {
        assert_eq!(shortcut_options("Ctrl+V").len(), 3);
        let options = shortcut_options("Ctrl+Alt+V");
        assert_eq!(options.len(), 4);
        assert_eq!(options[3].value, "Ctrl+Alt+V");
        assert_eq!(options[3].hint.as_deref(), Some("From the file"));
    }

    #[test]
    fn credits_cc_by_models() {
        assert!(needs_attribution("CC-BY-4.0") && needs_attribution("cc-by-sa") && !needs_attribution("MIT"));
    }

    #[test]
    fn export_writes_to_the_picked_file() {
        let mut p = page(json!({ "general": { "onboarded": true } }));
        file_dialog::answer_with(Some(PathBuf::from("C:\\backup\\flow-config.toml")));
        p.click_text("Export");
        settle(&mut p);
        assert!(p.requests().contains(&Request::ConfigExport("C:\\backup\\flow-config.toml".into())));
        assert!(p.h.shows("Settings exported. API keys were left out."));

        // A cancelled dialog sends nothing and clears the message.
        file_dialog::answer_with(None);
        p.click_text("Import");
        settle(&mut p);
        assert!(!p.requests().iter().any(|r| matches!(r, Request::ConfigImport(_))));
        assert!(!p.h.shows("Settings exported. API keys were left out."));
    }

    #[test]
    fn a_failed_import_says_why() {
        let mut p = page(json!({ "general": { "onboarded": true } }));
        p.backend.respond_with(|request| {
            matches!(request, Request::ConfigImport(_)).then(|| Reply::err("The file is not a Flow config"))
        });
        file_dialog::answer_with(Some(PathBuf::from("C:\\x.toml")));
        p.click_text("Import");
        settle(&mut p);
        assert!(p.requests().contains(&Request::ConfigImport("C:\\x.toml".into())));
        assert!(p.h.shows("The file is not a Flow config"));
    }

    #[test]
    fn checks_for_updates_and_shows_the_answer() {
        let mut p = page(json!({ "general": { "onboarded": true } }));
        p.backend.respond_with(|request| {
            matches!(request, Request::AppCheckUpdates).then(|| Reply::ok(&"Flow 0.1.0 is the latest version."))
        });
        assert!(p.h.shows("Flow checks GitHub Releases and installs signed updates only."));
        p.click_text("Check for updates");
        settle(&mut p);
        assert!(p.h.shows("Flow 0.1.0 is the latest version."));
        assert!(p.h.shows("Check for updates"), "the button is back once the answer is in");
    }

    #[test]
    fn opens_the_file_and_folders() {
        let mut p = page(json!({ "general": { "onboarded": true } }));
        for label in ["Open config file", "Open logs folder", "Open models folder"] {
            p.click_text(label);
        }
        let sent = p.requests();
        assert!(sent.contains(&Request::ConfigOpenFile));
        assert!(sent.contains(&Request::AppOpenLogs));
        assert!(sent.contains(&Request::AppOpenModels));
    }

    #[test]
    fn debug_logging_warns_while_on() {
        let mut p = page(json!({ "general": { "onboarded": true } }));
        let warning = "Logs now contain transcript text. Turn this off when you have finished, and check a log \
                       before sharing it.";
        assert!(!p.h.shows(warning));
        // The fourth On/Off label belongs to Debug logging (after restore clipboard, trailing space, match case).
        let off = p.h.text_rects("Off")[0];
        p.click(egui::pos2(off.right() + 12.0 + 21.0, off.center().y));
        settle(&mut p);
        assert!(p.config().general.debug_logging);
        assert!(p.h.shows(warning));
    }

    #[test]
    fn typing_disables_the_paste_settings() {
        let mut p = page(json!({ "general": { "onboarded": true }, "insert": { "method": "type" } }));
        // Restore clipboard is the first toggle; disabled, a click changes nothing.
        let on = p.h.text_rects("On")[0];
        p.click(egui::pos2(on.right() + 12.0 + 21.0, on.center().y));
        settle(&mut p);
        assert!(p.config().insert.restore_clipboard);
        assert!(!p.requests().iter().any(|r| matches!(r, Request::ConfigSet(_))));
    }
}
