//! The Windows theme (from `ThemeGet` / `ThemeChanged`) and the
//! `general.theme` override. Port of `lib/theme.ts`.

use flow_core::config::ThemeSetting;
use flow_core::protocol::settings::{Event, Request};
use flow_core::types::ThemeInfo;

use crate::backend::{Link, Pending};

/// `useSystemTheme`: the main process's view of the Windows theme, with the
/// window's own reading as the fallback until it answers.
pub struct SystemTheme {
    theme: ThemeInfo,
    loading: Option<Pending<ThemeInfo>>,
}

impl SystemTheme {
    pub(crate) fn new(link: &Link) -> Self {
        Self { theme: crate::platform::system_theme(), loading: Some(link.call(Request::ThemeGet)) }
    }

    pub(crate) fn pump(&mut self, events: &[Event]) {
        if let Some(result) = self.loading.as_mut().and_then(Pending::take) {
            self.loading = None;
            // On failure keep the fallback.
            if let Ok(theme) = result {
                self.theme = theme;
            }
        }
        for event in events {
            if let Event::ThemeChanged(theme) = event {
                self.theme = theme.clone();
            }
        }
    }

    pub fn info(&self) -> &ThemeInfo {
        &self.theme
    }
}

/// What the window draws with. The colours are Flow's own (`common/brand.css`),
/// so the Windows accent colour is not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appearance {
    pub dark: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
}

/// `useApplyTheme`: the system theme with the `general.theme` override applied.
pub fn appearance(system: &ThemeInfo, setting: ThemeSetting) -> Appearance {
    let dark = match setting {
        ThemeSetting::System => system.dark,
        ThemeSetting::Dark => true,
        ThemeSetting::Light => false,
    };
    Appearance { dark, high_contrast: system.high_contrast, reduced_motion: system.reduced_motion }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_wins_over_the_system() {
        let system = ThemeInfo { dark: true, ..ThemeInfo::default() };
        assert!(appearance(&system, ThemeSetting::System).dark);
        assert!(!appearance(&system, ThemeSetting::Light).dark);
        assert!(appearance(&ThemeInfo::default(), ThemeSetting::Dark).dark);
    }
}
