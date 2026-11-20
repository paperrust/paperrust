//! TOTP and recovery codes exactly like django-allauth's MFA module.
//!
//! upstream: allauth 65.20.1 – mfa/totp/internal/auth.py (generate_totp_secret, hotp_value,
//! format_hotp_value, validate_totp_code, TOTP.validate_code/_mark_code_used),
//! mfa/recovery_codes/internal/auth.py (RecoveryCodes.generate_seed/generate_codes/
//! get_unused_codes), mfa/adapter.py (get_totp_label, build_totp_url),
//! core/internal/cryptokit.py (normalize_user_code, compare_user_code).
//! Settings: upstream MFA_TOTP_ISSUER = "Paperless-ngx" (paperless/settings); PaperRust uses
//! `PAPERRUST_MFA_TOTP_ISSUER`, default `pl_core::brand::PRODUCT_NAME` (A-290). All other MFA_* defaults
//! (period 30, digits 6, tolerance 0, 10 recovery codes with 8 digits).

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;

pub const TOTP_PERIOD: u64 = 30;
pub const TOTP_DIGITS: u32 = 6;
pub const TOTP_TOLERANCE: i64 = 0;
/// Default TOTP issuer (shown in authenticator apps): the product name.
pub const TOTP_ISSUER: &str = pl_core::brand::PRODUCT_NAME;
pub const RECOVERY_CODE_COUNT: usize = 10;
pub const RECOVERY_CODE_DIGITS: u32 = 8;

const B32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// `base64.b32encode` (RFC 4648, with `=` padding).
/// allauth `mfa/totp/internal/auth.py::SECRET_SESSION_KEY`.
pub const SECRET_SESSION_KEY: &str = "mfa.totp.secret";

pub fn b32encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let v = buf.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        let chars = match chunk.len() {
            1 => 2,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => 8,
        };
        for i in 0..8 {
            if i < chars {
                let idx = ((v >> (35 - i * 5)) & 0x1f) as usize;
                out.push(B32_ALPHABET[idx] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `base64.b32decode(s, casefold=True)` (Python 3.12 semantics: length must be a multiple of 8,
/// padding count in {0,1,3,4,6}, only alphabet characters). `None` = `binascii.Error`.
pub fn b32decode_casefold(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() {
        return None; // secret.encode("ascii") raises
    }
    let s = s.to_ascii_uppercase();
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(8) {
        return None;
    }
    let l = bytes.len();
    let stripped = s.trim_end_matches('=');
    let padchars = l - stripped.len();
    let mut out: Vec<u8> = Vec::with_capacity(l / 8 * 5);
    let mut acc: u64 = 0;
    for quanta in stripped.as_bytes().chunks(8) {
        acc = 0;
        for c in quanta {
            let v = B32_ALPHABET.iter().position(|x| x == c)? as u64;
            acc = (acc << 5) + v;
        }
        out.extend_from_slice(&acc.to_be_bytes()[3..8]); // acc.to_bytes(5)
    }
    if !l.is_multiple_of(8) || ![0usize, 1, 3, 4, 6].contains(&padchars) {
        return None;
    }
    if padchars > 0 && !out.is_empty() {
        acc <<= 5 * padchars as u64;
        let last = acc.to_be_bytes();
        let leftover = (43 - 5 * padchars) / 8; // 1: 4, 3: 3, 4: 2, 6: 1
        let base = out.len() - 5;
        out.truncate(base);
        out.extend_from_slice(&last[3..3 + leftover]);
    }
    Some(out)
}

/// `generate_totp_secret(length=20)`.
pub fn generate_totp_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    b32encode(&bytes)
}

/// `hotp_value(secret, counter)`; `None` if the secret is not valid base32 (python raises).
pub fn hotp_value(secret: &str, counter: u64) -> Option<u32> {
    let key = b32decode_casefold(secret)?;
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let truncated = [digest[offset] & 0x7f, digest[offset + 1], digest[offset + 2], digest[offset + 3]];
    let value = u32::from_be_bytes(truncated);
    Some(value % 10u32.pow(TOTP_DIGITS))
}

/// `format_hotp_value`.
pub fn format_hotp_value(value: u32) -> String {
    format!("{value:0width$}", width = TOTP_DIGITS as usize)
}

/// `normalize_user_code`: drop `string.punctuation + string.whitespace`, then `lower()`.
pub fn normalize_user_code(code: &str) -> String {
    code.chars()
        .filter(|c| !(c.is_ascii_punctuation() || matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')))
        .collect::<String>()
        .to_lowercase()
}

/// `compare_user_code(actual=, expected=)`.
pub fn compare_user_code(actual: &str, expected: &str) -> bool {
    let a = normalize_user_code(actual);
    let e = normalize_user_code(expected);
    if e.is_empty() {
        return false;
    }
    // secrets.compare_digest on str raises TypeError for non-ASCII → False
    if !a.is_ascii() || !e.is_ascii() {
        return false;
    }
    constant_time_eq(a.as_bytes(), e.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Outcome of [`validate_totp_code`] when the secret itself is broken (python raises → 500).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadSecret;

/// `validate_totp_code(secret, code)` at unix time `now`.
pub fn validate_totp_code_at(secret: &str, code: &str, now: u64) -> Result<bool, BadSecret> {
    let counter = (now / TOTP_PERIOD) as i64;
    for i in -TOTP_TOLERANCE..=TOTP_TOLERANCE {
        let c = (counter + i).max(0) as u64;
        let value = hotp_value(secret, c).ok_or(BadSecret)?;
        if compare_user_code(code, &format_hotp_value(value)) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn validate_totp_code(secret: &str, code: &str) -> Result<bool, BadSecret> {
    validate_totp_code_at(secret, code, unix_now())
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `urllib.parse.quote(s, safe)`: UTF-8 percent-encoding, unreserved `A-Za-z0-9_.-~` kept.
pub fn py_quote(s: &str, safe: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        let c = *b;
        if c.is_ascii_alphanumeric() || b"_.-~".contains(&c) || safe.as_bytes().contains(&c) {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// `DefaultMFAAdapter.get_totp_label`: email, else username, else `str(user)` (= username).
pub fn totp_label(email: &str, username: &str) -> String {
    if !email.is_empty() {
        email.to_string()
    } else {
        username.to_string()
    }
}

/// Issuer: `PAPERRUST_MFA_TOTP_ISSUER` (aliases `PAPERLESS_*`), non-empty, else [`TOTP_ISSUER`].
pub fn totp_issuer() -> String {
    pl_core::config::var("PAPERRUST_MFA_TOTP_ISSUER")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| TOTP_ISSUER.to_string())
}

/// `DefaultMFAAdapter.build_totp_url(user, secret)`.
pub fn build_totp_url(label: &str, secret: &str) -> String {
    build_totp_url_with_issuer(label, secret, &totp_issuer())
}

pub fn build_totp_url_with_issuer(label: &str, secret: &str, issuer: &str) -> String {
    format!(
        "otpauth://totp/{}?secret={}&issuer={}",
        py_quote(label, "/"),
        py_quote(secret, ""),
        py_quote(issuer, "")
    )
}

/// `RecoveryCodes.generate_seed`: `secrets.token_hex(40)`.
pub fn generate_recovery_seed() -> String {
    let mut bytes = [0u8; 40];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex(&bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `RecoveryCodes.generate_codes` for a (decrypted) seed.
pub fn generate_recovery_codes(seed: &str) -> Vec<String> {
    let Ok(mut h) = <Hmac<Sha1> as Mac>::new_from_slice(seed.as_bytes()) else {
        return Vec::new();
    };
    let byte_count = ((RECOVERY_CODE_DIGITS / 2) as usize).min(20);
    let modulo = 10u64.pow(RECOVERY_CODE_DIGITS);
    let mut out = Vec::with_capacity(RECOVERY_CODE_COUNT);
    for i in 0..RECOVERY_CODE_COUNT {
        h.update(format!("{i:3},").as_bytes());
        let digest = h.clone().finalize().into_bytes();
        let value = digest[..byte_count].iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        out.push(format!("{:0width$}", value % modulo, width = RECOVERY_CODE_DIGITS as usize));
    }
    out
}

/// `RecoveryCodes.get_unused_codes` for an authenticator `data` JSON.
pub fn unused_recovery_codes(data: &serde_json::Value) -> Vec<String> {
    if let Some(migrated) = data.get("migrated_codes").and_then(|v| v.as_array()) {
        return migrated.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    }
    let seed = data.get("seed").and_then(|v| v.as_str()).unwrap_or("");
    let used = data.get("used_mask").and_then(|v| v.as_u64()).unwrap_or(0);
    generate_recovery_codes(seed)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| used & (1u64 << i) == 0)
        .map(|(_, c)| c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b32_roundtrip_and_python_rules() {
        let data = [0u8, 1, 2, 3, 250, 251, 252, 253, 254, 255, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0];
        let enc = b32encode(&data);
        assert_eq!(enc.len(), 32);
        assert_eq!(b32decode_casefold(&enc).unwrap(), data.to_vec());
        assert_eq!(b32decode_casefold(&enc.to_lowercase()).unwrap(), data.to_vec());
        // python: b32encode(b"f") == b"MY======", b"fo" == b"MZXQ===="
        assert_eq!(b32encode(b"f"), "MY======");
        assert_eq!(b32encode(b"foob"), "MZXW6YQ=");
        assert_eq!(b32decode_casefold("MY======").unwrap(), b"f".to_vec());
        assert_eq!(b32decode_casefold("MZXW6YQ=").unwrap(), b"foob".to_vec());
        assert_eq!(b32decode_casefold("MZXW6YTBOI======").unwrap(), b"foobar".to_vec());
        assert!(b32decode_casefold("ABC").is_none()); // incorrect padding
        assert!(b32decode_casefold("ABCDEFG1").is_none()); // non-base32 digit
        assert!(b32decode_casefold("AB======").is_some()); // 6 pad chars ok
        assert!(b32decode_casefold("ABCDEFG=").is_some()); // 1 pad char ok
        assert!(b32decode_casefold("ABCDEF==").is_none()); // 2 pad chars not allowed
    }

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 Appendix B, secret "12345678901234567890", 8 digits there; we use 6 (mod 10^6).
        let secret = b32encode(b"12345678901234567890");
        let cases = [(59u64, 94287082u32), (1111111109, 7081804), (1234567890, 89005924)];
        for (t, expected8) in cases {
            let v = hotp_value(&secret, t / 30).unwrap();
            assert_eq!(v, expected8 % 1_000_000, "t={t}");
            assert!(validate_totp_code_at(&secret, &format_hotp_value(v), t).unwrap());
            assert!(validate_totp_code_at(&secret, &format!(" {}-{} ", &format_hotp_value(v)[..3], &format_hotp_value(v)[3..]), t).unwrap());
        }
        assert!(validate_totp_code_at("not base32!", "123456", 59).is_err());
    }

    #[test]
    fn url_like_allauth() {
        assert_eq!(
            build_totp_url_with_issuer("test@example.com", "ABC", TOTP_ISSUER),
            "otpauth://totp/test%40example.com?secret=ABC&issuer=PaperRust"
        );
        assert_eq!(
            build_totp_url_with_issuer("a", "B", "Mein Archiv"),
            "otpauth://totp/a?secret=B&issuer=Mein%20Archiv"
        );
        assert_eq!(py_quote("Jöran Müller/x", "/"), "J%C3%B6ran%20M%C3%BCller/x");
        assert_eq!(totp_label("", "admin"), "admin");
    }

    #[test]
    fn recovery_codes_like_allauth() {
        // python: hmac.new(b"seed", None, sha1); update(f"{i:3},") cumulatively; first 4 bytes % 10**8
        let codes = generate_recovery_codes("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff0011223344556677");
        assert_eq!(codes.len(), 10);
        assert!(codes.iter().all(|c| c.len() == 8 && c.chars().all(|ch| ch.is_ascii_digit())));
        let data = serde_json::json!({"seed": "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff0011223344556677", "used_mask": 0b101});
        let unused = unused_recovery_codes(&data);
        assert_eq!(unused.len(), 8);
        assert_eq!(unused[0], codes[1]);
        let expected = include_str!("../tests/recovery_vector.txt").trim().split(',').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(codes, expected);
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_user_code(" 12-34.56\t"), "123456");
        assert!(!compare_user_code("123456", ""));
        assert!(!compare_user_code("12345ä", "12345ä"));
    }
}
