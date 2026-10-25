//! History: storage settings and the searchable list of past dictations, with
//! copy, re-run cleanup and delete on each entry. Port of `pages/History.tsx`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Galley, Layout, Rect, Shape, Ui, UiBuilder, pos2, vec2};
use flow_core::catalog::CatalogSnapshot;
use flow_core::path;
use flow_core::protocol::settings::{Event, Request};
use flow_core::types::HistoryEntry;

use super::{Cx, Page};
use crate::backend::{Link, Pending, take_settled};
use crate::components::button::spin_angle;
use crate::components::{
    Button, ConfirmDialog, Group, IconButton, Notice, NumberField, SettingRow, Toggle, Tone, empty_state, page_header,
    search_input,
};
use crate::design::text::{self, TextStyle};
use crate::design::{Icon, Weight, shadow, tokens};
use crate::stores::{Debounced, Flash};
use crate::util::format::{error_message, format_duration, format_when};
use crate::util::models::model_label;

const PAGE_SIZE: u32 = 40;

#[derive(Default)]
pub struct History {
    /// The list while history is on; dropped when it is turned off, as React
    /// unmounts `HistoryList`.
    list: Option<HistoryList>,
}

impl Page for History {
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let history = cx.config.config().history.clone();
        page_header(ui, "History", None);

        if !history.enabled {
            self.list = None;
            empty_state(
                ui,
                "History is off. Turn it on to keep a searchable list of what you dictate, stored only on this PC.",
                |ui| {
                    if Button::new("Turn on history").accent().show(ui).clicked() {
                        cx.config.set(path!["history", "enabled"], true);
                    }
                },
            );
            return;
        }

        Group::new("Storage").show(ui, |ui| {
            SettingRow::new(
                "Keep history",
                "Saves each transcript on this PC only. Audio is never saved. Turning this off stops new entries.",
            )
            .show(ui, |ui| {
                if let Some(v) = Toggle::new(history.enabled).show(ui).changed {
                    cx.config.set(path!["history", "enabled"], v);
                }
            });
            SettingRow::new("Delete entries after", "Older transcripts are removed automatically.").show(ui, |ui| {
                let field = NumberField::new("history.retention_days", history.retention_days, 1..=3650).unit("days");
                if let Some(v) = field.show(ui).committed {
                    cx.config.set(path!["history", "retention_days"], v);
                }
            });
        });

        self.list.get_or_insert_with(HistoryList::default).ui(ui, cx);
    }
}

/// One entry's own state (`Row` in the TSX), kept by entry id.
#[derive(Default)]
struct RowState {
    copied: Flash,
    copying: Option<Pending<()>>,
    rerunning: Option<Pending<Option<HistoryEntry>>>,
    deleting: Option<Pending<()>>,
}

impl RowState {
    fn idle(&self) -> bool {
        self.copying.is_none() && self.rerunning.is_none() && self.deleting.is_none()
    }
}

/// What a row's buttons asked for this frame.
enum RowAction {
    Copy,
    Rerun,
    Delete,
}

/// `HistoryList`: the search box, Clear all, the entries and Load more.
struct HistoryList {
    query: String,
    debounced: Debounced<String>,
    /// The search the list was last loaded for; a new debounced value reloads it.
    loaded_for: Option<String>,
    entries: Vec<HistoryEntry>,
    /// The search in flight and the offset it asked for. Only the newest one
    /// counts: starting another drops this one, so an older search finishing
    /// late is ignored.
    loading: Option<(Pending<Vec<HistoryEntry>>, usize)>,
    has_more: bool,
    error: Option<String>,
    confirm_clear: bool,
    clearing: Option<Pending<()>>,
    rows: HashMap<i64, RowState>,
}

impl Default for HistoryList {
    fn default() -> Self {
        Self {
            query: String::new(),
            debounced: Debounced::new(String::new(), Duration::from_millis(250)),
            loaded_for: None,
            entries: Vec::new(),
            loading: None,
            has_more: false,
            error: None,
            confirm_clear: false,
            clearing: None,
            rows: HashMap::new(),
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl HistoryList {
    fn load(&mut self, link: &Link, search: &str, offset: usize) {
        let request = Request::HistorySearch { query: search.to_string(), limit: PAGE_SIZE, offset: offset as u32 };
        self.loading = Some((link.call(request), offset));
    }

    /// Applies replies that arrived since the last frame.
    fn settle(&mut self) {
        if let Some((pending, offset)) = self.loading.as_mut()
            && let Some(result) = pending.take()
        {
            let offset = *offset;
            self.loading = None;
            match result {
                Ok(page) => {
                    self.has_more = page.len() == PAGE_SIZE as usize;
                    if offset == 0 {
                        self.entries = page;
                    } else {
                        self.entries.extend(page);
                    }
                    self.error = None;
                }
                Err(err) => self.error = Some(error_message(&err)),
            }
        }
        if let Some(result) = take_settled(&mut self.clearing) {
            match result {
                Ok(()) => {
                    self.entries.clear();
                    self.has_more = false;
                }
                Err(err) => self.error = Some(error_message(&err)),
            }
        }
        let mut changed = Vec::new();
        let mut deleted = Vec::new();
        for (&id, row) in &mut self.rows {
            if let Some(result) = take_settled(&mut row.copying) {
                match result {
                    Ok(()) => row.copied.flash(),
                    Err(err) => self.error = Some(error_message(&err)),
                }
            }
            if let Some(result) = take_settled(&mut row.rerunning) {
                match result {
                    Ok(Some(next)) => changed.push(next),
                    Ok(None) => {}
                    Err(err) => self.error = Some(error_message(&err)),
                }
            }
            if let Some(result) = take_settled(&mut row.deleting) {
                match result {
                    Ok(()) => deleted.push(id),
                    Err(err) => self.error = Some(error_message(&err)),
                }
            }
        }
        for next in changed {
            if let Some(entry) = self.entries.iter_mut().find(|e| e.id == next.id) {
                *entry = next;
            }
        }
        self.entries.retain(|e| !deleted.contains(&e.id));
    }

    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
        let ctx = ui.ctx().clone();
        self.settle();

        let debounced = self.debounced.update(&ctx, &self.query.trim().to_string()).clone();
        if self.loaded_for.as_ref() != Some(&debounced) {
            self.loaded_for = Some(debounced.clone());
            self.load(cx.link, &debounced, 0);
        }
        // A dictation made while the page is open appears at the top.
        if cx.events.iter().any(|e| matches!(e, Event::DictationEvent(d) if d.ok)) {
            self.load(cx.link, &debounced, 0);
        }
        let loading = self.loading.is_some();

        // `mb-2 flex items-center gap-2`: the search box takes what Clear all leaves.
        let width = ui.available_width();
        let clear_w = text::width(ui, "Clear all", TextStyle::BODY.weight(Weight::Strong)) + 24.0;
        let row = Rect::from_min_size(ui.cursor().min, vec2(width, 32.0));
        let mut row_ui = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)));
        row_ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        search_input(
            &mut row_ui,
            "history.search",
            &mut self.query,
            width - 8.0 - clear_w,
            Some("Search past dictations"),
        );
        let nothing = self.entries.is_empty() && debounced.is_empty();
        if Button::new("Clear all").disabled(nothing).show(&mut row_ui).clicked() {
            self.confirm_clear = true;
        }
        ui.advance_cursor_after_rect(row);
        ui.add_space(8.0);

        if let Some(error) = &self.error {
            Notice::new(Tone::Danger, error).show(ui);
            ui.add_space(8.0);
        }

        if self.entries.is_empty() && !loading {
            let sentence = if debounced.is_empty() {
                "Nothing here yet. Your next dictation will appear in this list.".to_string()
            } else {
                format!("Nothing in your history matches “{debounced}”.")
            };
            empty_state(ui, &sentence, |_| {});
        }

        // `flex flex-col gap-[3px]`
        let snapshot = cx.catalog.snapshot();
        let now = now_ms();
        let mut actions = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                ui.add_space(3.0);
            }
            let row = self.rows.entry(entry.id).or_default();
            if let Some(action) = entry_row(ui, entry, row, snapshot, now) {
                actions.push((entry.clone(), action));
            }
        }
        for (entry, action) in actions {
            let row = self.rows.entry(entry.id).or_default();
            match action {
                RowAction::Copy => row.copying = Some(cx.link.call(Request::AppCopy(entry.text))),
                RowAction::Rerun => row.rerunning = Some(cx.link.call(Request::HistoryRerun(entry.id))),
                RowAction::Delete => row.deleting = Some(cx.link.call(Request::HistoryDelete(entry.id))),
            }
        }
        // Forget rows that are gone and have nothing left to report.
        let shown: Vec<i64> = self.entries.iter().map(|e| e.id).collect();
        self.rows.retain(|id, row| shown.contains(id) || !row.idle());

        if self.has_more {
            // `mt-3 flex justify-center`
            ui.add_space(12.0);
            let label = if loading { "Loading…" } else { "Load more" };
            let button_w = text::width(ui, label, TextStyle::BODY.weight(Weight::Strong)) + 24.0;
            let at = Rect::from_min_size(ui.cursor().min + vec2((width - button_w) / 2.0, 0.0), vec2(button_w, 32.0));
            let mut button_ui = ui.new_child(UiBuilder::new().max_rect(at).layout(Layout::left_to_right(Align::Min)));
            if Button::new(label).disabled(loading).show(&mut button_ui).clicked() {
                let offset = self.entries.len();
                self.load(cx.link, &debounced, offset);
            }
            ui.advance_cursor_after_rect(Rect::from_min_size(ui.cursor().min, vec2(width, 32.0)));
        }

        if ConfirmDialog::new(
            "history.clear",
            "Clear all history?",
            "Every saved transcript is deleted from this PC. This cannot be undone.",
            "Clear history",
        )
        .show(&ctx, &mut self.confirm_clear)
        {
            self.clearing = Some(cx.link.call(Request::HistoryClear));
        }
        // `<section className="mb-8">`
        ui.add_space(32.0);
    }
}

/// Lays out `text` wrapped at `width`, breaking long words (`break-words`).
fn wrapped(ui: &Ui, text: &str, style: TextStyle, color: egui::Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_string(), style.format(color));
    job.wrap = TextWrapping { max_width: width, ..TextWrapping::default() };
    text::snap_baselines(ui.painter().layout_job(job), style)
}

/// One entry: `card flex items-start gap-3 py-3 pr-2 pl-4`, the selectable
/// text and its details on the left, Copy / Re-run / Delete on the right.
fn entry_row(
    ui: &mut Ui,
    entry: &HistoryEntry,
    row: &mut RowState,
    snapshot: Option<&CatalogSnapshot>,
    now: i64,
) -> Option<RowAction> {
    let ctx = ui.ctx().clone();
    let t = tokens(&ctx);
    let slot = ui.painter().add(Shape::Noop);
    let width = ui.available_width();
    let top = ui.cursor().min;
    let buttons_w = 3.0 * 32.0;
    let left = top.x + 16.0;
    let right = top.x + width - 8.0;
    let text_w = (right - buttons_w - 12.0 - left).max(40.0);

    // The transcript (`selectable whitespace-pre-wrap break-words`).
    let galley = wrapped(ui, &entry.text, TextStyle::BODY, t.fg, text_w);
    let text_h = galley.size().y;
    let mut text_ui = ui.new_child(
        UiBuilder::new()
            .id_salt(("history-entry", entry.id))
            .max_rect(Rect::from_min_size(pos2(left, top.y + 12.0), vec2(text_w, text_h)))
            .layout(Layout::top_down(Align::Min)),
    );
    text_ui.style_mut().interaction.selectable_labels = true;
    text_ui.add(egui::Label::new(galley).selectable(true));

    // The details (`dl mt-1.5 flex flex-wrap gap-x-4 gap-y-0.5 text-caption text-fg-2`).
    let mut details = vec![format_when(entry.created_at, now)];
    if !entry.app.is_empty() {
        details.push(entry.app.clone());
    }
    details.push(model_label(snapshot, &entry.model));
    details.push(format!("{} of audio", format_duration(entry.audio_ms as f64)));
    let (mut x, mut y) = (left, top.y + 12.0 + text_h + 6.0);
    for detail in details {
        let galley = text::galley(ui, detail, TextStyle::CAPTION, t.fg_2, text_w);
        if x > left && x + galley.size().x > left + text_w {
            x = left;
            y += 16.0 + 2.0;
        }
        let w = galley.size().x;
        ui.painter().galley(pos2(x, y), galley, t.fg_2);
        x += w + 16.0;
    }
    let text_bottom = y + 16.0;

    // `flex shrink-0 items-center`: three 32 px icon buttons.
    let buttons = Rect::from_min_size(pos2(right - buttons_w, top.y + 12.0), vec2(buttons_w, 32.0));
    let mut buttons_ui = ui.new_child(
        UiBuilder::new()
            .id_salt(("history-buttons", entry.id))
            .max_rect(buttons)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let mut action = None;
    let copied = row.copied.on(&ctx);
    let (copy_label, copy_icon) = if copied { ("Copied", Icon::Check) } else { ("Copy text", Icon::Copy) };
    if IconButton::new(copy_label, copy_icon, 15.0).show(&mut buttons_ui).clicked() {
        action = Some(RowAction::Copy);
    }
    let rerunning = row.rerunning.is_some();
    let rerun = IconButton::new("Re-run cleanup with the current settings", Icon::RefreshCw, 15.0)
        .rotation(if rerunning { spin_angle(ui) } else { 0.0 })
        .disabled(rerunning);
    if rerun.show(&mut buttons_ui).clicked() {
        action = Some(RowAction::Rerun);
    }
    if IconButton::new("Delete", Icon::Trash2, 15.0).show(&mut buttons_ui).clicked() {
        action = Some(RowAction::Delete);
    }

    let bottom = text_bottom.max(buttons.bottom()) + 12.0;
    let rect = Rect::from_min_max(top, pos2(top.x + width, bottom));
    ui.painter().set(slot, shadow::surface(rect, 10.0, t.card, &t.shadow_card, t.hc_border));
    ui.advance_cursor_after_rect(rect);
    action
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use flow_core::protocol::settings::{Event, Reply, Request};
    use flow_core::types::{DictationEvent, HistoryEntry};
    use parking_lot::Mutex;
    use serde_json::json;

    use crate::pages::PageId;
    use crate::testing::PageHarness;

    fn entry(id: i64, text: &str) -> HistoryEntry {
        HistoryEntry {
            id,
            created_at: 0,
            text: text.into(),
            raw_text: text.into(),
            model: "parakeet-tdt-0.6b-v2".into(),
            app: "Code.exe".into(),
            audio_ms: 1800,
            elapsed_ms: 400,
        }
    }

    /// A History page whose backend serves `store` and answers re-runs by upper-casing.
    fn page(store: Arc<Mutex<Vec<HistoryEntry>>>) -> PageHarness {
        let mut p = PageHarness::new(
            PageId::History,
            json!({ "general": { "onboarded": true }, "history": { "enabled": true, "retention_days": 30 } }),
        );
        p.h.screen = egui::vec2(1000.0, 5000.0);
        p.backend.respond_with(move |request| match request {
            Request::HistorySearch { query, limit, offset } => {
                let all = store.lock();
                let page: Vec<HistoryEntry> = all
                    .iter()
                    .filter(|e| e.text.to_lowercase().contains(&query.to_lowercase()))
                    .skip(*offset as usize)
                    .take(*limit as usize)
                    .cloned()
                    .collect();
                Some(Reply::ok(&page))
            }
            Request::HistoryDelete(id) => {
                store.lock().retain(|e| e.id != *id);
                Some(Reply::ok(&()))
            }
            Request::HistoryClear => {
                store.lock().clear();
                Some(Reply::ok(&()))
            }
            Request::HistoryRerun(id) => {
                let mut all = store.lock();
                let found = all.iter_mut().find(|e| e.id == *id).map(|e| {
                    e.text = e.text.to_uppercase();
                    e.clone()
                });
                Some(Reply::ok(&found))
            }
            _ => None,
        });
        p
    }

    fn settle(p: &mut PageHarness) {
        for _ in 0..4 {
            p.frame();
        }
    }

    fn searches(p: &PageHarness) -> Vec<(String, u32)> {
        p.requests()
            .into_iter()
            .filter_map(|r| match r {
                Request::HistorySearch { query, offset, .. } => Some((query, offset)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn history_off_offers_to_turn_it_on() {
        let mut p = PageHarness::new(PageId::History, json!({ "general": { "onboarded": true } }));
        settle(&mut p);
        assert!(p.h.shows(
            "History is off. Turn it on to keep a searchable list of what you dictate, stored only on this PC."
        ));
        assert!(searches(&p).is_empty(), "nothing is searched while history is off");
        p.click_text("Turn on history");
        assert!(p.config().history.enabled);
        settle(&mut p);
        assert!(p.h.shows("Keep history"));
        assert_eq!(searches(&p), vec![(String::new(), 0)]);
    }

    #[test]
    fn lists_entries_and_deletes_one() {
        let store = Arc::new(Mutex::new(vec![entry(2, "Second entry"), entry(1, "First entry")]));
        let mut p = page(store.clone());
        settle(&mut p);
        assert!(p.h.shows("Second entry") && p.h.shows("First entry"));
        assert!(p.h.shows("1.8 s of audio"));
        // Delete is the last of the three 32 px buttons, 8 px from the card's right edge.
        let text = p.h.text_rect("Second entry").unwrap();
        p.click(egui::pos2(696.0 - 8.0 - 16.0, text.top() + 16.0));
        settle(&mut p);
        assert!(p.requests().contains(&Request::HistoryDelete(2)));
        assert!(!p.h.shows("Second entry"));
        assert!(p.h.shows("First entry"));
    }

    #[test]
    fn copy_and_rerun_reach_the_backend() {
        let store = Arc::new(Mutex::new(vec![entry(7, "Hello there")]));
        let mut p = page(store);
        settle(&mut p);
        let text = p.h.text_rect("Hello there").unwrap();
        let right = 696.0 - 8.0;
        p.click(egui::pos2(right - 80.0, text.top() + 16.0));
        settle(&mut p);
        assert!(p.requests().contains(&Request::AppCopy("Hello there".into())));
        p.click(egui::pos2(right - 48.0, text.top() + 16.0));
        settle(&mut p);
        assert!(p.requests().contains(&Request::HistoryRerun(7)));
        assert!(p.h.shows("HELLO THERE"), "the re-run entry replaces the old one");
    }

    #[test]
    fn clear_all_asks_first() {
        let store = Arc::new(Mutex::new(vec![entry(1, "Keep me")]));
        let mut p = page(store);
        settle(&mut p);
        p.click_text("Clear all");
        settle(&mut p);
        assert!(p.h.shows("Clear all history?"));
        p.click_text("Cancel");
        settle(&mut p);
        assert!(!p.h.shows("Clear all history?"));
        assert!(!p.requests().contains(&Request::HistoryClear), "Cancel backs out");
        assert!(p.h.shows("Keep me"));

        p.click_text("Clear all");
        settle(&mut p);
        p.click_text("Clear history");
        settle(&mut p);
        assert!(p.requests().contains(&Request::HistoryClear));
        assert!(!p.h.shows("Keep me"));
        assert!(p.h.shows("Nothing here yet. Your next dictation will appear in this list."));
    }

    #[test]
    fn search_waits_for_a_pause_in_typing() {
        let store = Arc::new(Mutex::new(vec![entry(2, "Coffee filters"), entry(1, "Oat milk")]));
        let mut p = page(store);
        settle(&mut p);
        p.click_text("Search past dictations");
        p.h.type_text("oat");
        settle(&mut p);
        assert_eq!(searches(&p), vec![(String::new(), 0)], "no search while typing");
        std::thread::sleep(std::time::Duration::from_millis(300));
        settle(&mut p);
        assert_eq!(searches(&p).last(), Some(&("oat".to_string(), 0)));
        assert!(p.h.shows("Oat milk") && !p.h.shows("Coffee filters"));

        p.h.type_text("zzz");
        settle(&mut p);
        std::thread::sleep(std::time::Duration::from_millis(300));
        settle(&mut p);
        assert!(p.h.shows("Nothing in your history matches “oatzzz”."));
        // Escape clears the search box, as in a browser.
        p.h.key(egui::Key::Escape);
        settle(&mut p);
        std::thread::sleep(std::time::Duration::from_millis(300));
        settle(&mut p);
        assert_eq!(searches(&p).last(), Some(&(String::new(), 0)));
    }

    #[test]
    fn loads_more_in_pages_of_forty() {
        let store = Arc::new(Mutex::new((1..=45).rev().map(|i| entry(i, &format!("Entry {i}"))).collect()));
        let mut p = page(store);
        settle(&mut p);
        assert!(p.h.shows("Load more"));
        assert!(!p.h.shows("Entry 5"));
        p.click_text("Load more");
        settle(&mut p);
        assert_eq!(searches(&p).last(), Some(&(String::new(), 40)));
        assert!(p.h.shows("Entry 5") && p.h.shows("Entry 1"));
        assert!(!p.h.shows("Load more"), "a short page means there is no more");
    }

    #[test]
    fn a_new_dictation_reloads_the_list() {
        let store = Arc::new(Mutex::new(vec![entry(1, "Old")]));
        let mut p = page(store.clone());
        settle(&mut p);
        store.lock().insert(0, entry(2, "Fresh"));
        p.events.push(Event::DictationEvent(DictationEvent { ok: false, ..DictationEvent::default() }));
        settle(&mut p);
        assert!(!p.h.shows("Fresh"), "failed dictations are not listed");
        p.events.push(Event::DictationEvent(DictationEvent { ok: true, ..DictationEvent::default() }));
        settle(&mut p);
        assert!(p.h.shows("Fresh"));
    }
}
