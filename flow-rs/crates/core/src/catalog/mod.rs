//! The model catalog: manifest types, parsing and signature verification.

mod types;
pub mod verify;

pub use types::*;
pub use verify::{
    CATALOG_PUBLIC_KEY, CATALOG_SIG_URL, CATALOG_URL, CATALOG_VERSION, CatalogError, EMPTY_CATALOG, is_https_url,
    parse_catalog, verify_catalog,
};
