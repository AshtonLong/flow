//! Flow's stored data: the model catalog and manager (downloads, verification and
//! status), transcript history in SQLite, and API keys encrypted one by one.
//! PORT: src/main/models/*, src/main/history/*, src/main/secrets/*
//!
//! Every file keeps the Electron build's format and location, so a user switching
//! builds keeps their models, history and keys.

pub mod downloader;
mod fs_util;
pub mod history;
pub mod models;
pub mod secrets;

#[cfg(test)]
mod test_support;

pub use downloader::{DEFAULT_STALL_TIMEOUT, DownloadOptions, ModelError, download_file, sha256_file};
pub use history::{HistoryError, HistoryStore, HistoryStoreOptions, fts_query};
pub use models::{FreeBytesFn, ModelEvent, ModelManager, ModelManagerDeps, describe_error, format_bytes};
pub use secrets::{SecretCipher, SecretError, SecretStore, secret_ref, system_cipher};

/// The catalog constants and checks, re-exported from `flow-core` (`src/main/models/index.ts`).
pub use flow_core::catalog::{
    CATALOG_PUBLIC_KEY, CATALOG_SIG_URL, CATALOG_URL, CATALOG_VERSION, parse_catalog, verify_catalog,
};
