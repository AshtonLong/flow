//! Test bench: record one phrase, run it through any set of ready models, and
//! compare text, latency and cost side by side. Port of
//! `pages/models/TestBench.tsx`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Id, Margin, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use flow_core::catalog::{ModelEntry, cloud_cost};
use flow_core::protocol::Pcm;
use flow_core::protocol::settings::{BenchResult, Event, Request};
use flow_core::types::SAMPLE_RATE;

use super::{Cross, flex_row, live_region, padded, rule, spinner_icon};
use crate::adapters::audio::{CaptureError, ClipRecording, MonitorState};
use crate::backend::Pending;
use crate::components::{Button, Group, Notice, Tone, capture_help, card, checkbox, level_meter};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, tokens};
use crate::pages::Cx;
use crate::util::format::{error_message, format_cost, format_duration, format_ms};
use crate::util::models::{model_label, ready_models};

const MAX_CLIP_MS: f64 = 60_000.0;
const MIN_CLIP_MS: f64 = 400.0;

fn cost_of(entry: Option<&ModelEntry>, result: &BenchResult, audio_ms: f64) -> String {
    match entry {
        None | Some(ModelEntry::Local(_)) => "Free".into(),
        Some(ModelEntry::Cloud(cloud)) => format_cost(result.cost.unwrap_or_else(|| cloud_cost(cloud, audio_ms))),
    }
}

/// The test bench's state (`TestBench`'s `useState`s and refs).
#[derive(Default)]
pub struct TestBench {
    /// The microphone being opened or recording.
    recorder: Option<ClipRecording>,
    /// The clip after Stop, once its last chunk is flushed.
    stopping: Option<crossbeam_channel::Receiver<Vec<f32>>>,
    /// When recording started; `Some` while recording.
    started_at: Option<Instant>,
    elapsed_ms: f64,
    clip: Option<Vec<f32>>,
    mic_error: Option<CaptureError>,
    /// The user's ticks; `None` until they change one.
    selected: Option<HashSet<String>>,
    order: Vec<String>,
    results: HashMap<String, BenchResult>,
    running: bool,
    run_error: Option<String>,
    starting: Option<Pending<u64>>,
    /// Results can arrive before `BenchRun` answers with the run id, so they
    /// are held until the id is known. Anything from another run is dropped.
    run_id: Option<u64>,
    early: Vec<BenchResult>,
}

impl TestBench {
    /// A recorded clip, as if Record and Stop had been pressed (tests).
    #[cfg(test)]
    pub(crate) fn set_clip(&mut self, clip: Vec<f32>) {
        self.clip = Some(clip);
    }

    fn recording(&self) -> bool {
        self.started_at.is_some()
    }

    fn apply(&mut self, result: BenchResult) {
        if result.done {
            self.running = false;
        }
        self.results.insert(result.model_id.clone(), result);
    }

    /// Bench results and the run id.
    fn pump(&mut self, events: &[Event]) {
        for event in events {
            let Event::BenchResult(result) = event else {
                continue;
            };
            match self.run_id {
                None if self.starting.is_some() => self.early.push(result.clone()),
                Some(id) if result.run_id == id => self.apply(result.clone()),
                _ => {}
            }
        }
        if let Some(reply) = self.starting.as_mut().and_then(Pending::take) {
            self.starting = None;
            match reply {
                Ok(id) => {
                    self.run_id = Some(id);
                    for result in std::mem::take(&mut self.early) {
                        if result.run_id == id {
                            self.apply(result);
                        }
                    }
                }
                Err(err) => {
                    self.early.clear();
                    self.running = false;
                    self.run_error = Some(error_message(&err));
                }
            }
        }
    }

    /// The microphone: opening, recording, the 60 s limit.
    fn pump_recorder(&mut self, ui: &Ui) {
        if let Some(stopping) = &self.stopping
            && let Ok(clip) = stopping.try_recv()
        {
            self.stopping = None;
            self.clip = Some(clip);
        }
        let Some(recorder) = &self.recorder else {
            return;
        };
        match recorder.state() {
            MonitorState::Opening => {}
            MonitorState::Failed(error) if !self.recording() => {
                self.recorder = None;
                self.mic_error = Some(error);
            }
            // A device lost while recording keeps what was recorded until Stop, like the TSX.
            MonitorState::Failed(_) | MonitorState::Listening => {
                if !self.recording() {
                    self.started_at = Some(Instant::now());
                    self.elapsed_ms = 0.0;
                    self.clip = None;
                    self.results.clear();
                    self.order.clear();
                }
                self.elapsed_ms = self.started_at.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0);
                if self.elapsed_ms >= MAX_CLIP_MS {
                    self.stop();
                } else {
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                }
            }
        }
    }

    fn record(&mut self, ui: &Ui, device: &str) {
        self.mic_error = None;
        self.run_error = None;
        let ctx = ui.ctx().clone();
        self.recorder = Some(ClipRecording::start(device, Arc::new(move || ctx.request_repaint())));
    }

    fn stop(&mut self) {
        let Some(recorder) = self.recorder.take() else {
            return;
        };
        self.started_at = None;
        self.stopping = Some(recorder.stop());
    }

    fn clip_ms(&self) -> f64 {
        self.clip.as_ref().map_or(0.0, |c| c.len() as f64 / SAMPLE_RATE as f64 * 1000.0)
    }

    fn run(&mut self, cx: &mut Cx<'_>, ids: Vec<String>) {
        let Some(clip) = &self.clip else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        self.order = ids.clone();
        self.results.clear();
        self.run_error = None;
        self.running = true;
        self.run_id = None;
        self.early.clear();
        self.starting = Some(cx.link.call(Request::BenchRun { audio: Pcm(clip.clone()), model_ids: ids }));
        self.pump(&[]);
    }

    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let t = tokens(ui.ctx());
        self.pump(cx.events);
        self.pump_recorder(ui);
        let snapshot = cx.catalog.snapshot().cloned();
        let ready = snapshot.as_ref().map(ready_models).unwrap_or_default();
        // Local models and the active model are ticked to begin with. Cloud models
        // are opt-in, because testing one sends the recording to its provider.
        let chosen: HashSet<String> = self.selected.clone().unwrap_or_else(|| {
            ready
                .iter()
                .filter(|m| {
                    !m.is_cloud() || snapshot.as_ref().and_then(|s| s.status.get(m.id())).is_some_and(|s| s.active)
                })
                .map(|m| m.id().to_string())
                .collect()
        });
        let picked: Vec<String> =
            ready.iter().filter(|m| chosen.contains(m.id())).map(|m| m.id().to_string()).collect();
        let recording = self.recording();
        let clip_ms = self.clip_ms();
        let too_short = self.clip.is_some() && clip_ms < MIN_CLIP_MS;
        let device = cx.config.config().audio.input_device.clone();

        Group::new("Test bench")
            .description("Record one phrase and run it through several models to compare them on your own voice.")
            .show(ui, |ui| {
                card(ui, Margin::ZERO, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
                    // 1. Record a phrase
                    let status = if recording {
                        format!("Recording… {}", format_duration(self.elapsed_ms))
                    } else if self.clip.is_some() {
                        if too_short {
                            "That was too short. Record a full sentence.".into()
                        } else {
                            format!(
                                "Recorded {} of audio. It stays in memory and is never saved.",
                                format_duration(clip_ms)
                            )
                        }
                    } else {
                        "A sentence or two, the way you normally dictate.".into()
                    };
                    let level = self.recorder.as_ref().map_or(0.0, ClipRecording::level);
                    let has_clip = self.clip.is_some();
                    let running = self.running;
                    let (record, stop) = padded(ui, 16.0, |ui| {
                        ui.add_space(12.0);
                        let out = flex_row(
                            ui,
                            Id::unique("flow-bench-record"),
                            16.0,
                            Cross::Center(0.0),
                            16.0,
                            |ui| step(ui, "1. Record a phrase", &status),
                            |ui| {
                                if recording {
                                    let (rect, _) = ui.allocate_exact_size(vec2(160.0, 6.0), Sense::hover());
                                    level_meter(&mut ui.new_child(UiBuilder::new().max_rect(rect)), level);
                                    let stop = Button::new("Stop").accent().icon(Icon::Square, 13.0).show(ui).clicked();
                                    (false, stop)
                                } else {
                                    let label = if has_clip { "Record again" } else { "Record" };
                                    let button = Button::new(label).icon(Icon::Mic, 15.0).disabled(running);
                                    (button.show(ui).clicked(), false)
                                }
                            },
                        );
                        ui.add_space(12.0);
                        out.1
                    });
                    if let Some(error) = &self.mic_error {
                        let help = capture_help(error);
                        padded(ui, 16.0, |ui| {
                            Notice::new(Tone::Danger, &help).show(ui);
                            ui.add_space(12.0);
                        });
                    }
                    if record && self.recorder.is_none() {
                        self.record(ui, &device);
                    }
                    if stop {
                        self.stop();
                    }

                    // 2. Choose models
                    rule(ui);
                    let mut toggled = None;
                    padded(ui, 16.0, |ui| {
                        ui.add_space(12.0);
                        step(
                            ui,
                            "2. Choose models",
                            "Installed and connected models. Ticking a cloud model sends the recording to its provider.",
                        );
                        if ready.is_empty() {
                            ui.add_space(8.0);
                            text::paragraph(ui, "Download or connect a model above first.", TextStyle::BODY, t.fg_2);
                        } else {
                            ui.add_space(10.0);
                            // `grid grid-cols-2 gap-x-6 gap-y-2`
                            let width = ui.available_width();
                            let col_w = ((width - 24.0) / 2.0).max(0.0);
                            let top = ui.cursor().min;
                            for (i, entry) in ready.iter().enumerate() {
                                let (row, col) = (i / 2, i % 2);
                                let cell = Rect::from_min_size(
                                    pos2(top.x + col as f32 * (col_w + 24.0), top.y + row as f32 * 28.0),
                                    vec2(col_w, 20.0),
                                );
                                let mut child = ui.new_child(UiBuilder::new().max_rect(cell));
                                let on = chosen.contains(entry.id());
                                let (_, changed) = checkbox(&mut child, on, running);
                                let label_w = col_w - 30.0;
                                let label = model_label(snapshot.as_ref(), entry.id());
                                let color = t.fg;
                                let galley = text::galley_truncated(ui, label, TextStyle::BODY, color, label_w);
                                let label_rect = Rect::from_min_size(pos2(cell.left() + 30.0, cell.top()), galley.size());
                                ui.painter().galley(label_rect.min, galley, color);
                                // `<label htmlFor>`: the name toggles the box too.
                                let label_click = !running
                                    && ui
                                        .interact(label_rect, Id::unique(("flow-bench-label", entry.id())), Sense::click())
                                        .clicked();
                                if let Some(next) = changed {
                                    toggled = Some((entry.id().to_string(), next));
                                } else if label_click {
                                    toggled = Some((entry.id().to_string(), !on));
                                }
                            }
                            let rows = ready.len().div_ceil(2) as f32;
                            ui.advance_cursor_after_rect(Rect::from_min_size(
                                top,
                                vec2(width, rows * 20.0 + (rows - 1.0) * 8.0),
                            ));
                        }
                        ui.add_space(12.0);
                    });
                    if let Some((id, on)) = toggled {
                        let mut next = chosen.clone();
                        if on {
                            next.insert(id);
                        } else {
                            next.remove(&id);
                        }
                        self.selected = Some(next);
                    }

                    // 3. Compare
                    rule(ui);
                    let hint = if self.clip.is_none() || too_short {
                        "Record a phrase first.".to_string()
                    } else if picked.is_empty() {
                        "Tick at least one model.".to_string()
                    } else if picked.len() == 1 {
                        "Runs the recording through the ticked model.".to_string()
                    } else {
                        format!("Runs the recording through {} models.", picked.len())
                    };
                    let disabled = self.clip.is_none() || too_short || picked.is_empty() || running || recording;
                    let run = padded(ui, 16.0, |ui| {
                        ui.add_space(12.0);
                        let (_, run) = flex_row(
                            ui,
                            Id::unique("flow-bench-compare"),
                            16.0,
                            Cross::Center(0.0),
                            8.0,
                            |ui| step(ui, "3. Compare", &hint),
                            |ui| {
                                let mut button =
                                    Button::new(if running { "Running…" } else { "Run test" }).accent().disabled(disabled);
                                if running {
                                    button = button.icon_spec(spinner_icon(ui));
                                }
                                button.show(ui).clicked()
                            },
                        );
                        ui.add_space(12.0);
                        run
                    });
                    if let Some(error) = &self.run_error {
                        let message = format!("The test could not start: {error}");
                        padded(ui, 16.0, |ui| {
                            Notice::new(Tone::Danger, &message).show(ui);
                            ui.add_space(12.0);
                        });
                    }
                    let finished = self.order.iter().filter(|id| self.results.contains_key(*id)).count();
                    let progress = if self.order.is_empty() {
                        String::new()
                    } else if self.running {
                        format!("{finished} of {} results in.", self.order.len())
                    } else {
                        format!("Test finished. {finished} of {} results.", self.order.len())
                    };
                    live_region(ui, Id::unique("flow-bench-progress"), &progress);
                    if !self.order.is_empty() {
                        rule(ui);
                        self.results_table(ui, snapshot.as_ref(), &ready, clip_ms);
                    }
                    if run {
                        self.run(cx, picked.clone());
                    }
                });
            });
    }

    /// The results table: Model (190), Text, Latency (88), Cost (84).
    fn results_table(
        &self,
        ui: &mut Ui,
        snapshot: Option<&flow_core::catalog::CatalogSnapshot>,
        ready: &[ModelEntry],
        clip_ms: f64,
    ) {
        let t = tokens(ui.ctx());
        let width = ui.available_width();
        let left = ui.cursor().min.x;
        let text_w = (width - 190.0 - 88.0 - 84.0).max(40.0);
        // (x, content width, right-aligned) per column, padding included in the widths.
        let cols = [
            (left + 16.0, 190.0 - 32.0, false),
            (left + 190.0 + 8.0, text_w - 16.0, false),
            (left + 190.0 + text_w + 8.0, 88.0 - 16.0, true),
            (left + 190.0 + text_w + 88.0 + 8.0, 84.0 - 24.0, true),
        ];
        let cell = |ui: &Ui, col: usize, top: f32, text: &str, style: TextStyle, color| {
            let (x, w, right) = cols[col];
            let galley = text::galley(ui, text, style, color, w);
            let pos = if right { pos2(x + w - galley.size().x, top) } else { pos2(x, top) };
            let h = galley.size().y;
            ui.painter().galley(pos, galley, color);
            h
        };
        // Header: `text-caption text-fg-2 py-2`.
        let top = ui.cursor().min.y;
        for (i, name) in ["Model", "Text", "Latency", "Cost"].iter().enumerate() {
            cell(ui, i, top + 8.0, name, TextStyle::CAPTION, t.fg_2);
        }
        ui.advance_cursor_after_rect(Rect::from_min_size(pos2(left, top), vec2(width, 32.0)));
        for id in &self.order {
            rule(ui);
            let result = self.results.get(id);
            let entry = ready.iter().find(|m| m.id() == id);
            let top = ui.cursor().min.y + 10.0;
            let mut h: f32 = cell(ui, 0, top, &model_label(snapshot, id), TextStyle::BODY, t.fg);
            let (text, color) = match result {
                None => ("Waiting…".to_string(), t.fg_3),
                Some(r) if r.ok => match r.text.as_deref().filter(|s| !s.is_empty()) {
                    Some(text) => (text.to_string(), t.fg),
                    None => ("No speech found".to_string(), t.fg_3),
                },
                Some(r) => (r.error.clone().unwrap_or_else(|| "Failed".into()), t.danger),
            };
            // `selectable`: the transcript can be copied.
            let (x, w, _) = cols[1];
            let galley = text::galley(ui, text, TextStyle::BODY, color, w);
            h = h.max(galley.size().y);
            let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(x, top), galley.size())));
            child.style_mut().interaction.selectable_labels = true;
            child.add(egui::Label::new(galley).selectable(true));
            if let Some(ms) = result.and_then(|r| r.elapsed_ms) {
                h = h.max(cell(ui, 2, top, &format_ms(ms), TextStyle::BODY, t.fg));
            }
            if let Some(r) = result.filter(|r| r.ok) {
                h = h.max(cell(ui, 3, top, &cost_of(entry, r, clip_ms), TextStyle::BODY, t.fg));
            }
            ui.advance_cursor_after_rect(Rect::from_min_size(pos2(left, top - 10.0), vec2(width, h + 20.0)));
        }
    }
}

/// A numbered step: its title over a caption (`mt-0.5 text-caption text-fg-2`).
fn step(ui: &mut Ui, title: &str, caption: &str) {
    let t = tokens(ui.ctx());
    text::paragraph(ui, title, TextStyle::BODY, t.fg);
    ui.add_space(2.0);
    text::paragraph(ui, caption, TextStyle::CAPTION, t.fg_2);
}
