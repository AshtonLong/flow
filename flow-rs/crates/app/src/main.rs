//! Flow: push-to-talk dictation for Windows. Hold a key, speak, release — text
//! lands at the cursor.
//!
//! One executable, three processes:
//! - `Flow.exe [--hidden] [--settings] [--page <id>]` — the tray app.
//! - `Flow.exe --engine` — the speech engine, started on demand by the tray app.
//! - `Flow.exe --settings-window [--page <id>]` — the settings window, started
//!   by the tray app while it is open.

#![windows_subsystem = "windows"]

mod app;
mod ipc;
mod logger;
mod paths;
mod router;
mod session;
mod settings_host;
mod system;
mod test_mode;
mod updater;

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(target: "app", "panic: {info}");
        previous(info);
    }));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    install_panic_hook();
    let code = if args.iter().any(|a| a == "--engine") {
        logger::init_stderr();
        flow_engine::run_engine_process()
    } else if args.iter().any(|a| a == "--settings-window") {
        logger::init_stderr();
        let options = flow_ui::RunOptions { page: app::page_arg(&args), ..Default::default() };
        flow_ui::run(flow_ui::pipe_backend(), options)
    } else {
        app::run(args)
    };
    std::process::exit(code);
}
