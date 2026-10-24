//! Config: the schema, lenient validation, in-place TOML patching and the store.

mod schema;
pub mod validate;
pub mod value;
pub mod toml_parse;
pub mod toml_doc;
pub mod toml_write;
pub mod patch;
pub mod migrations;
pub mod atomic_write;
pub mod store;

pub use schema::*;
pub use validate::{validate_config, Validated};
pub use patch::{patch_toml, TomlPatchError};
pub use store::{strip_api_keys, ConfigError, ConfigStore, ConfigStoreOptions, DEFAULT_CONFIG_TEXT};
pub use migrations::{Migration, MIGRATIONS};
pub use toml_doc::locate_line;
pub use toml_parse::{parse_toml, TomlSyntaxError};
pub use atomic_write::write_file_atomic;
