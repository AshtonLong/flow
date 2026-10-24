//! Parsing and Ed25519 signature verification of the model catalog manifest.
//! PORT: src/main/models/catalog.ts

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize as _;
use serde_json::Value;

use super::Catalog;

/// Public half of the catalog signing key (Ed25519, SPKI PEM). The private half
/// lives outside the repository; see `scripts/sign-catalog.mjs`.
pub const CATALOG_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAPaBKDIhOEL41T4LvtPwy0i8S3/3h67ZcVVAYRzVglmA=
-----END PUBLIC KEY-----
";

/// Where the refreshed catalog and its detached signature are published.
pub const CATALOG_URL: &str = "https://raw.githubusercontent.com/AshtonLong/flow/main/resources/catalog.json";
/// `CATALOG_URL` + `.sig`.
pub const CATALOG_SIG_URL: &str = "https://raw.githubusercontent.com/AshtonLong/flow/main/resources/catalog.json.sig";

/// Highest manifest `version` this build understands.
pub const CATALOG_VERSION: i64 = 1;

/// A catalog with nothing in it, used only if the bundled file is unreadable.
pub const EMPTY_CATALOG: Catalog = Catalog {
    version: CATALOG_VERSION,
    as_of: String::new(),
    providers: Vec::new(),
    local: Vec::new(),
    cloud: Vec::new(),
    cleanup: Vec::new(),
};

const ED25519_SIGNATURE_BYTES: usize = 64;

/// DER prefix of an Ed25519 `SubjectPublicKeyInfo`: SEQUENCE { SEQUENCE { OID 1.3.101.112 },
/// BIT STRING (33 bytes, no unused bits) }. The 32-byte key follows it.
const ED25519_SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

/// Why a catalog manifest was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CatalogError(pub String);

fn has_crlf(bytes: &[u8]) -> bool {
    bytes.windows(2).any(|pair| pair == b"\r\n")
}

/// True if `signature_base64` is a valid signature of `bytes` under the catalog key.
/// Never panics: a malformed key or signature is simply not valid. Pass
/// [`CATALOG_PUBLIC_KEY`] for the key the app trusts.
pub fn verify_catalog(bytes: &[u8], signature_base64: &str, public_key_pem: &str) -> bool {
    let signature = decode_base64_loose(signature_base64.trim());
    let Ok(signature) = <[u8; ED25519_SIGNATURE_BYTES]>::try_from(signature.as_slice()) else {
        return false;
    };
    let signature = Signature::from_bytes(&signature);
    let Some(key) = parse_ed25519_spki_pem(public_key_pem) else {
        return false;
    };
    if key_verify(&key, bytes, &signature) {
        return true;
    }
    // The catalog is signed with LF line endings. A Windows checkout may have
    // rewritten them as CRLF, which changes the bytes but not the JSON.
    if !has_crlf(bytes) {
        return false;
    }
    let lf = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    key_verify(&key, lf.as_bytes(), &signature)
}

/// Plain RFC 8032 verification, as OpenSSL (and so Node's `crypto.verify`) does it.
fn key_verify(key: &VerifyingKey, bytes: &[u8], signature: &Signature) -> bool {
    use ed25519_dalek::Verifier as _;
    key.verify(bytes, signature).is_ok()
}

/// The Ed25519 key in an SPKI PEM (`-----BEGIN PUBLIC KEY-----`), or `None` for
/// anything else (Node's `createPublicKey` plus the `asymmetricKeyType` check).
fn parse_ed25519_spki_pem(pem: &str) -> Option<VerifyingKey> {
    const BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
    const END: &str = "-----END PUBLIC KEY-----";
    let start = pem.find(BEGIN)? + BEGIN.len();
    let end = start + pem[start..].find(END)?;
    let body: String = pem[start..end].chars().filter(|c| !c.is_whitespace()).collect();
    let der = STANDARD.decode(body).ok()?;
    let key = der.strip_prefix(&ED25519_SPKI_PREFIX[..])?;
    let key = <[u8; 32]>::try_from(key).ok()?;
    VerifyingKey::from_bytes(&key).ok()
}

/// Decodes base64 the way Node's `Buffer.from(text, 'base64')` does: both the
/// standard and the URL-safe alphabet, padding optional, other characters
/// skipped, and decoding stops at the first `=`.
fn decode_base64_loose(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Parses and validates a catalog manifest. Fails on invalid JSON, a schema
/// failure, a manifest version newer than this build, or a local model whose
/// download URL or file name is unsafe.
pub fn parse_catalog(bytes: impl AsRef<[u8]>) -> Result<Catalog, CatalogError> {
    let text = String::from_utf8_lossy(bytes.as_ref());
    let value: Value = serde_json::from_str(&text).map_err(|e| CatalogError(e.to_string()))?;
    check_kinds(&value)?;
    let catalog = Catalog::deserialize(&value).map_err(|e| CatalogError(e.to_string()))?;
    if catalog.version > CATALOG_VERSION {
        return Err(CatalogError(format!(
            "Catalog version {} is newer than this app understands",
            catalog.version
        )));
    }
    for entry in &catalog.local {
        if !is_https_url(&entry.url) {
            return Err(CatalogError(format!("Catalog entry {} has a download URL that is not https", entry.id)));
        }
        // Files are stored flat in the models directory; a path here could escape it.
        let file = entry.file.as_str();
        if file.is_empty() || file.chars().all(|c| c == '.') || file.contains(['\\', '/', ':']) {
            return Err(CatalogError(format!("Catalog entry {} has an unsafe file name", entry.id)));
        }
        if entry.sha256.len() != 64 || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CatalogError(format!("Catalog entry {} has a malformed SHA-256", entry.id)));
        }
    }
    Ok(catalog)
}

/// The manifest tags every entry with its list (`"kind": "local"` and so on). The
/// Rust entry types do not carry the tag, so it is checked here, as the schema does.
fn check_kinds(value: &Value) -> Result<(), CatalogError> {
    for list in ["local", "cloud", "cleanup"] {
        let Some(entries) = value.get(list).and_then(Value::as_array) else {
            continue; // a missing or mistyped list fails the typed parse
        };
        for (index, entry) in entries.iter().enumerate() {
            if entry.is_object() && entry.get("kind").and_then(Value::as_str) != Some(list) {
                return Err(CatalogError(format!("{list}[{index}].kind: expected \"{list}\"")));
            }
        }
    }
    Ok(())
}

/// True for a well-formed `https:` URL.
pub fn is_https_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CloudModelEntry, LocalModelEntry, ProviderApi, ProviderEntry, RunsOn, Tier};
    use crate::config::{CleanupConfig, DEFAULT_LOCAL_MODEL};
    use ed25519_dalek::{Signer as _, SigningKey};
    use sha2::{Digest as _, Sha256};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    // ── Fixtures (port of tests/unit/models/helpers.ts) ─────────────────────

    /// Deterministic pseudo-random bytes.
    fn random_bytes(length: usize, seed: u32) -> Vec<u8> {
        let mut state = if seed == 0 { 1 } else { seed };
        (0..length)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    struct TestKey {
        public_pem: String,
        private_key: SigningKey,
    }

    /// A fresh key pair; seeds come from a counter, so every call differs.
    fn test_key() -> TestKey {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let seed: [u8; 32] = Sha256::digest(format!("flow-core test key {n}")).into();
        let private_key = SigningKey::from_bytes(&seed);
        let mut der = ED25519_SPKI_PREFIX.to_vec();
        der.extend_from_slice(private_key.verifying_key().as_bytes());
        let public_pem = format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", STANDARD.encode(der));
        TestKey { public_pem, private_key }
    }

    fn sign_bytes(bytes: &[u8], key: &TestKey) -> String {
        STANDARD.encode(key.private_key.sign(bytes).to_bytes())
    }

    fn local_entry(id: &str, bytes: &[u8]) -> LocalModelEntry {
        LocalModelEntry {
            id: id.into(),
            name: format!("Model {id}"),
            family: "test".into(),
            description: "A test model".into(),
            url: format!("https://models.test/{id}.gguf"),
            file: format!("{id}.gguf"),
            sha256: sha256(bytes),
            size_bytes: bytes.len() as u64,
            languages: vec!["en".into()],
            licence: "MIT".into(),
            licence_url: None,
            runs_on: RunsOn::Cpu,
            tier: Tier::Core,
            wer: None,
            gated: false,
            note: None,
        }
    }

    fn provider(id: &str, name: &str, base_url: &str, keyless: bool) -> ProviderEntry {
        ProviderEntry {
            id: id.into(),
            name: name.into(),
            api: ProviderApi::Openai,
            base_url: base_url.into(),
            key_url: None,
            keyless,
        }
    }

    fn cloud(id: &str, provider: &str, model: &str, name: &str, description: &str, price: f64) -> CloudModelEntry {
        CloudModelEntry {
            id: id.into(),
            provider: provider.into(),
            model: model.into(),
            name: name.into(),
            description: description.into(),
            price_per_hour: price,
            recommended: false,
            hints: false,
            min_billed_seconds: None,
            docs_url: None,
        }
    }

    fn test_catalog(local: Vec<LocalModelEntry>) -> Catalog {
        Catalog {
            version: 1,
            as_of: "2026-10-03".into(),
            providers: vec![
                provider("groq", "Groq", "https://api.groq.com/openai/v1", false),
                provider("openai", "OpenAI", "https://api.openai.com/v1", false),
                provider("ollama", "Ollama (local)", "http://localhost:11434/v1", true),
            ],
            local,
            cloud: vec![
                cloud("groq/whisper-large-v3-turbo", "groq", "whisper-large-v3-turbo", "Whisper large-v3-turbo", "Lowest price", 0.04),
                cloud("openai/gpt-4o-transcribe", "openai", "gpt-4o-transcribe", "gpt-4o-transcribe", "OpenAI", 0.36),
                cloud("ollama/whisper", "ollama", "whisper", "Local Whisper", "Keyless", 0.0),
            ],
            cleanup: vec![],
        }
    }

    /// The manifest as JSON, with each entry's `kind` tag first, as the TS writes it.
    fn to_json(catalog: &Catalog) -> Value {
        let mut value = serde_json::to_value(catalog).expect("catalog serialises");
        for list in ["local", "cloud", "cleanup"] {
            if let Some(entries) = value.get_mut(list).and_then(Value::as_array_mut) {
                for entry in entries {
                    let mut tagged = serde_json::Map::new();
                    tagged.insert("kind".into(), Value::from(list));
                    if let Value::Object(fields) = entry.take() {
                        tagged.extend(fields);
                    }
                    *entry = Value::Object(tagged);
                }
            }
        }
        value
    }

    fn encode_value(value: &Value) -> Vec<u8> {
        format!("{}\n", serde_json::to_string_pretty(value).expect("json")).into_bytes()
    }

    fn encode(catalog: &Catalog) -> Vec<u8> {
        encode_value(&to_json(catalog))
    }

    // ── bundled catalog ─────────────────────────────────────────────────────

    fn bundled() -> (Vec<u8>, String) {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../resources/catalog.json");
        let bytes = std::fs::read(path).expect("bundled catalog");
        let signature = std::fs::read_to_string(format!("{path}.sig")).expect("bundled signature");
        (bytes, signature)
    }

    #[test]
    fn bundled_catalog_validates_against_the_schema() {
        let (bytes, _) = bundled();
        let catalog = parse_catalog(&bytes).expect("parses");
        assert_eq!(catalog.version, 1);
        assert!(regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap().is_match(&catalog.as_of));
    }

    #[test]
    fn bundled_catalog_carries_a_signature_that_verifies_with_the_embedded_public_key() {
        let (bytes, signature) = bundled();
        assert!(verify_catalog(&bytes, &signature, CATALOG_PUBLIC_KEY));
    }

    #[test]
    fn bundled_catalog_still_verifies_after_a_checkout_rewrote_its_line_endings() {
        let (bytes, signature) = bundled();
        let text = String::from_utf8(bytes).unwrap().replace("\r\n", "\n");
        assert!(verify_catalog(text.as_bytes(), &signature, CATALOG_PUBLIC_KEY));
        let crlf = text.replace('\n', "\r\n");
        assert!(verify_catalog(crlf.as_bytes(), &signature, CATALOG_PUBLIC_KEY));
    }

    #[test]
    fn bundled_catalog_has_unique_ids_and_known_providers() {
        let catalog = parse_catalog(bundled().0).unwrap();
        let ids: Vec<&str> = catalog
            .local
            .iter()
            .map(|e| e.id.as_str())
            .chain(catalog.cloud.iter().map(|e| e.id.as_str()))
            .chain(catalog.cleanup.iter().map(|e| e.id.as_str()))
            .collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        let providers: HashSet<&str> = catalog.providers.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(providers.len(), catalog.providers.len());
        let pairs = catalog
            .cloud
            .iter()
            .map(|e| (&e.id, &e.provider, &e.model))
            .chain(catalog.cleanup.iter().map(|e| (&e.id, &e.provider, &e.model)));
        for (id, provider, model) in pairs {
            assert!(providers.contains(provider.as_str()), "{id}");
            assert_eq!(*id, format!("{provider}/{model}"));
        }
    }

    #[test]
    fn bundled_catalog_lists_local_models_with_https_urls_that_end_in_their_file_name() {
        let catalog = parse_catalog(bundled().0).unwrap();
        assert!(!catalog.local.is_empty());
        let hex = regex::Regex::new(r"^[0-9a-f]{64}$").unwrap();
        for entry in &catalog.local {
            let url = url::Url::parse(&entry.url).unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("huggingface.co"));
            assert!(url.path().ends_with(&format!("/resolve/main/{}", entry.file)), "{}", entry.id);
            assert!(hex.is_match(&entry.sha256));
            assert!(entry.size_bytes > 0);
            assert!(!entry.id.contains('/'));
        }
    }

    #[test]
    fn bundled_catalog_contains_the_config_defaults() {
        let catalog = parse_catalog(bundled().0).unwrap();
        assert!(catalog.local.iter().any(|e| e.id == DEFAULT_LOCAL_MODEL));
        let llm_model = CleanupConfig::default().llm_model;
        assert!(catalog.cleanup.iter().any(|e| e.id == llm_model));
        let recommended: Vec<&str> =
            catalog.cloud.iter().filter(|e| e.recommended).map(|e| e.id.as_str()).collect();
        assert_eq!(recommended, ["groq/whisper-large-v3-turbo"]);
        assert_eq!(catalog.cleanup.iter().filter(|e| e.recommended).count(), 1);
    }

    // ── verify_catalog ──────────────────────────────────────────────────────

    struct Signed {
        key: TestKey,
        bytes: Vec<u8>,
        signature: String,
    }

    fn signed() -> Signed {
        let key = test_key();
        let bytes = encode(&test_catalog(vec![local_entry("a", &random_bytes(64, 1))]));
        let signature = sign_bytes(&bytes, &key);
        Signed { key, bytes, signature }
    }

    #[test]
    fn verify_catalog_accepts_a_good_signature() {
        let s = signed();
        assert!(verify_catalog(&s.bytes, &s.signature, &s.key.public_pem));
        assert!(verify_catalog(&s.bytes, &format!("{}\n", s.signature), &s.key.public_pem));
    }

    #[test]
    fn verify_catalog_rejects_tampered_bytes() {
        let s = signed();
        let mut tampered = s.bytes.clone();
        let at = tampered.windows(11).position(|w| w == b"models.test").unwrap();
        tampered[at] = b'n';
        assert!(!verify_catalog(&tampered, &s.signature, &s.key.public_pem));
    }

    #[test]
    fn verify_catalog_rejects_a_signature_made_with_another_key() {
        let s = signed();
        assert!(!verify_catalog(&s.bytes, &s.signature, &test_key().public_pem));
        assert!(!verify_catalog(&s.bytes, &s.signature, CATALOG_PUBLIC_KEY));
        let (bundled_bytes, bundled_signature) = bundled();
        assert!(!verify_catalog(&bundled_bytes, &bundled_signature, &s.key.public_pem));
    }

    #[test]
    fn verify_catalog_returns_false_without_throwing_for_malformed_input() {
        let s = signed();
        assert!(!verify_catalog(&s.bytes, "", &s.key.public_pem));
        assert!(!verify_catalog(&s.bytes, "not base64 !!", &s.key.public_pem));
        assert!(!verify_catalog(&s.bytes, &s.signature, "not a key"));
    }

    // ── parse_catalog ───────────────────────────────────────────────────────

    fn entry() -> LocalModelEntry {
        local_entry("a", &random_bytes(64, 1))
    }

    #[test]
    fn parse_catalog_accepts_bytes_or_a_string() {
        let catalog = test_catalog(vec![entry()]);
        assert_eq!(parse_catalog(encode(&catalog)).unwrap(), catalog);
        let text = serde_json::to_string(&to_json(&catalog)).unwrap();
        assert_eq!(parse_catalog(&text).unwrap(), catalog);
    }

    #[test]
    fn parse_catalog_throws_on_invalid_json_and_on_a_schema_failure() {
        assert!(parse_catalog("{").is_err());
        assert!(parse_catalog(r#"{"version":1}"#).is_err());
        let mut broken = to_json(&test_catalog(vec![entry()]));
        broken["local"][0]["sizeBytes"] = Value::from("big");
        assert!(parse_catalog(encode_value(&broken)).is_err());
        // The `kind` tag is part of the schema too.
        let mut untagged = to_json(&test_catalog(vec![entry()]));
        untagged["local"][0]["kind"] = Value::from("cloud");
        assert!(parse_catalog(encode_value(&untagged)).is_err());
    }

    #[test]
    fn parse_catalog_rejects_a_download_url_that_is_not_https() {
        let catalog = test_catalog(vec![LocalModelEntry { url: "http://models.test/a.gguf".into(), ..entry() }]);
        let error = parse_catalog(encode(&catalog)).unwrap_err();
        assert!(error.0.contains("not https"), "{error}");
    }

    #[test]
    fn parse_catalog_rejects_a_file_name_that_could_leave_the_models_directory() {
        for file in ["../a.gguf", "..\\a.gguf", "sub/a.gguf", "C:a.gguf", "..", ""] {
            let catalog = test_catalog(vec![LocalModelEntry { file: file.into(), ..entry() }]);
            let error = parse_catalog(encode(&catalog)).unwrap_err();
            assert!(error.0.contains("unsafe file name"), "{file}: {error}");
        }
    }

    #[test]
    fn parse_catalog_rejects_a_manifest_version_newer_than_this_build() {
        let catalog = Catalog { version: 2, ..test_catalog(vec![entry()]) };
        let error = parse_catalog(encode(&catalog)).unwrap_err();
        assert!(error.0.contains("newer"), "{error}");
    }

    #[test]
    fn parse_catalog_rejects_a_malformed_sha256() {
        let catalog = test_catalog(vec![LocalModelEntry { sha256: "z".repeat(64), ..entry() }]);
        let error = parse_catalog(encode(&catalog)).unwrap_err();
        assert_eq!(error.0, "Catalog entry a has a malformed SHA-256");
    }

    // ── remote location ─────────────────────────────────────────────────────

    #[test]
    fn remote_location_points_at_the_repository_over_https() {
        assert_eq!(CATALOG_URL, "https://raw.githubusercontent.com/AshtonLong/flow/main/resources/catalog.json");
        assert_eq!(CATALOG_SIG_URL, format!("{CATALOG_URL}.sig"));
    }

    #[test]
    fn https_urls_are_recognised() {
        assert!(is_https_url("https://models.test/a.gguf"));
        assert!(!is_https_url("http://models.test/a.gguf"));
        assert!(!is_https_url("file:///c:/a.gguf"));
        assert!(!is_https_url("not a url"));
    }

    #[test]
    fn empty_catalog_is_empty() {
        assert_eq!(EMPTY_CATALOG.version, CATALOG_VERSION);
        assert!(EMPTY_CATALOG.local.is_empty() && EMPTY_CATALOG.providers.is_empty());
    }
}
