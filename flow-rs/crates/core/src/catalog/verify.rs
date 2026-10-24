//! PORT: src/main/models/catalog.ts (parse + Ed25519 verification).

use super::Catalog;

pub const CATALOG_URL: &str = "https://raw.githubusercontent.com/AshtonLong/flow/main/resources/catalog.json";
pub const CATALOG_SIG_URL: &str = "https://raw.githubusercontent.com/AshtonLong/flow/main/resources/catalog.json.sig";
pub const CATALOG_VERSION: i64 = 1;
pub const CATALOG_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAPaBKDIhOEL41T4LvtPwy0i8S3/3h67ZcVVAYRzVglmA=
-----END PUBLIC KEY-----
";

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CatalogError(pub String);

pub fn parse_catalog(bytes: &[u8]) -> Result<Catalog, CatalogError> {
    serde_json::from_slice(bytes).map_err(|e| CatalogError(e.to_string()))
}

pub fn verify_catalog(_bytes: &[u8], _signature_base64: &str, _public_key_pem: &str) -> bool {
    false
}
