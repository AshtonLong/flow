//! API keys, encrypted one by one and kept in `secrets.bin`. `config.toml` only
//! holds references such as `api_key = "secret:groq"`. The cipher is injected
//! ([`system_cipher`] in the app) so the store itself is plain file handling.
//! PORT: src/main/secrets/store.ts
//!
//! The file is the Electron build's envelope, `{"v":1,"items":{"<id>":"<base64>"}}`,
//! and [`system_cipher`] also decrypts the values Electron's `safeStorage` wrote,
//! so a user switching builds keeps their keys.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde_json::{Map, Value};

use crate::fs_util::{with_suffix, write_file_atomic};

/// Encrypts and decrypts single values. Stands in for Electron's `safeStorage`.
pub trait SecretCipher: Send + Sync {
    fn is_available(&self) -> bool;
    /// `safeStorage.encryptString`
    fn encrypt(&self, plain: &str) -> Result<Vec<u8>, SecretError>;
    /// `safeStorage.decryptString`
    fn decrypt(&self, data: &[u8]) -> Result<String, SecretError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("A secret needs an id.")]
    EmptyId,
    /// Refused rather than fall back to storing the key in the clear.
    #[error("Secure storage is not available on this system, so the key was not saved.")]
    Unavailable,
    #[error("{0}")]
    Cipher(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

const REF_PREFIX: &str = "secret:";

/// `"secret:groq"` → `"groq"`; anything else → `None`.
pub fn secret_ref(value: Option<&str>) -> Option<&str> {
    value?.strip_prefix(REF_PREFIX).filter(|id| !id.is_empty())
}

/// Decodes base64 the way Node's `Buffer.from(text, 'base64')` does: both
/// alphabets, padding optional, other characters skipped, stopping at `=`.
fn decode_base64_loose(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
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

fn parse_envelope(text: &str) -> Option<IndexMap<String, Vec<u8>>> {
    let data: Value = serde_json::from_str(text).ok()?;
    let items = match (data.get("v").and_then(Value::as_f64), data.get("items")) {
        (Some(1.0), Some(Value::Object(items))) => items,
        _ => return None, // "Unrecognised secrets file"
    };
    let mut out = IndexMap::new();
    for (id, encoded) in items {
        out.insert(id.clone(), decode_base64_loose(encoded.as_str()?));
    }
    Some(out)
}

#[derive(Default)]
struct Items {
    /// Ciphertext only: values are decrypted on demand and never cached in the clear.
    items: IndexMap<String, Vec<u8>>,
    loaded: bool,
}

/// The `secrets.bin` store. Thread-safe; writes happen one at a time.
pub struct SecretStore {
    path: PathBuf,
    cipher: Arc<dyn SecretCipher>,
    state: Mutex<Items>,
}

impl SecretStore {
    pub fn new(path: PathBuf, cipher: Arc<dyn SecretCipher>) -> Self {
        Self { path, cipher, state: Mutex::new(Items::default()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the file. Never fails: a corrupt file is copied to `<file>.corrupt` and ignored.
    pub fn load(&self) {
        self.load_into(&mut self.state.lock());
    }

    fn load_into(&self, state: &mut Items) {
        state.loaded = true;
        let text = match fs::read(&self.path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => {
                // Missing or unreadable: start empty.
                state.items.clear();
                return;
            }
        };
        match parse_envelope(&text) {
            Some(items) => state.items = items,
            None => {
                state.items.clear();
                // Kept for recovery, since the next `set` overwrites the original.
                if let Err(error) = fs::copy(&self.path, with_suffix(&self.path, ".corrupt")) {
                    log::warn!(target: "secrets", "could not keep a copy of the unreadable secrets file: {error}");
                }
                log::warn!(target: "secrets", "secrets file is unreadable; starting empty");
            }
        }
    }

    pub fn has(&self, id: &str) -> bool {
        self.state.lock().items.contains_key(id)
    }

    /// The decrypted secret, or `None` if it is missing or cannot be decrypted. Main process only.
    pub fn get(&self, id: &str) -> Option<String> {
        let data = self.state.lock().items.get(id)?.clone();
        self.cipher.decrypt(&data).ok()
    }

    /// Stores a secret; an empty string deletes it. Fails if encryption is unavailable.
    pub fn set(&self, id: &str, value: &str) -> Result<(), SecretError> {
        if value.is_empty() {
            return self.delete(id);
        }
        if id.is_empty() {
            return Err(SecretError::EmptyId);
        }
        if !self.cipher.is_available() {
            // Refuse rather than fall back to storing the key in the clear.
            return Err(SecretError::Unavailable);
        }
        let data = self.cipher.encrypt(value)?;
        self.mutate(|items| {
            items.insert(id.to_string(), data);
            true
        })
    }

    pub fn delete(&self, id: &str) -> Result<(), SecretError> {
        self.mutate(|items| items.shift_remove(id).is_some())
    }

    pub fn ids(&self) -> Vec<String> {
        self.state.lock().items.keys().cloned().collect()
    }

    /// Applies a change and writes the file, one change at a time. `change` returns
    /// false when it changed nothing, and then nothing is written.
    fn mutate(&self, change: impl FnOnce(&mut IndexMap<String, Vec<u8>>) -> bool) -> Result<(), SecretError> {
        let mut state = self.state.lock();
        // A store that was never loaded would otherwise overwrite the secrets already on disk.
        if !state.loaded {
            self.load_into(&mut state);
        }
        if !change(&mut state.items) {
            return Ok(());
        }
        let items: Map<String, Value> =
            state.items.iter().map(|(id, data)| (id.clone(), Value::from(STANDARD.encode(data)))).collect();
        let mut envelope = Map::new();
        envelope.insert("v".into(), Value::from(1));
        envelope.insert("items".into(), Value::Object(items));
        let text = Value::Object(envelope).to_string();
        write_file_atomic(&self.path, text.as_bytes())?;
        Ok(())
    }
}

// ── The system cipher ──────────────────────────────────────────────────────

/// Tag on values Electron's `safeStorage` (Chromium `OSCrypt`) encrypted: a
/// 12-byte nonce and the AES-256-GCM ciphertext with its 16-byte tag follow it.
const SAFE_STORAGE_PREFIX: &[u8] = b"v10";
const SAFE_STORAGE_NONCE: usize = 12;
const SAFE_STORAGE_TAG: usize = 16;
/// Prefix of `os_crypt.encrypted_key` in `Local State` once base64-decoded.
const LOCAL_STATE_KEY_PREFIX: &[u8] = b"DPAPI";

/// The cipher the app uses: Windows DPAPI for the current user. It also reads the
/// values the Electron build wrote with `safeStorage`, whose AES key is kept,
/// DPAPI-protected, in `<user_data_dir>\Local State`.
pub fn system_cipher(user_data_dir: &Path) -> Arc<dyn SecretCipher> {
    Arc::new(SystemCipher {
        local_state: user_data_dir.join("Local State"),
        available: OnceLock::new(),
        legacy_key: OnceLock::new(),
    })
}

struct SystemCipher {
    local_state: PathBuf,
    available: OnceLock<bool>,
    legacy_key: OnceLock<Option<[u8; 32]>>,
}

impl SystemCipher {
    /// The Electron build's AES key: `CryptUnprotectData` of `os_crypt.encrypted_key`
    /// without its `DPAPI` prefix. Read once.
    fn legacy_key(&self) -> Option<&[u8; 32]> {
        self.legacy_key
            .get_or_init(|| {
                let key = read_legacy_key(&self.local_state);
                if key.is_none() {
                    log::warn!(target: "secrets", "no usable key in {}", self.local_state.display());
                }
                key
            })
            .as_ref()
    }

    fn decrypt_safe_storage(&self, data: &[u8]) -> Result<Vec<u8>, SecretError> {
        use aes_gcm::aead::Aead as _;
        use aes_gcm::{Aes256Gcm, KeyInit as _, Nonce};
        let key = self.legacy_key().ok_or_else(|| {
            SecretError::Cipher("The key the Electron build stored in Local State is unavailable.".into())
        })?;
        if data.len() < SAFE_STORAGE_NONCE + SAFE_STORAGE_TAG {
            return Err(SecretError::Cipher("The stored value is too short.".into()));
        }
        let (nonce, ciphertext) = data.split_at(SAFE_STORAGE_NONCE);
        Aes256Gcm::new(key.into())
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| SecretError::Cipher("The stored value could not be decrypted.".into()))
    }
}

fn read_legacy_key(local_state: &Path) -> Option<[u8; 32]> {
    let text = fs::read_to_string(local_state).ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    let encoded = json.get("os_crypt")?.get("encrypted_key")?.as_str()?;
    let blob = STANDARD.decode(encoded).ok()?;
    let protected = blob.strip_prefix(LOCAL_STATE_KEY_PREFIX)?;
    let key = dpapi::unprotect(protected).ok()?;
    <[u8; 32]>::try_from(key.as_slice()).ok()
}

impl SecretCipher for SystemCipher {
    fn is_available(&self) -> bool {
        *self.available.get_or_init(|| dpapi::protect(b"flow").is_ok())
    }

    /// A bare DPAPI blob: Chromium's format before "v10", which Electron's
    /// `safeStorage` still decrypts, so keys saved here survive going back to the
    /// Electron build. DPAPI blobs start with a version dword, never "v10".
    fn encrypt(&self, plain: &str) -> Result<Vec<u8>, SecretError> {
        dpapi::protect(plain.as_bytes())
    }

    fn decrypt(&self, data: &[u8]) -> Result<String, SecretError> {
        let plain = if let Some(rest) = data.strip_prefix(SAFE_STORAGE_PREFIX) {
            self.decrypt_safe_storage(rest)?
        } else {
            dpapi::unprotect(data)?
        };
        String::from_utf8(plain).map_err(|_| SecretError::Cipher("The stored value is not text.".into()))
    }
}

/// Windows DPAPI for the current user (`CryptProtectData` / `CryptUnprotectData`).
pub(crate) mod dpapi {
    use super::SecretError;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::w;

    fn blob_of(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, SecretError> {
        let len = u32::try_from(data.len()).map_err(|_| SecretError::Cipher("The value is too large.".into()))?;
        Ok(CRYPT_INTEGER_BLOB { cbData: len, pbData: data.as_ptr().cast_mut() })
    }

    /// Copies out and frees a blob the system allocated.
    fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if blob.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: on success the API returns `cbData` readable bytes at `pbData`,
        // allocated with LocalAlloc; they are copied before being freed exactly once.
        unsafe {
            let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
            LocalFree(Some(HLOCAL(blob.pbData.cast())));
            bytes
        }
    }

    pub(crate) fn protect(data: &[u8]) -> Result<Vec<u8>, SecretError> {
        let input = blob_of(data)?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` points at `data`, which outlives the call; the API only reads it.
        unsafe { CryptProtectData(&input, w!(""), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output) }
            .map_err(|error| SecretError::Cipher(format!("Encryption failed: {}", error.message())))?;
        Ok(take(output))
    }

    pub(crate) fn unprotect(data: &[u8]) -> Result<Vec<u8>, SecretError> {
        let input = blob_of(data)?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as above.
        unsafe { CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output) }
            .map_err(|error| SecretError::Cipher(format!("Decryption failed: {}", error.message())))?;
        Ok(take(output))
    }
}

#[cfg(test)]
mod tests;
