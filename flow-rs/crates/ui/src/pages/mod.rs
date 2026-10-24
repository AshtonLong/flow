//! The settings pages, one module each (`src/renderer/settings/pages/*.tsx`).
//!
//! A page is a struct implementing [`Page`]. The shell creates it when the
//! page is opened and drops it when another page is opened (React unmounts
//! inactive tab panels the same way), so page state starts fresh each visit
//! and resources it owns (an open microphone) are released on leaving.
//!
//! Pages read and write everything through [`Cx`]:
//!
//! ```ignore
//! impl Page for MyPage {
//!     fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>) {
//!         let sounds = cx.config.config().audio.sounds;
//!         page_header(ui, "My page", None);
//!         Group::new("Audio").show(ui, |ui| {
//!             SettingRow::new("Sounds", "Play a short sound.").show(ui, |ui| {
//!                 if let Some(v) = Toggle::new(sounds).show(ui).changed {
//!                     cx.config.set(path!["audio", "sounds"], v);
//!                 }
//!             });
//!         });
//!     }
//! }
//! ```

pub mod advanced;
pub mod cleanup;
pub mod general;
pub mod history;
pub mod hotkeys;
pub mod models;
pub mod profiles;

use egui::Ui;
use flow_core::protocol::settings::Event;

use crate::backend::Link;
use crate::design::Icon;
use crate::stores::{AppInfoStore, CatalogStore, ConfigStore, Microphones};

/// The pages, in nav order. `as_str` is the id used by `--page` and `Event::Focus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageId {
    General,
    Hotkeys,
    Models,
    Cleanup,
    Profiles,
    History,
    Advanced,
}

impl PageId {
    pub const ALL: [Self; 7] = [
        Self::General,
        Self::Hotkeys,
        Self::Models,
        Self::Cleanup,
        Self::Profiles,
        Self::History,
        Self::Advanced,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Hotkeys => "hotkeys",
            Self::Models => "models",
            Self::Cleanup => "cleanup",
            Self::Profiles => "profiles",
            Self::History => "history",
            Self::Advanced => "advanced",
        }
    }

    /// `isPageId`: `general`, `models`, … (a leading `#` or `#/` is accepted, like the URL hash).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim_start_matches('#').trim_start_matches('/');
        Self::ALL.into_iter().find(|p| p.as_str() == text)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Hotkeys => "Hotkeys",
            Self::Models => "Models",
            Self::Cleanup => "Cleanup",
            Self::Profiles => "Profiles",
            Self::History => "History",
            Self::Advanced => "Advanced",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::General => Icon::Settings2,
            Self::Hotkeys => Icon::Keyboard,
            Self::Models => Icon::Boxes,
            Self::Cleanup => Icon::SpellCheck,
            Self::Profiles => Icon::AppWindow,
            Self::History => Icon::History,
            Self::Advanced => Icon::Wrench,
        }
    }
}

/// What a page can reach: the stores, the backend, this frame's events and navigation.
pub struct Cx<'a> {
    /// `useConfig()`: `config()`, `snapshot()`, `set(path, value)`, `remove(path)`, `patch(patches)`.
    pub config: &'a mut ConfigStore,
    /// `useCatalog()`: `snapshot()`, `progress()`, `reload()`, `download_of(id)`.
    pub catalog: &'a mut CatalogStore,
    /// `useAppInfo()`: `info()`, `refresh()`.
    pub app_info: &'a mut AppInfoStore,
    /// `useMicrophones()`: `list()`, `refresh()`.
    pub mics: &'a mut Microphones,
    /// The main process: `link.call::<T>(Request::…)` returns a `Pending<T>`; `link.send(…)` fires and forgets.
    pub link: &'a Link,
    /// Events that arrived since the last frame (bench results, capture progress, dictations…).
    pub events: &'a [Event],
    pub(crate) nav: &'a mut Option<PageId>,
}

impl Cx<'_> {
    /// `useGoTo()`: sends the user to another page ("Add a key on the Models page").
    pub fn go_to(&mut self, page: PageId) {
        *self.nav = Some(page);
    }
}

/// One settings page.
pub trait Page {
    /// Draws the page into the content column (760 px max, 32 px side padding),
    /// below the error notice and the issues banner.
    fn ui(&mut self, ui: &mut Ui, cx: &mut Cx<'_>);
}

/// A fresh instance of a page.
pub fn create(id: PageId) -> Box<dyn Page> {
    match id {
        PageId::General => Box::new(general::General::default()),
        PageId::Hotkeys => Box::new(hotkeys::Hotkeys::default()),
        PageId::Models => Box::new(models::Models::default()),
        PageId::Cleanup => Box::new(cleanup::Cleanup::default()),
        PageId::Profiles => Box::new(profiles::Profiles::default()),
        PageId::History => Box::new(history::History::default()),
        PageId::Advanced => Box::new(advanced::Advanced::default()),
    }
}

/// The body of a page that has not been ported yet. Remove when porting.
pub(crate) fn not_ported(ui: &mut Ui, title: &str, source: &str) {
    crate::components::page_header(ui, title, None);
    crate::components::Notice::new(
        crate::components::Tone::Info,
        &format!("This page has not been ported yet ({source})."),
    )
    .show(ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_ids() {
        assert_eq!(PageId::parse("models"), Some(PageId::Models));
        assert_eq!(PageId::parse("#/history"), Some(PageId::History));
        assert_eq!(PageId::parse("nope"), None);
        for page in PageId::ALL {
            assert_eq!(PageId::parse(page.as_str()), Some(page));
        }
    }
}
