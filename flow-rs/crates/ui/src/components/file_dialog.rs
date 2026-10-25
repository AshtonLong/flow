//! Native open and save dialogs. In Electron the main process showed them
//! (`dialog.showSaveDialog(win, …)`); here the window picks the file itself
//! and hands the path to the main process (`ConfigExport(path)`). The dialogs
//! are owned by the settings window, so they are modal to it as Electron's
//! were, and run on their own thread so the window keeps drawing. The choice
//! arrives as a [`Pending`]: `None` when the dialog was cancelled.
//!
//! ```ignore
//! self.picking = Some(file_dialog::save(ui.ctx(), "Export Flow settings", "flow-config.toml", TOML));
//! ```

use std::path::PathBuf;

use egui::{Context, Id};

use crate::backend::Pending;

/// A filter: its name and extensions, `("TOML", &["toml"])`.
pub type Filter<'a> = (&'a str, &'a [&'a str]);

fn owner_id() -> Id {
    Id::unique("flow-file-dialog-owner")
}

/// Makes the window the owner of every dialog. The shell calls this once.
pub(crate) fn set_owner(ctx: &Context, window: &eframe::Frame) {
    let template = rfd::FileDialog::new().set_parent(window);
    ctx.data_mut(|d| d.insert_temp(owner_id(), template));
}

fn template(ctx: &Context, title: &str, filter: Filter<'_>) -> rfd::FileDialog {
    let owned = ctx.data(|d| d.get_temp::<rfd::FileDialog>(owner_id())).unwrap_or_default();
    owned.set_title(title).add_filter(filter.0, filter.1)
}

/// "Save as", suggesting `file_name`.
pub fn save(ctx: &Context, title: &str, file_name: &str, filter: Filter<'_>) -> Pending<Option<PathBuf>> {
    run(ctx, template(ctx, title, filter).set_file_name(file_name), true)
}

/// "Open" for one existing file.
pub fn open(ctx: &Context, title: &str, filter: Filter<'_>) -> Pending<Option<PathBuf>> {
    run(ctx, template(ctx, title, filter), false)
}

#[cfg(not(test))]
fn run(ctx: &Context, dialog: rfd::FileDialog, save: bool) -> Pending<Option<PathBuf>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let wake = ctx.clone();
    let spawned = std::thread::Builder::new().name("flow-file-dialog".into()).spawn(move || {
        let picked = if save { dialog.save_file() } else { dialog.pick_file() };
        let _ = tx.send(Ok(picked));
        wake.request_repaint();
    });
    match spawned {
        Ok(_) => Pending::from_receiver(rx),
        Err(err) => Pending::ready(Err(format!("Could not open the file dialog: {err}"))),
    }
}

#[cfg(test)]
thread_local! {
    /// What dialogs answer in tests: the picked path, or `None` for cancelled.
    static ANSWER: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Tests: the path the next dialogs pick (`None`: they are cancelled).
#[cfg(test)]
pub(crate) fn answer_with(path: Option<PathBuf>) {
    ANSWER.with(|a| *a.borrow_mut() = path);
}

#[cfg(test)]
fn run(_ctx: &Context, _dialog: rfd::FileDialog, _save: bool) -> Pending<Option<PathBuf>> {
    Pending::ready(Ok(ANSWER.with(|a| a.borrow().clone())))
}
