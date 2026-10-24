//! Thin adapters over crates that are being ported in parallel. Each one keeps
//! the rest of `flow-ui` independent of an API that is not final yet; fix-ups
//! are marked `ADAPTER:`.

pub mod audio;
pub mod hotkeys;
