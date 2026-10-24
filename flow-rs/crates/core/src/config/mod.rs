//! Config: the schema, lenient validation, in-place TOML patching and the store.

mod schema;
pub mod validate;
pub mod value;
pub mod toml_doc;
pub mod toml_write;
pub mod patch;
pub mod migrations;
pub mod store;

pub use schema::*;
pub use validate::{validate_config, Validated};
