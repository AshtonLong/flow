//! PORT: tests/unit/secrets/store.test.ts, plus the system cipher.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Stands in for safeStorage: reversible, and the output does not contain the input.
struct FakeCipher {
    available: AtomicBool,
}

fn fake_cipher(available: bool) -> Arc<FakeCipher> {
    Arc::new(FakeCipher { available: AtomicBool::new(available) })
}

impl SecretCipher for FakeCipher {
    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }
    fn encrypt(&self, plain: &str) -> Result<Vec<u8>, SecretError> {
        let mut out = b"v10".to_vec();
        out.extend(plain.bytes().map(|byte| byte ^ 0x5a));
        Ok(out)
    }
    fn decrypt(&self, data: &[u8]) -> Result<String, SecretError> {
        let rest = data.strip_prefix(b"v10").ok_or_else(|| SecretError::Cipher("not ours".into()))?;
        String::from_utf8(rest.iter().map(|byte| byte ^ 0x5a).collect()).map_err(|e| SecretError::Cipher(e.to_string()))
    }
}

struct Dir {
    _root: tempfile::TempDir,
    file: PathBuf,
}

fn dir() -> Dir {
    let root = tempfile::Builder::new().prefix("flow-secrets-").tempdir().unwrap();
    let file = root.path().join("Flow").join("secrets.bin");
    Dir { _root: root, file }
}

fn store(file: &Path, cipher: Arc<FakeCipher>) -> SecretStore {
    SecretStore::new(file.to_path_buf(), cipher)
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

// ── secret_ref ──────────────────────────────────────────────────────────────

#[test]
fn secret_ref_extracts_the_id_from_a_reference() {
    assert_eq!(secret_ref(Some("secret:groq")), Some("groq"));
    assert_eq!(secret_ref(Some("secret:custom-lan")), Some("custom-lan"));
    assert_eq!(secret_ref(Some("secret:a:b")), Some("a:b"));
}

#[test]
fn secret_ref_returns_null_for_anything_else() {
    assert_eq!(secret_ref(None), None);
    assert_eq!(secret_ref(Some("")), None);
    assert_eq!(secret_ref(Some("secret:")), None);
    assert_eq!(secret_ref(Some("gsk_live_key")), None);
    assert_eq!(secret_ref(Some("Secret:groq")), None);
    assert_eq!(secret_ref(Some(" secret:groq")), None);
}

// ── SecretStore ─────────────────────────────────────────────────────────────

#[test]
fn starts_empty_when_there_is_no_file_without_creating_one() {
    let d = dir();
    let s = store(&d.file, fake_cipher(true));
    s.load();
    assert!(s.ids().is_empty());
    assert!(!s.has("groq"));
    assert_eq!(s.get("groq"), None);
    assert!(!d.file.exists());
}

#[test]
fn stores_reads_back_and_lists_secrets() {
    let d = dir();
    let s = store(&d.file, fake_cipher(true));
    s.load();
    s.set("groq", "gsk_abc123").unwrap();
    s.set("openai", "sk-ünïcode-🔑").unwrap();
    assert!(s.has("groq"));
    assert_eq!(s.get("groq").as_deref(), Some("gsk_abc123"));
    assert_eq!(s.get("openai").as_deref(), Some("sk-ünïcode-🔑"));
    assert_eq!(sorted(s.ids()), ["groq", "openai"]);
}

#[test]
fn persists_across_instances() {
    let d = dir();
    let first = store(&d.file, fake_cipher(true));
    first.load();
    first.set("groq", "gsk_abc123").unwrap();

    let second = store(&d.file, fake_cipher(true));
    assert!(!second.has("groq"));
    second.load();
    assert_eq!(second.get("groq").as_deref(), Some("gsk_abc123"));
}

#[test]
fn writes_an_envelope_of_separately_encrypted_values_and_never_the_plaintext() {
    let d = dir();
    let cipher = fake_cipher(true);
    let s = store(&d.file, cipher.clone());
    s.load();
    s.set("groq", "gsk_abc123").unwrap();
    s.set("openai", "sk-xyz789").unwrap();

    let text = fs::read_to_string(&d.file).unwrap();
    assert!(!text.contains("gsk_abc123"));
    assert!(!text.contains("sk-xyz789"));
    let envelope: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["v"], 1);
    let items = envelope["items"].as_object().unwrap();
    assert_eq!(sorted(items.keys().cloned().collect()), ["groq", "openai"]);
    let decoded = |id: &str| STANDARD.decode(items[id].as_str().unwrap()).unwrap();
    assert_eq!(decoded("groq"), cipher.encrypt("gsk_abc123").unwrap());
    assert_eq!(decoded("openai"), cipher.encrypt("sk-xyz789").unwrap());
    assert!(!with_suffix(&d.file, ".tmp").exists());
    // Exactly what `JSON.stringify` writes.
    assert!(text.starts_with(r#"{"v":1,"items":{"groq":""#), "{text}");
}

#[test]
fn replaces_a_value() {
    let d = dir();
    let s = store(&d.file, fake_cipher(true));
    s.load();
    s.set("groq", "old").unwrap();
    s.set("groq", "new").unwrap();
    assert_eq!(s.get("groq").as_deref(), Some("new"));
    assert_eq!(s.ids(), ["groq"]);
}

#[test]
fn deletes_with_delete_and_with_an_empty_string() {
    let d = dir();
    let s = store(&d.file, fake_cipher(true));
    s.load();
    s.set("groq", "a").unwrap();
    s.set("openai", "b").unwrap();
    s.delete("groq").unwrap();
    s.set("openai", "").unwrap();
    s.delete("never-existed").unwrap();
    assert!(s.ids().is_empty());

    let reloaded = store(&d.file, fake_cipher(true));
    reloaded.load();
    assert!(reloaded.ids().is_empty());
}

#[test]
fn refuses_to_store_anything_when_encryption_is_unavailable() {
    let d = dir();
    let s = store(&d.file, fake_cipher(false));
    s.load();
    let error = s.set("groq", "gsk_abc123").unwrap_err();
    assert!(error.to_string().contains("Secure storage is not available"), "{error}");
    assert!(!s.has("groq"));
    assert!(!d.file.exists());
}

#[test]
fn can_still_delete_when_encryption_is_unavailable() {
    let d = dir();
    let cipher = fake_cipher(true);
    let s = store(&d.file, cipher.clone());
    s.load();
    s.set("groq", "a").unwrap();
    cipher.available.store(false, Ordering::SeqCst);
    s.set("groq", "").unwrap();
    assert!(!s.has("groq"));
}

#[test]
fn rejects_an_empty_id() {
    let d = dir();
    let s = store(&d.file, fake_cipher(true));
    s.load();
    assert!(matches!(s.set("", "value"), Err(SecretError::EmptyId)));
}

#[test]
fn treats_a_corrupt_file_as_empty_and_keeps_a_copy() {
    let d = dir();
    let seeded = store(&d.file, fake_cipher(true));
    seeded.set("groq", "a").unwrap();
    for garbage in ["not json at all", r#"{"v":2,"items":{}}"#, r#"{"v":1,"items":[1]}"#, "null"] {
        fs::write(&d.file, garbage).unwrap();
        let s = store(&d.file, fake_cipher(true));
        s.load();
        assert!(s.ids().is_empty(), "{garbage}");
        assert_eq!(fs::read_to_string(with_suffix(&d.file, ".corrupt")).unwrap(), garbage);
    }
    // The store is usable again afterwards.
    let s = store(&d.file, fake_cipher(true));
    s.load();
    s.set("groq", "fresh").unwrap();
    let reloaded = store(&d.file, fake_cipher(true));
    reloaded.load();
    assert_eq!(reloaded.get("groq").as_deref(), Some("fresh"));
}

#[test]
fn returns_undefined_for_a_value_that_no_longer_decrypts() {
    let d = dir();
    let s = store(&d.file, fake_cipher(true));
    s.load();
    s.set("groq", "a").unwrap();
    // For example the file was copied from another Windows account.
    let envelope = serde_json::json!({ "v": 1, "items": { "groq": STANDARD.encode("xxxxxx") } });
    fs::write(&d.file, envelope.to_string()).unwrap();
    let other = store(&d.file, fake_cipher(true));
    other.load();
    assert!(other.has("groq"));
    assert_eq!(other.get("groq"), None);
}

#[test]
fn loads_before_the_first_write_so_existing_secrets_are_not_lost() {
    let d = dir();
    let first = store(&d.file, fake_cipher(true));
    first.load();
    first.set("groq", "a").unwrap();

    let second = store(&d.file, fake_cipher(true));
    second.set("openai", "b").unwrap(); // no load() call
    let third = store(&d.file, fake_cipher(true));
    third.load();
    assert_eq!(sorted(third.ids()), ["groq", "openai"]);
}

#[test]
fn keeps_every_value_when_writes_overlap() {
    let d = dir();
    let s = store(&d.file, fake_cipher(true));
    s.load();
    std::thread::scope(|scope| {
        for i in 0..12 {
            let s = &s;
            scope.spawn(move || s.set(&format!("id{i}"), &format!("value{i}")).unwrap());
        }
    });
    let reloaded = store(&d.file, fake_cipher(true));
    reloaded.load();
    assert_eq!(reloaded.ids().len(), 12);
    assert_eq!(reloaded.get("id7").as_deref(), Some("value7"));
}

// ── The system cipher ───────────────────────────────────────────────────────

#[test]
fn system_cipher_round_trips_through_bare_dpapi() {
    let root = tempfile::tempdir().unwrap();
    let cipher = system_cipher(root.path());
    assert!(cipher.is_available());
    let data = cipher.encrypt("gsk_abc123 ünïcode 🔑").unwrap();
    // A DPAPI blob (version 1), readable by Electron's safeStorage too.
    assert!(data.starts_with(&[1, 0, 0, 0]));
    assert!(!data.windows(10).any(|w| w == b"gsk_abc123"));
    assert_eq!(cipher.decrypt(&data).unwrap(), "gsk_abc123 ünïcode 🔑");
    // Tampered ciphertext does not decrypt.
    let mut tampered = data.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;
    assert!(cipher.decrypt(&tampered).is_err());
    assert!(cipher.decrypt(b"garbage").is_err());
}

#[test]
fn system_cipher_reads_a_bare_dpapi_blob() {
    let root = tempfile::tempdir().unwrap();
    let blob = dpapi::protect("older format".as_bytes()).unwrap();
    assert_eq!(system_cipher(root.path()).decrypt(&blob).unwrap(), "older format");
}

/// Writes `Local State` the way Chromium does: a random AES key, DPAPI-protected,
/// prefixed with `DPAPI` and base64-encoded under `os_crypt.encrypted_key`.
fn fake_local_state(dir: &Path, key: &[u8; 32]) {
    let mut blob = b"DPAPI".to_vec();
    blob.extend(dpapi::protect(key).unwrap());
    let state = serde_json::json!({
        "os_crypt": { "audit_enabled": true, "encrypted_key": STANDARD.encode(blob) },
        "profile": {}
    });
    fs::write(dir.join("Local State"), state.to_string()).unwrap();
}

/// `safeStorage.encryptString` on Windows: `v10` + nonce + AES-256-GCM(ciphertext ‖ tag).
fn safe_storage_encrypt(key: &[u8; 32], nonce: &[u8; 12], plain: &str) -> Vec<u8> {
    use aes_gcm::aead::Aead as _;
    use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};
    let mut out = b"v10".to_vec();
    out.extend_from_slice(nonce);
    out.extend(Aes256Gcm::new(key.into()).encrypt(Nonce::from_slice(nonce), plain.as_bytes()).unwrap());
    out
}

#[test]
fn system_cipher_decrypts_values_written_by_electron_safe_storage() {
    let root = tempfile::tempdir().unwrap();
    let key: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11));
    fake_local_state(root.path(), &key);
    let data = safe_storage_encrypt(&key, &[7; 12], "gsk_from_electron");

    let cipher = system_cipher(root.path());
    assert_eq!(cipher.decrypt(&data).unwrap(), "gsk_from_electron");

    // A value encrypted under another key fails cleanly.
    let other = safe_storage_encrypt(&[1; 32], &[7; 12], "nope");
    assert!(cipher.decrypt(&other).is_err());
    assert!(cipher.decrypt(b"v10short").is_err());
}

#[test]
fn system_cipher_without_local_state_cannot_read_electron_values() {
    let root = tempfile::tempdir().unwrap();
    let data = safe_storage_encrypt(&[3; 32], &[7; 12], "gsk");
    assert!(system_cipher(root.path()).decrypt(&data).is_err());
}

#[test]
fn an_electron_secrets_file_keeps_working_after_the_switch() {
    let root = tempfile::tempdir().unwrap();
    let key = [42u8; 32];
    fake_local_state(root.path(), &key);
    let file = root.path().join("secrets.bin");
    let envelope = serde_json::json!({
        "v": 1,
        "items": { "groq": STANDARD.encode(safe_storage_encrypt(&key, &[9; 12], "gsk_old")) }
    });
    fs::write(&file, envelope.to_string()).unwrap();

    let s = SecretStore::new(file.clone(), system_cipher(root.path()));
    s.load();
    assert_eq!(s.get("groq").as_deref(), Some("gsk_old"));
    s.set("openai", "sk-new").unwrap();

    let reloaded = SecretStore::new(file, system_cipher(root.path()));
    reloaded.load();
    assert_eq!(reloaded.get("groq").as_deref(), Some("gsk_old"));
    assert_eq!(reloaded.get("openai").as_deref(), Some("sk-new"));
}
