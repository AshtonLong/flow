//! Config: the schema, lenient validation, in-place TOML patching and the store.

pub mod atomic_write;
pub mod migrations;
pub mod patch;
mod schema;
pub mod store;
pub mod toml_doc;
pub mod toml_parse;
pub mod toml_write;
pub mod validate;
pub mod value;

pub use atomic_write::write_file_atomic;
pub use migrations::{MIGRATIONS, Migration};
pub use patch::{TomlPatchError, patch_toml};
pub use schema::*;
pub use store::{ConfigError, ConfigStore, ConfigStoreOptions, DEFAULT_CONFIG_TEXT, strip_api_keys};
pub use toml_doc::locate_line;
pub use toml_parse::{TomlSyntaxError, parse_toml};
pub use validate::{Validated, validate_config};
