//! Encryption with [age](https://age-encryption.org) (crate `age`, X25519 + ChaCha20-Poly1305).
//!
//! Key model:
//! * On setup (`backup init`) a random X25519 key of the backup target is created.
//! * The **public** part (`age1…`) is stored in plaintext in `config.json` at the target and locally in
//!   `DATA_DIR/backup/repo.json`. It is enough for backing up: the server cannot decrypt
//!   its own backups.
//! * The **secret** part is stored at the target only, encrypted with the user's passphrase
//!   (`keys/repo.key.age`, age-scrypt). For restoring, the passphrase is needed (or the
//!   key file shown at setup).

use std::io::{Read, Write};
use std::path::Path;

use age::secrecy::{ExposeSecret, SecretString};

/// New key: (secret key `AGE-SECRET-KEY-1…`, public `age1…`).
pub fn generate() -> (SecretString, String) {
    let id = age::x25519::Identity::generate();
    let public = id.to_public().to_string();
    (id.to_string(), public)
}

pub fn parse_recipient(s: &str) -> Result<age::x25519::Recipient, String> {
    s.trim().parse::<age::x25519::Recipient>().map_err(|e| format!("invalid age recipient: {e}"))
}

pub fn parse_identity(s: &str) -> Result<age::x25519::Identity, String> {
    // key file: comment lines (`# …`) allowed
    let line = s.lines().map(str::trim).find(|l| l.starts_with("AGE-SECRET-KEY-")).ok_or("no AGE-SECRET-KEY in key file")?;
    line.parse::<age::x25519::Identity>().map_err(|e| format!("invalid age identity: {e}"))
}

/// Encrypt a stream to a recipient.
pub fn encrypt_stream(recipient: &age::x25519::Recipient, mut input: impl Read, output: impl Write) -> Result<u64, String> {
    let enc = age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient)).map_err(|e| e.to_string())?;
    let mut w = enc.wrap_output(output).map_err(|e| e.to_string())?;
    let n = std::io::copy(&mut input, &mut w).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(n)
}

/// Decrypt a stream with the target's key.
pub fn decrypt_stream(identity: &age::x25519::Identity, input: impl Read, mut output: impl Write) -> Result<u64, String> {
    let dec = age::Decryptor::new(input).map_err(|e| e.to_string())?;
    let mut r = dec.decrypt(std::iter::once(identity as &dyn age::Identity)).map_err(|e| e.to_string())?;
    std::io::copy(&mut r, &mut output).map_err(|e| e.to_string())
}

pub fn encrypt_bytes(recipient: &age::x25519::Recipient, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len() + 256);
    encrypt_stream(recipient, data, &mut out)?;
    Ok(out)
}

pub fn decrypt_bytes(identity: &age::x25519::Identity, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(data.len());
    decrypt_stream(identity, data, &mut out)?;
    Ok(out)
}

/// Encrypt the secret key with the passphrase (age-scrypt). `work_factor` = log2(N) of
/// scrypt; `None` = age default (~1 s in a release build). Tests use small values.
pub fn wrap_identity(identity: &SecretString, passphrase: &str, work_factor: Option<u8>) -> Result<Vec<u8>, String> {
    let mut r = age::scrypt::Recipient::new(SecretString::from(passphrase.to_string()));
    if let Some(f) = work_factor {
        r.set_work_factor(f);
    }
    let enc = age::Encryptor::with_recipients(std::iter::once(&r as &dyn age::Recipient)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut w = enc.wrap_output(&mut out).map_err(|e| e.to_string())?;
    w.write_all(identity.expose_secret().as_bytes()).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Decrypt the secret key with the passphrase.
pub fn unwrap_identity(wrapped: &[u8], passphrase: &str) -> Result<age::x25519::Identity, String> {
    let dec = age::Decryptor::new(wrapped).map_err(|e| e.to_string())?;
    let id = age::scrypt::Identity::new(SecretString::from(passphrase.to_string()));
    let mut r = dec.decrypt(std::iter::once(&id as &dyn age::Identity)).map_err(|_| "wrong passphrase or damaged key file".to_string())?;
    let mut s = String::new();
    r.read_to_string(&mut s).map_err(|e| e.to_string())?;
    parse_identity(&s)
}

/// Encrypt a file to a recipient (result in `out`). Returns (encrypted size,
/// SHA-256 of the plaintext that was read): this way a change during reading is noticed.
pub fn encrypt_file(recipient: &age::x25519::Recipient, src: &Path, out: &Path) -> Result<(u64, String), String> {
    struct Hashing<R> {
        inner: R,
        h: sha2::Sha256,
    }
    impl<R: Read> Read for Hashing<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            sha2::Digest::update(&mut self.h, &buf[..n]);
            Ok(n)
        }
    }
    let f = std::fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let o = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut w = std::io::BufWriter::new(o);
    let mut r = Hashing { inner: std::io::BufReader::new(f), h: <sha2::Sha256 as sha2::Digest>::new() };
    encrypt_stream(recipient, &mut r, &mut w)?;
    w.flush().map_err(|e| e.to_string())?;
    let sha = crate::guard::hex(&sha2::Digest::finalize(r.h));
    Ok((std::fs::metadata(out).map_err(|e| e.to_string())?.len(), sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let (secret, public) = generate();
        assert!(public.starts_with("age1"));
        let r = parse_recipient(&public).unwrap();
        let id = parse_identity(secret.expose_secret()).unwrap();
        let ct = encrypt_bytes(&r, b"hello archive").unwrap();
        assert!(!ct.windows(5).any(|w| w == b"hello"));
        assert_eq!(decrypt_bytes(&id, &ct).unwrap(), b"hello archive");
        let other = age::x25519::Identity::generate();
        assert!(decrypt_bytes(&other, &ct).is_err());
    }

    #[test]
    fn passphrase_wrap() {
        let (secret, public) = generate();
        let wrapped = wrap_identity(&secret, "correct horse", Some(10)).unwrap();
        let id = unwrap_identity(&wrapped, "correct horse").unwrap();
        assert_eq!(id.to_public().to_string(), public);
        assert!(unwrap_identity(&wrapped, "wrong").is_err());
    }
}
