//! Auto-update from GitHub Releases. Only installed (packaged) builds update.
//! Mirrors `src/main/updater.ts`, which used electron-updater: the same
//! `latest.yml` feed is read, the installer is downloaded and verified in the
//! background, and it runs silently when Flow quits.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use parking_lot::Mutex;
use sha2::{Digest, Sha512};

const FEED_URL: &str = "https://github.com/AshtonLong/flow/releases/latest/download/latest.yml";
const DOWNLOAD_BASE: &str = "https://github.com/AshtonLong/flow/releases/latest/download/";

#[derive(Debug, Clone, PartialEq)]
struct Release {
    version: String,
    /// Installer file name.
    path: String,
    /// Base64 SHA-512 of the installer.
    sha512: String,
}

/// Reads the fields electron-builder writes to `latest.yml`.
fn parse_feed(text: &str) -> Option<Release> {
    let mut version = None;
    let mut path = None;
    let mut sha512 = None;
    for line in text.lines() {
        // Top-level keys only; `files:` entries are indented.
        if line.starts_with(' ') || line.starts_with('-') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim().trim_matches(|c| c == '\'' || c == '"').to_string();
        match key.trim() {
            "version" => version = Some(value),
            "path" => path = Some(value),
            "sha512" => sha512 = Some(value),
            _ => {}
        }
    }
    Some(Release { version: version?, path: path?, sha512: sha512? })
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (semver::Version::parse(latest), semver::Version::parse(current)) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => false,
    }
}

pub struct Updater {
    http: reqwest::Client,
    packaged: bool,
    /// The verified installer waiting for quit.
    pending: Mutex<Option<(String, PathBuf)>>,
}

impl Updater {
    pub fn new(http: reqwest::Client, packaged: bool) -> Arc<Self> {
        Arc::new(Self { http, packaged, pending: Mutex::new(None) })
    }

    fn current() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Checks for an update and returns one line describing the result.
    pub async fn check(&self) -> String {
        if !self.packaged {
            return "Updates are checked in installed builds only.".into();
        }
        match self.check_inner().await {
            Ok(Some(version)) => format!("Flow {version} is available and will install when you quit."),
            Ok(None) => format!("Flow {} is up to date.", Self::current()),
            Err(err) => {
                log::warn!(target: "updater", "{err}");
                "Could not check for updates. Check your connection and try again.".into()
            }
        }
    }

    async fn check_inner(&self) -> anyhow::Result<Option<String>> {
        if let Some((version, _)) = self.pending.lock().clone() {
            return Ok(Some(version));
        }
        let text =
            self.http.get(FEED_URL).timeout(Duration::from_secs(20)).send().await?.error_for_status()?.text().await?;
        let release = parse_feed(&text).ok_or_else(|| anyhow::anyhow!("unreadable update feed"))?;
        if !is_newer(&release.version, Self::current()) {
            return Ok(None);
        }
        let file = self.download(&release).await?;
        log::info!(target: "updater", "update {} downloaded; installs on quit", release.version);
        *self.pending.lock() = Some((release.version.clone(), file));
        Ok(Some(release.version))
    }

    async fn download(&self, release: &Release) -> anyhow::Result<PathBuf> {
        if release.path.contains(['/', '\\']) || release.path.contains("..") {
            anyhow::bail!("unsafe installer name in the update feed");
        }
        let dir = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("flow-updater")
            .join("pending");
        tokio::fs::create_dir_all(&dir).await?;
        let target = dir.join(&release.path);
        let bytes = self
            .http
            .get(format!("{DOWNLOAD_BASE}{}", release.path))
            .timeout(Duration::from_secs(600))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let digest = base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&bytes));
        if digest != release.sha512 {
            anyhow::bail!("the downloaded update failed its checksum");
        }
        tokio::fs::write(&target, &bytes).await?;
        Ok(target)
    }

    /// Quiet background check shortly after launch. Test runs never update.
    pub fn schedule_check(self: &Arc<Self>, rt: &tokio::runtime::Handle) {
        if !self.packaged || crate::test_mode::enabled() {
            return;
        }
        let me = self.clone();
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            me.check().await;
        });
    }

    /// The installer to run after the app has shut down, if one is ready.
    pub fn take_pending_install(&self) -> Option<Box<dyn FnOnce()>> {
        let (_, file) = self.pending.lock().take()?;
        Some(Box::new(move || {
            // Silent install; the installer restarts Flow when it is done.
            let _ = std::process::Command::new(file).args(["/S", "--updated", "--force-run"]).spawn();
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_electron_builder_feeds() {
        let feed = "version: 0.2.0\nfiles:\n  - url: Flow-Setup-0.2.0.exe\n    sha512: abc==\n    size: 10\npath: Flow-Setup-0.2.0.exe\nsha512: abc==\nreleaseDate: '2026-11-01T00:00:00.000Z'\n";
        assert_eq!(
            parse_feed(feed),
            Some(Release { version: "0.2.0".into(), path: "Flow-Setup-0.2.0.exe".into(), sha512: "abc==".into() })
        );
        assert_eq!(parse_feed("files: []"), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
