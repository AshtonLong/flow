//! The settings window on the mock backend, for previews and screenshots.
//!
//! ```sh
//! cargo run -p flow-ui --example preview -- --page general --theme dark --screenshot out.png
//! ```
//!
//! Flags: `--page <id>`, `--theme light|dark` (the mock's Windows theme),
//! `--screenshot <file.png>`, `--scroll <points>`, `--contrast high`,
//! `--motion reduced`, `--gallery`, `--demo <name>` (gallery overlays, or with
//! `--onboarding` a later step: `hotkey`, `model`, `cloud`, `try`), and the
//! mock's starting states: `--fresh`,
//! `--onboarding`, `--noissues`, `--history-off`, `--downloading <id>`,
//! `--degraded`.

use std::path::PathBuf;

use flow_ui::{MockOptions, RunOptions, mock_backend_with, run};

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) && record.target() == "settings" {
            eprintln!("{} [{}] {}", record.level(), record.target(), record.args());
        }
    }
    fn flush(&self) {}
}

fn main() {
    let _ = log::set_logger(&StderrLogger).map(|()| log::set_max_level(log::LevelFilter::Info));
    let mut mock = MockOptions::default();
    let mut options = RunOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--page" => options.page = args.next(),
            "--theme" => mock.dark = args.next().map(|t| t == "dark"),
            "--screenshot" => options.screenshot = args.next().map(PathBuf::from),
            "--scroll" => options.scroll = args.next().and_then(|v| v.parse().ok()),
            "--contrast" => mock.high_contrast = args.next().as_deref() == Some("high"),
            "--motion" => mock.reduced_motion = args.next().as_deref() == Some("reduced"),
            "--gallery" => options.gallery = true,
            "--demo" => options.demo = args.next(),
            "--bench" => options.bench = args.next().and_then(|v| v.parse().ok()),
            "--fresh" => mock.fresh = true,
            "--onboarding" => mock.onboarding = true,
            "--noissues" => mock.no_issues = true,
            "--history-off" => mock.history_off = true,
            "--downloading" => mock.downloading = args.next(),
            "--degraded" => mock.degraded = true,
            other => eprintln!("unknown flag {other}"),
        }
    }
    std::process::exit(run(mock_backend_with(mock), options));
}
