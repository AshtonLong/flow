//! Pure helpers ported from `src/renderer/settings/lib/`: formatting, config
//! patches, list drafts, catalog lookups and profile helpers. The stateful
//! parts of `lib/` (config, catalog, hooks, theme) are in [`crate::stores`].

pub mod format;
pub mod list_draft;
pub mod models;
pub mod patch;
pub mod profiles;
