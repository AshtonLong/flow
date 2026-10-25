//! The dictation test under the model choice (`TestDictation`): a box to
//! dictate into with the real hotkey, and a button that records a clip in the
//! window and transcribes it without the hotkey.

use std::sync::Arc;

use crossbeam_channel::{Receiver, TryRecvError};
use egui::{Align, Id, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use flow_core::protocol::Pcm;
use flow_core::protocol::settings::{Event, Request};
use flow_core::types::{DictationEvent, Transcript};

use crate::adapters::audio::{ClipRecording, MonitorState};
use crate::backend::Pending;
use crate::components::{Button, Notice, Tone, capture_help, input_area, key_caps, level_meter};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, tokens};
use crate::pages::Cx;
use crate::util::format::{error_message, format_duration};
use crate::util::models::model_label;

/// How long a dictation's text gets to land in the box before it is added from the event.
const LAND_SECONDS: f64 = 0.25;

/// The test recording (`recordClip`), from opening the microphone to the transcript.
#[derive(Default)]
enum Recording {
    #[default]
    Idle,
    /// The microphone is opening; the button still offers to record.
    Opening(ClipRecording),
    Live(ClipRecording),
    /// Flushing the last chunk (`working`).
    Stopping(Receiver<Vec<f32>>),
    /// `dictation.test` in flight (`working`).
    Transcribing(Pending<Transcript>),
}

/// A status line under the test: `{ ok, message }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub ok: bool,
    pub message: String,
}

impl Status {
    fn ok(message: String) -> Self {
        Self { ok: true, message }
    }
    fn failed(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into() }
    }
}

/// Keyed by the chosen model: a new choice starts over (`key={chosen.id}`).
pub struct TestDictation {
    model_id: String,
    text: String,
    status: Option<Status>,
    recording: Recording,
    /// The box takes focus when the test appears.
    focus: bool,
    /// Dictated text to add from the event if it did not land in the box: (due time, text).
    landing: Vec<(f64, String)>,
    /// Preview only: a dictation to show as if Flow had sent it.
    simulated: Option<DictationEvent>,
}

/// Adds `heard` after what is there, with a space unless the text already ends in one.
pub(super) fn append(text: &mut String, heard: &str) {
    if !text.is_empty() && !text.ends_with(char::is_whitespace) {
        text.push(' ');
    }
    text.push_str(heard);
}

impl TestDictation {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            text: String::new(),
            status: None,
            recording: Recording::Idle,
            focus: true,
            landing: Vec::new(),
            simulated: None,
        }
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Preview only (`--demo try`): handles a made-up dictation next frame.
    pub fn simulate(&mut self, text: &str) {
        self.simulated = Some(DictationEvent {
            id: 0,
            ok: true,
            text: text.to_string(),
            raw_text: text.to_string(),
            model: self.model_id.clone(),
            cloud: false,
            audio_ms: 2400.0,
            elapsed_ms: 640.0,
            outcome: None,
            fell_back: false,
            error: None,
        });
    }

    #[cfg(test)]
    pub fn text(&self) -> &str {
        &self.text
    }

    fn working(&self) -> bool {
        matches!(self.recording, Recording::Stopping(_) | Recording::Transcribing(_))
    }

    /// A real dictation is typed into the focused box by Flow itself. If the text
    /// did not land there (the box lost focus), show it from the event instead.
    fn on_dictation(&mut self, event: &DictationEvent, cx: &Cx<'_>, now: f64) {
        if !event.ok {
            self.status = Some(Status::failed(event.error.clone().unwrap_or_else(|| "That dictation failed.".into())));
            return;
        }
        self.status = Some(Status::ok(format!(
            "It works. {} took {}{}.",
            model_label(cx.catalog.snapshot(), &event.model),
            format_duration(event.elapsed_ms),
            if event.fell_back { " (the fallback model was used)" } else { "" },
        )));
        self.landing.push((now + LAND_SECONDS, event.text.trim().to_string()));
    }

    /// Moves the recording along: opened, flushed, transcribed.
    fn poll(&mut self, cx: &Cx<'_>) {
        match &mut self.recording {
            Recording::Opening(clip) => match clip.state() {
                MonitorState::Opening => {}
                MonitorState::Listening => {
                    if let Recording::Opening(clip) = std::mem::take(&mut self.recording) {
                        self.recording = Recording::Live(clip);
                    }
                }
                MonitorState::Failed(error) => {
                    self.recording = Recording::Idle;
                    self.status = Some(Status::failed(capture_help(&error)));
                }
            },
            Recording::Stopping(rx) => match rx.try_recv() {
                Ok(audio) => {
                    self.recording = Recording::Transcribing(cx.link.call(Request::DictationTest(Pcm(audio))));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.recording = Recording::Idle;
                    self.status = Some(Status::failed("The recording could not be finished."));
                }
            },
            Recording::Transcribing(pending) => {
                let Some(result) = pending.take() else {
                    return;
                };
                self.recording = Recording::Idle;
                self.status = Some(match result {
                    Ok(transcript) => {
                        let heard = transcript.text.trim();
                        if heard.is_empty() {
                            Status::failed("No speech was found in that recording. Try again a little louder.")
                        } else {
                            append(&mut self.text, heard);
                            Status::ok(format!(
                                "It works. {} took {}.",
                                model_label(cx.catalog.snapshot(), &transcript.model),
                                format_duration(transcript.elapsed_ms),
                            ))
                        }
                    }
                    Err(err) => Status::failed(error_message(&err)),
                });
            }
            Recording::Idle | Recording::Live(_) => {}
        }
    }

    fn record(&mut self, ui: &Ui, device: &str) {
        if !matches!(self.recording, Recording::Idle) {
            return;
        }
        self.status = None;
        let ctx = ui.ctx().clone();
        self.recording = Recording::Opening(ClipRecording::start(device, Arc::new(move || ctx.request_repaint())));
    }

    fn stop(&mut self) {
        if let Recording::Live(clip) = std::mem::take(&mut self.recording) {
            self.recording = Recording::Stopping(clip.stop());
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        let t = tokens(&ctx);
        let now = ctx.input(|i| i.time);
        for event in cx.events {
            if let Event::DictationEvent(event) = event {
                self.on_dictation(event, cx, now);
            }
        }
        if let Some(event) = self.simulated.take() {
            self.on_dictation(&event, cx, now);
        }
        let text = &mut self.text;
        self.landing.retain(|(due, heard)| {
            if now < *due {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(due - now));
                return true;
            }
            if !heard.is_empty() && !text.contains(heard.as_str()) {
                append(text, heard);
            }
            false
        });
        self.poll(cx);

        let config = cx.config.config();
        let (binding, device) = (config.hotkeys.hold_to_talk.clone(), config.audio.input_device.clone());
        let area_id = Id::unique(("flow-onboarding-try", &self.model_id));

        // `label.flex.flex-wrap.items-center.gap-x-2.font-semibold`, pointing at the box.
        let style = TextStyle::BODY.weight(Weight::Semibold);
        let width = ui.available_width();
        let row = ui
            .allocate_ui_with_layout(vec2(width, 29.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                text::label(ui, "Try it: click in the box, hold", style, t.fg);
                key_caps(ui, &binding, false);
                text::label(ui, "and speak", style, t.fg);
            })
            .response;
        let label_clicked = ui.interact(row.rect, area_id.with("label"), Sense::click()).clicked();
        ui.add_space(8.0);
        let area = input_area(
            ui,
            ("onboarding.try", &self.model_id),
            &mut self.text,
            3,
            Some("Your words appear here when you release the hotkey."),
        );
        if std::mem::take(&mut self.focus) || label_clicked {
            area.request_focus();
        }
        ui.add_space(8.0);

        // `mt-2 flex items-center gap-3`
        let recording = matches!(self.recording, Recording::Live(_));
        let working = self.working();
        let label = model_label(cx.catalog.snapshot(), &self.model_id);
        let mut record = false;
        let mut stop = false;
        let width = ui.available_width();
        ui.allocate_ui_with_layout(vec2(width, 32.0), Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if recording {
                stop = Button::new("Stop").icon(Icon::Square, 13.0).show(ui).clicked();
                let level = match &self.recording {
                    Recording::Live(clip) => clip.level(),
                    _ => 0.0,
                };
                // `flex-1`: the meter takes the rest of the row.
                let rest = ui.available_rect_before_wrap();
                let meter = Rect::from_min_max(
                    pos2(rest.left(), rest.center().y - 3.0),
                    pos2(rest.right(), rest.center().y + 3.0),
                );
                let mut child = ui.new_child(UiBuilder::new().max_rect(meter));
                level_meter(&mut child, level);
            } else {
                let caption = if working { "Transcribing…" } else { "Record a test instead" };
                record = Button::new(caption).icon(Icon::Mic, 15.0).disabled(working).show(ui).clicked();
                text::label(ui, format!("Uses {label} without the hotkey."), TextStyle::CAPTION, t.fg_2);
            }
        });
        if stop {
            self.stop();
        }
        if record {
            self.record(ui, &device);
        }

        if let Some(status) = &self.status {
            ui.add_space(12.0);
            Notice::new(if status.ok { Tone::Ok } else { Tone::Danger }, &status.message).show(ui);
        }
    }
}
