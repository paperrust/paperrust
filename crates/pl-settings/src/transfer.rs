//! Export with secrets, encrypted with a passphrase (docs/SETTINGS.md §2.7).
//!
//! Format (JSON): `{"format":"paperrust-settings-encrypted","version":1,"kdf":"pbkdf2-sha256",
//! "iterations":N,"salt":B64,"nonce":B64,"ciphertext":B64}`; key = PBKDF2-HMAC-SHA256
//! (passphrase, 16 bytes salt, 600,000 rounds), cipher XChaCha20-Poly1305 over the plaintext export
//! ([`crate::store::export`]). Deviation from upstream (`document_exporter --passphrase`: Fernet per
//! field): one envelope for the whole file; paperless-ngx does not know this file anyway.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde_json::{Value, json};

pub const FORMAT: &str = "paperrust-settings-encrypted";
const ITERATIONS: u32 = 600_000;

fn b64(d: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(d)
}

fn unb64(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|_| "invalid base64".to_string())
}

fn key(passphrase: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(passphrase.as_bytes(), salt, iterations, &mut k);
    k
}

/// Encrypts an export.
pub fn encrypt(doc: &Value, passphrase: &str) -> Result<Value, String> {
    if passphrase.chars().count() < 8 {
        return Err("passphrase must have at least 8 characters".into());
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);
    let k = key(passphrase, &salt, ITERATIONS);
    let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&k));
    let plain = serde_json::to_vec(doc).map_err(|e| e.to_string())?;
    let ct = cipher.encrypt(XNonce::from_slice(&nonce), plain.as_slice()).map_err(|_| "encryption failed".to_string())?;
    Ok(json!({
        "format": FORMAT, "version": 1, "kdf": "pbkdf2-sha256", "iterations": ITERATIONS,
        "salt": b64(&salt), "nonce": b64(&nonce), "ciphertext": b64(&ct),
    }))
}

/// Is this an encrypted export?
pub fn is_encrypted(doc: &Value) -> bool {
    doc.get("format").and_then(Value::as_str) == Some(FORMAT)
}

/// Decrypts an export (wrong passphrase → error).
pub fn decrypt(doc: &Value, passphrase: &str) -> Result<Value, String> {
    let s = |k: &str| doc.get(k).and_then(Value::as_str).ok_or_else(|| format!("missing field {k}"));
    let iterations = doc.get("iterations").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).unwrap_or(ITERATIONS);
    if !(10_000..=10_000_000).contains(&iterations) {
        return Err("unsupported iteration count".into());
    }
    let salt = unb64(s("salt")?)?;
    let nonce = unb64(s("nonce")?)?;
    if nonce.len() != 24 {
        return Err("invalid nonce".into());
    }
    let ct = unb64(s("ciphertext")?)?;
    let k = key(passphrase, &salt, iterations);
    let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&k));
    let plain = cipher.decrypt(XNonce::from_slice(&nonce), ct.as_slice()).map_err(|_| "wrong passphrase or damaged file".to_string())?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let doc = json!({"format":"paperrust-settings","settings":{"PAPERLESS_EMAIL_HOST_PASSWORD":"hunter2"}});
        let enc = encrypt(&doc, "correct horse").unwrap();
        assert!(is_encrypted(&enc));
        assert!(!enc.to_string().contains("hunter2"));
        assert_eq!(decrypt(&enc, "correct horse").unwrap(), doc);
        assert!(decrypt(&enc, "wrong horse!").unwrap_err().contains("wrong passphrase"));
        assert!(encrypt(&doc, "short").is_err());
    }
}
