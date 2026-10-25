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
mod render;
mod runner;
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
    if std::env::var("FLOW_RENDERER").is_ok_and(|r| r.starts_with("glow") || r.starts_with("wgpu")) {
        return glow_experiment::run(viewport, backend, options);
    }
    // Benchmarks repaint as fast as they can; the window paces to the monitor.
    let paced = options.bench.is_none();
    let result = runner::run(
        viewport,
        paced,
        Box::new(move |ctx, hwnd| shell::SettingsApp::new(ctx, backend, options, hwnd)),
    );
    match result {
        Ok(()) => 0,
        Err(err) => {
            log::error!(target: "settings", "the settings window failed: {err}");
            1
        }
    }
}

mod glow_experiment {
    use super::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    struct Glow(shell::SettingsApp);
    impl eframe::App for Glow {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            runner::App::ui(&mut self.0, ui);
        }
        fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
            runner::App::clear_color(&self.0).to_normalized_gamma_f32()
        }
        fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
            runner::App::raw_input_hook(&mut self.0, ctx, raw_input);
        }
    }

    pub fn run(viewport: egui::ViewportBuilder, backend: Arc<dyn Backend>, options: RunOptions) -> i32 {
        let which = std::env::var("FLOW_RENDERER").unwrap_or_default();
        let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
        let backends = match which.as_str() {
            "wgpu-dx12" => Some(eframe::wgpu::Backends::DX12),
            "wgpu-vulkan" => Some(eframe::wgpu::Backends::VULKAN),
            "wgpu-gl" => Some(eframe::wgpu::Backends::GL),
            _ => None,
        };
        if let Some(backends) = backends
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(ref mut create) = wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = backends;
            create.power_preference = eframe::wgpu::PowerPreference::LowPower;
        }
        let mut glow_options = eframe::egui_glow::GlowConfiguration::default();
        if which == "glow-novsync" || options.bench.is_some() {
            glow_options.vsync = false;
        }
        let native = eframe::NativeOptions {
            viewport,
            wgpu_options,
            glow_options,
            renderer: if backends.is_some() { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow },
            centered: true,
            persist_window: false,
            clipboard_shortcuts: false,
            multisampling: 0,
            depth_buffer: 0,
            stencil_buffer: 0,
            ..Default::default()
        };
        let result = eframe::run_native(
            "Flow",
            native,
            Box::new(move |cc| {
                let hwnd = match cc.window_handle().map(|h| h.as_raw()) {
                    Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
                    _ => 0,
                };
                Ok(Box::new(Glow(shell::SettingsApp::new(&cc.egui_ctx, backend, options, hwnd))))
            }),
        );
        i32::from(result.is_err())
    }
}
