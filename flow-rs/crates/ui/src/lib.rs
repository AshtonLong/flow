//! Flow's settings window: an eframe (glow) app that runs in its own process
//! (`Flow.exe --settings-window`) and talks to the main process over
//! stdin/stdout JSON lines. Port of `src/renderer/settings/**`.
//!
//! ```ignore
//! // In the app's `--settings-window` branch:
//! let code = flow_ui::run(flow_ui::pipe_backend(), flow_ui::RunOptions { page, ..Default::default() });
//! ```
//!
//! Layout:
//! - [`backend`]: the [`Backend`] trait, the pipe and the mock, and [`backend::Link`]
//!   (typed calls returning [`backend::Pending`]).
//! - [`stores`]: config (optimistic writes), catalog, app info, microphones, theme.
//! - [`design`]: tokens, fonts, text styles, shadows, icons, motion, focus rings.
//! - [`components`]: buttons, cards and rows, toggles, selects, inputs, dialogs, …
//! - [`pages`]: one module per page, plus the [`pages::Page`] trait and [`pages::Cx`].
//! - [`onboarding`]: the first-run flow.
//! - [`util`]: pure helpers (formatting, patches, list drafts, catalog and profile lookups).
//! - [`adapters`]: thin wrappers over crates still being ported (audio, hotkeys).
//!
//! See `README.md` for the patterns page porters follow.

pub mod adapters;
pub mod backend;
pub mod components;
pub mod design;
pub mod onboarding;
pub mod pages;
mod platform;
mod shell;
pub mod stores;
pub mod util;

#[cfg(test)]
mod testing;

use std::sync::Arc;

pub use backend::{Backend, MockOptions, mock_backend, mock_backend_with, pipe_backend};
pub use shell::RunOptions;

/// Opens the settings window and runs it until it closes. Returns the process
/// exit code: 0 after a normal close, 1 when the window could not be created.
pub fn run(backend: Arc<dyn Backend>, options: RunOptions) -> i32 {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Flow")
        .with_app_id("Flow")
        .with_inner_size([1000.0, 700.0])
        .with_min_inner_size([860.0, 560.0])
        // Flow draws its own title strip and caption buttons.
        .with_decorations(false)
        .with_resizable(true);
    if let Some(icon) = shell::icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let native = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        centered: true,
        persist_window: false,
        // Copy and paste go through Win32 (eframe's clipboard feature is off).
        clipboard_shortcuts: false,
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        ..Default::default()
    };
    let result = eframe::run_native(
        "Flow",
        native,
        Box::new(move |cc| Ok(Box::new(shell::SettingsApp::new(cc, backend, options)))),
    );
    match result {
        Ok(()) => 0,
        Err(err) => {
            log::error!(target: "settings", "the settings window failed: {err}");
            1
        }
    }
}
