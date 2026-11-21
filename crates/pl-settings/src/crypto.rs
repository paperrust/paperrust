//! Encryption of the secrets in `paperrust_setting` (docs/SETTINGS.md §2.3, E6).
//!
//! * XChaCha20-Poly1305, key = HKDF-SHA256(IKM = `SECRET_KEY`, salt = `paperrust-settings-v1`,
//!   info = `settings`), AAD = key name (a ciphertext cannot be moved under a different
//!   key).
//! * `secret_ct` = nonce (24 bytes) ‖ ciphertext; `secret_kid` = hex of the first 8 bytes of
//!   SHA-256(derived key) – detects a changed `SECRET_KEY`.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::{Digest, Sha256};

const SALT: &[u8] = b"paperrust-settings-v1";
const INFO: &[u8] = b"settings";
const NONCE_LEN: usize = 24;

/// Key for secrets (derived from `SECRET_KEY`). `Debug` shows only the identifier.
#[derive(Clone)]
pub struct SecretBox {
    key: [u8; 32],
    kid: String,
}

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretBox").field("kid", &self.kid).finish()
    }
}

/// Error when decrypting.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// Different `SECRET_KEY` (identifier does not match) – value counts as not set.
    #[error("secret was encrypted with a different SECRET_KEY (kid {0})")]
    WrongKey(String),
    /// Ciphertext damaged or stored under a different key name.
    #[error("secret cannot be decrypted (corrupt or moved)")]
    Corrupt,
}

impl SecretBox {
    /// Derives the key from `SECRET_KEY`.
    pub fn from_secret_key(secret_key: &str) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(SALT), secret_key.as_bytes());
        let mut key = [0u8; 32];
        hk.expand(INFO, &mut key).expect("32 bytes is a valid HKDF-SHA256 output length");
        let digest = Sha256::digest(key);
        let kid = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        Self { key, kid }
    }

    /// Identifier of the derived key (`secret_kid`).
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// Encrypts `plain` for the settings key `name` (AAD).
    pub fn seal(&self, name: &str, plain: &str) -> Vec<u8> {
        let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&self.key));
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain.as_bytes(), aad: name.as_bytes() })
            .expect("XChaCha20-Poly1305 encryption cannot fail for in-memory buffers");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        out
    }

    /// Decrypts; checks the identifier first.
    pub fn open(&self, name: &str, kid: &str, data: &[u8]) -> Result<String, CryptoError> {
        if kid != self.kid {
            return Err(CryptoError::WrongKey(kid.to_string()));
        }
        if data.len() < NONCE_LEN {
            return Err(CryptoError::Corrupt);
        }
        let (nonce, ct) = data.split_at(NONCE_LEN);
        let cipher = XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(&self.key));
        let plain =
            cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: name.as_bytes() }).map_err(|_| CryptoError::Corrupt)?;
        String::from_utf8(plain).map_err(|_| CryptoError::Corrupt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let b = SecretBox::from_secret_key("s3cret-key-for-tests");
        let ct = b.seal("PAPERLESS_EMAIL_HOST_PASSWORD", "hunter2");
        assert!(!ct.windows(7).any(|w| w == b"hunter2"));
        assert_eq!(b.open("PAPERLESS_EMAIL_HOST_PASSWORD", b.kid(), &ct).unwrap(), "hunter2");
        // new nonce every time
        assert_ne!(ct, b.seal("PAPERLESS_EMAIL_HOST_PASSWORD", "hunter2"));
        assert_eq!(b.kid().len(), 16);
        assert_eq!(b.kid(), SecretBox::from_secret_key("s3cret-key-for-tests").kid());
    }

    #[test]
    fn wrong_key_and_moved_ciphertext() {
        let a = SecretBox::from_secret_key("key-a");
        let b = SecretBox::from_secret_key("key-b");
        let ct = a.seal("K", "v");
        assert_eq!(b.open("K", a.kid(), &ct), Err(CryptoError::WrongKey(a.kid().to_string())));
        // same identifier faked → AEAD fails
        let forged = SecretBox { key: b.key, kid: a.kid().to_string() };
        assert_eq!(forged.open("K", a.kid(), &ct), Err(CryptoError::Corrupt));
        // AAD: not readable under a different name
        assert_eq!(a.open("OTHER", a.kid(), &ct), Err(CryptoError::Corrupt));
        assert_eq!(a.open("K", a.kid(), &ct[..10]), Err(CryptoError::Corrupt));
    }
}
