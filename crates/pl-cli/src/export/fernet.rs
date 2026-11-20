//! Passphrase encryption of the exports like `CryptMixin` (PBKDF2-SHA256 → Fernet).
//!
//! upstream: src/documents/management/commands/mixins.py (v3.4.2), cryptography 46
//! `fernet.py` (token = 0x80 ‖ time (u64 BE) ‖ IV ‖ AES-128-CBC/PKCS7 ‖ HMAC-SHA256, base64url
//! with padding). The exporter stores `token.hex()` (hex of the ASCII base64 representation).

use aes::Aes128;
use aes::cipher::{Array, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug)]
pub struct InvalidToken;

impl std::fmt::Display for InvalidToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InvalidToken")
    }
}

pub struct Fernet {
    signing_key: [u8; 16],
    encryption_key: [u8; 16],
}

/// `setup_crypto`: key = urlsafe_b64(PBKDF2-HMAC-SHA256(passphrase, salt, iterations, key_size)).
pub fn derive(passphrase: &str, salt_hex: &str, iterations: u32, key_size: usize) -> Result<Fernet, String> {
    let salt = hex_decode(salt_hex).ok_or_else(|| "invalid salt".to_string())?;
    let mut key = vec![0u8; key_size];
    pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt, iterations, &mut key);
    Fernet::new(&key)
}

impl Fernet {
    /// 32-byte key (first 16 bytes signature, last 16 bytes AES).
    pub fn new(key: &[u8]) -> Result<Fernet, String> {
        if key.len() != 32 {
            return Err("Fernet key must be 32 url-safe base64-encoded bytes.".into());
        }
        let mut s = [0u8; 16];
        let mut e = [0u8; 16];
        s.copy_from_slice(&key[..16]);
        e.copy_from_slice(&key[16..]);
        Ok(Fernet { signing_key: s, encryption_key: e })
    }

    pub fn encrypt_with(&self, data: &[u8], now: u64, iv: [u8; 16]) -> String {
        let cipher = Aes128::new(&Array::from(self.encryption_key));
        let pad = 16 - data.len() % 16;
        let mut buf = data.to_vec();
        buf.extend(std::iter::repeat_n(pad as u8, pad));
        let mut prev = iv;
        for chunk in buf.chunks_mut(16) {
            let mut block = [0u8; 16];
            for (i, b) in block.iter_mut().enumerate() {
                *b = chunk[i] ^ prev[i];
            }
            let mut arr = Array::from(block);
            cipher.encrypt_block(&mut arr);
            chunk.copy_from_slice(&arr);
            prev.copy_from_slice(&arr);
        }
        let mut token = Vec::with_capacity(57 + buf.len());
        token.push(0x80);
        token.extend_from_slice(&now.to_be_bytes());
        token.extend_from_slice(&iv);
        token.extend_from_slice(&buf);
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.signing_key).expect_hmac();
        mac.update(&token);
        token.extend_from_slice(&mac.finalize().into_bytes());
        base64::engine::general_purpose::URL_SAFE.encode(token)
    }

    pub fn encrypt(&self, data: &[u8]) -> String {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let iv: [u8; 16] = rand::random();
        self.encrypt_with(data, now, iv)
    }

    pub fn decrypt(&self, token_b64: &[u8]) -> Result<Vec<u8>, InvalidToken> {
        let data = base64::engine::general_purpose::URL_SAFE.decode(token_b64).map_err(|_| InvalidToken)?;
        if data.len() < 57 || data[0] != 0x80 || (data.len() - 57) % 16 != 0 || data.len() == 57 {
            return Err(InvalidToken);
        }
        let (body, sig) = data.split_at(data.len() - 32);
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.signing_key).map_err(|_| InvalidToken)?;
        mac.update(body);
        mac.verify_slice(sig).map_err(|_| InvalidToken)?;
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&body[9..25]);
        let ct = &body[25..];
        let cipher = Aes128::new(&Array::from(self.encryption_key));
        let mut out = Vec::with_capacity(ct.len());
        let mut prev = iv;
        for chunk in ct.chunks(16) {
            let mut arr = Array::from(<[u8; 16]>::try_from(chunk).map_err(|_| InvalidToken)?);
            cipher.decrypt_block(&mut arr);
            for i in 0..16 {
                out.push(arr[i] ^ prev[i]);
            }
            prev.copy_from_slice(chunk);
        }
        let pad = *out.last().ok_or(InvalidToken)? as usize;
        if pad == 0 || pad > 16 || out.len() < pad || !out[out.len() - pad..].iter().all(|b| *b as usize == pad) {
            return Err(InvalidToken);
        }
        out.truncate(out.len() - pad);
        Ok(out)
    }

    /// `encrypt_string`: `fernet.encrypt(value.encode()).hex()`.
    pub fn encrypt_string(&self, value: &str) -> String {
        hex_encode(self.encrypt(value.as_bytes()).as_bytes())
    }

    /// `decrypt_string`: `fernet.decrypt(bytes.fromhex(value)).decode()`.
    pub fn decrypt_string(&self, value: &str) -> Result<String, InvalidToken> {
        let token = hex_decode(value).ok_or(InvalidToken)?;
        String::from_utf8(self.decrypt(&token)?).map_err(|_| InvalidToken)
    }
}

trait ExpectHmac {
    fn expect_hmac(self) -> HmacSha256;
}

impl ExpectHmac for Result<HmacSha256, hmac::digest::InvalidLength> {
    fn expect_hmac(self) -> HmacSha256 {
        // HMAC accepts any key length; the error case cannot occur.
        match self {
            Ok(m) => m,
            Err(_) => unreachable!("HMAC accepts keys of any length"),
        }
    }
}

pub fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `bytes.fromhex` (whitespace between bytes allowed).
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(2) {
        return None;
    }
    clean
        .chunks(2)
        .map(|c| std::str::from_utf8(c).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fernet_spec_vector() {
        // https://github.com/fernet/spec/blob/master/generate.json
        let key = base64::engine::general_purpose::URL_SAFE.decode("cw_0x689RpI-jtRR7oE8h_eQsKImvJapLeSbXpwF4e4=").unwrap();
        let f = Fernet::new(&key).unwrap();
        let iv: [u8; 16] = (0u8..16).collect::<Vec<_>>().try_into().unwrap();
        let tok = f.encrypt_with(b"hello", 499162800, iv);
        assert_eq!(tok, "gAAAAAAdwJ6wAAECAwQFBgcICQoLDA0ODy021cpGVWKZ_eEwCGM4BLLF_5CV9dOPmrhuVUPgJobwOz7JcbmrR64jVmpU4IwqDA==");
        assert_eq!(f.decrypt(tok.as_bytes()).unwrap(), b"hello");
        assert!(f.decrypt(b"gAAAAAAdwJ6xAAECAwQFBgcICQoLDA0OD3HkMATM5lFqGaerZ-fWPAl1-szkFVzXTuGb4hR8AKtwcaX1YdykQUFBQUFBQUFBQQ==").is_err());
    }

    #[test]
    fn roundtrip_with_passphrase() {
        let f = derive("secret", "00112233445566778899aabbccddeeff", 1000, 32).unwrap();
        let enc = f.encrypt_string("fake-mail-password");
        assert_eq!(f.decrypt_string(&enc).unwrap(), "fake-mail-password");
        let g = derive("other", "00112233445566778899aabbccddeeff", 1000, 32).unwrap();
        assert!(g.decrypt_string(&enc).is_err());
    }
}
