//! Django signing primitives (salted HMAC, base64/base62, TimestampSigner, signed objects).
//!
//! upstream: django/utils/crypto.py@5.2 (salted_hmac, constant_time_compare),
//!           django/core/signing.py@5.2 (b64_encode, b64_decode, b62_encode, b62_decode,
//!           base64_hmac, Signer, TimestampSigner, sign_object, unsign_object, JSONSerializer),
//!           django/contrib/sessions/backends/base.py@5.2 (SessionBase.key_salt/encode/decode),
//!           django/contrib/auth/base_user.py@5.2 (AbstractBaseUser.get_session_auth_hash).

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::io::Read as _;

type HmacSha256 = Hmac<Sha256>;

/// Salt used by `SessionStore` of `django.contrib.sessions.backends.db` / `cached_db`:
/// `"django.contrib.sessions." + self.__class__.__qualname__` (qualname is `SessionStore`).
pub const SESSION_KEY_SALT: &str = "django.contrib.sessions.SessionStore";

/// Key salt of `AbstractBaseUser._get_session_auth_hash`.
pub const SESSION_AUTH_HASH_SALT: &str =
    "django.contrib.auth.models.AbstractBaseUser.get_session_auth_hash";

/// Upper bound for decompressed signed payloads (protection only; Django has none).
const MAX_DECOMPRESSED: u64 = 16 * 1024 * 1024;

/// `django.utils.crypto.salted_hmac(key_salt, value, secret, algorithm="sha256")`.
///
/// key = sha256(key_salt + secret); HMAC-SHA256(key, value).
pub fn salted_hmac_sha256(key_salt: &str, value: &[u8], secret: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(key_salt.as_bytes());
    h.update(secret.as_bytes());
    let key = h.finalize();
    // HMAC accepts keys of any length, so this never fails; keep a non-panicking fallback.
    let Ok(mut mac) = HmacSha256::new_from_slice(&key) else {
        return [0u8; 32];
    };
    mac.update(value);
    mac.finalize().into_bytes().into()
}

/// `django.utils.crypto.constant_time_compare` (on bytes).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // hmac.compare_digest: lengths may leak, content does not.
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// `AbstractBaseUser.get_session_auth_hash()`:
/// `salted_hmac(key_salt, self.password, secret=SECRET_KEY, algorithm="sha256").hexdigest()`.
pub fn session_auth_hash(password_hash: &str, secret_key: &str) -> String {
    let d = salted_hmac_sha256(SESSION_AUTH_HASH_SALT, password_hash.as_bytes(), secret_key);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// `signing.b64_encode`: urlsafe base64 without `=` padding.
pub fn b64_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// `signing.b64_decode`: `urlsafe_b64decode(s + b"=" * (-len(s) % 4))`.
///
/// Python's non-strict decoder silently drops characters outside the alphabet;
/// signed values are produced by Django itself, so a strict decode is enough here
/// (anything else fails the signature check before it is decoded).
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).ok()
}

const B62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// `signing.b62_encode`.
pub fn b62_encode(mut n: i64) -> String {
    if n == 0 {
        return "0".into();
    }
    let sign = if n < 0 { "-" } else { "" };
    n = n.abs();
    let mut out = Vec::new();
    while n > 0 {
        out.push(B62[(n % 62) as usize]);
        n /= 62;
    }
    out.reverse();
    format!("{sign}{}", String::from_utf8_lossy(&out))
}

/// `signing.b62_decode`.
pub fn b62_decode(s: &str) -> Option<i64> {
    if s == "0" {
        return Some(0);
    }
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, s),
    };
    let mut n: i64 = 0;
    for c in digits.bytes() {
        let v = B62.iter().position(|&x| x == c)? as i64;
        n = n.checked_mul(62)?.checked_add(v)?;
    }
    Some(sign * n)
}

/// `Signer.signature(value)` with algorithm sha256:
/// `b64_encode(salted_hmac(salt + "signer", value, key, algorithm="sha256").digest())`.
pub fn signature(salt: &str, value: &str, key: &str) -> String {
    let key_salt = format!("{salt}signer");
    b64_encode(&salted_hmac_sha256(&key_salt, value.as_bytes(), key))
}

/// Why a signed value was rejected (`signing.BadSignature` / decode errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// `No ":" found in value.`
    NoSeparator,
    /// `Signature "..." does not match`.
    BadSignature,
    /// Timestamp part is not base62.
    BadTimestamp,
    /// Payload is not valid base64 / zlib / JSON.
    BadPayload,
}

/// `Signer.unsign` for the given salt (and SECRET_KEY_FALLBACKS, which paperless never sets).
pub fn unsign<'a>(signed: &'a str, salt: &str, key: &str) -> Result<&'a str, SignError> {
    let (value, sig) = signed.rsplit_once(':').ok_or(SignError::NoSeparator)?;
    let expected = signature(salt, value, key);
    if constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
        Ok(value)
    } else {
        Err(SignError::BadSignature)
    }
}

/// `TimestampSigner.unsign(value, max_age=None)`: returns (value, timestamp).
pub fn unsign_timestamped<'a>(
    signed: &'a str,
    salt: &str,
    key: &str,
) -> Result<(&'a str, i64), SignError> {
    let value = unsign(signed, salt, key)?;
    let (value, ts) = value.rsplit_once(':').ok_or(SignError::NoSeparator)?;
    let ts = b62_decode(ts).ok_or(SignError::BadTimestamp)?;
    Ok((value, ts))
}

/// `TimestampSigner(salt=salt).unsign_object(signed, serializer=JSONSerializer)`
/// = `signing.loads(signed, salt=salt)` without `max_age`.
pub fn loads(signed: &str, salt: &str, key: &str) -> Result<serde_json::Value, SignError> {
    let (b64, _ts) = unsign_timestamped(signed, salt, key)?;
    let (compressed, b64) = match b64.strip_prefix('.') {
        Some(rest) => (true, rest),
        None => (false, b64),
    };
    let mut data = b64_decode(b64).ok_or(SignError::BadPayload)?;
    if compressed {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&data[..])
            .take(MAX_DECOMPRESSED)
            .read_to_end(&mut out)
            .map_err(|_| SignError::BadPayload)?;
        data = out;
    }
    // JSONSerializer.loads: json.loads(data.decode("latin-1"))
    let text: String = data.iter().map(|&b| b as char).collect();
    serde_json::from_str(&text).map_err(|_| SignError::BadPayload)
}

/// `signing.dumps(obj, salt=salt, compress=compress)` at the given unix timestamp.
///
/// The JSON must already be serialized like Python's `json.dumps(obj, separators=(",", ":"))`
/// (ASCII-escaped). Use [`python_json_dumps`] for that.
pub fn dumps_at(json: &str, salt: &str, key: &str, compress: bool, timestamp: i64) -> String {
    // JSONSerializer.dumps(...).encode("latin-1"): python_json_dumps output is pure ASCII.
    let data = json.as_bytes();
    let mut payload = b64_encode(data);
    if compress {
        use std::io::Write as _;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let compressed = enc.write_all(data).ok().and_then(|_| enc.finish().ok());
        if let Some(c) = compressed
            && c.len() < data.len().saturating_sub(1)
        {
            payload = format!(".{}", b64_encode(&c));
        }
    }
    let value = format!("{payload}:{}", b62_encode(timestamp));
    let sig = signature(salt, &value, key);
    format!("{value}:{sig}")
}

/// Session encode as `SessionBase.encode` (salt [`SESSION_KEY_SALT`], compress=True).
pub fn encode_session(json: &str, secret_key: &str, timestamp: i64) -> String {
    dumps_at(json, SESSION_KEY_SALT, secret_key, true, timestamp)
}

/// Session decode as `SessionBase.decode`: any error yields an empty session (`{}`).
pub fn decode_session(
    session_data: &str,
    secret_key: &str,
) -> serde_json::Map<String, serde_json::Value> {
    match loads(session_data, SESSION_KEY_SALT, secret_key) {
        Ok(serde_json::Value::Object(m)) => m,
        Ok(_) => serde_json::Map::new(),
        Err(SignError::BadSignature) | Err(SignError::NoSeparator) => {
            // logger "django.security.SuspiciousSession": "Session data corrupted"
            tracing::warn!(target: "django.security.SuspiciousSession", "Session data corrupted");
            serde_json::Map::new()
        }
        Err(_) => serde_json::Map::new(),
    }
}

/// Serialize a JSON value exactly like Python `json.dumps(v, separators=(",", ":"))`
/// (`ensure_ascii=True`): non-ASCII as `\uXXXX` (surrogate pairs). Floats use serde_json's
/// shortest representation (sessions written by Rust contain no floats).
pub fn python_json_dumps(v: &serde_json::Value) -> String {
    let mut out = String::new();
    write_py_json(v, &mut out);
    out
}

fn write_py_json(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_py_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_py_json(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_py_str(k, out);
                out.push(':');
                write_py_json(x, out);
            }
            out.push('}');
        }
    }
}

fn write_py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Standard base64 (with padding) – used by password hashers (`base64.b64encode`).
pub(crate) fn b64_std(data: &[u8]) -> String {
    STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors from testvectors/gen_vectors.py (Django 5.2 algorithms re-implemented in CPython)
    // and cross-checked with the real Django 5.2.18 of reference/venv (signing.loads,
    // SessionStore().decode, salted_hmac, check_password, _unmask_cipher_token).

    #[test]
    fn session_auth_hash_vector() {
        assert_eq!(
            session_auth_hash("pbkdf2_sha256$1000000$abc$def", "test-secret"),
            VEC_SESSION_AUTH_HASH
        );
    }

    #[test]
    fn b62_roundtrip() {
        for n in [0i64, 1, 61, 62, 1_700_000_000, -5] {
            assert_eq!(b62_decode(&b62_encode(n)), Some(n));
        }
        assert_eq!(b62_encode(1_700_000_000), "1r31eq");
    }

    #[test]
    fn decode_known_session() {
        let m = decode_session(VEC_SESSION_DATA, "test-secret");
        assert_eq!(m.get("_auth_user_id").and_then(|v| v.as_str()), Some("1"));
        assert_eq!(
            m.get("_auth_user_backend").and_then(|v| v.as_str()),
            Some("django.contrib.auth.backends.ModelBackend")
        );
        assert_eq!(
            m.get("_auth_user_hash").and_then(|v| v.as_str()),
            Some(VEC_SESSION_AUTH_HASH)
        );
    }

    #[test]
    fn encode_roundtrip() {
        // zlib output of flate2 may differ byte-wise from CPython's zlib; Django only needs
        // to decode it, so a roundtrip plus the uncompressed exact vector below suffice.
        let enc = encode_session(VEC_SESSION_JSON, "test-secret", 1_700_000_000);
        assert!(enc.starts_with('.'));
        let m = decode_session(&enc, "test-secret");
        assert_eq!(serde_json::Value::Object(m).to_string(), VEC_SESSION_JSON);
    }

    #[test]
    fn wrong_key_or_tamper_gives_empty() {
        assert!(decode_session(VEC_SESSION_DATA, "other").is_empty());
        let mut t = VEC_SESSION_DATA.to_string();
        t.insert(3, 'x');
        assert!(decode_session(&t, "test-secret").is_empty());
        assert!(decode_session("garbage", "test-secret").is_empty());
    }

    #[test]
    fn uncompressed_signed_object() {
        let s = dumps_at(r#"{"a":1}"#, "my.salt", "k", false, 1_700_000_000);
        assert_eq!(s, VEC_SIGNED_PLAIN);
        let v = loads(&s, "my.salt", "k").unwrap();
        assert_eq!(v, serde_json::json!({"a": 1}));
    }

    #[test]
    fn python_json() {
        let v = serde_json::json!({"a": "\u{e4}\"\n", "b": [1, true, null], "c": "\u{1F600}"});
        let want = "{\"a\":\"\\u00e4\\\"\\n\",\"b\":[1,true,null],\"c\":\"\\ud83d\\ude00\"}";
        assert_eq!(python_json_dumps(&v), want);
    }

    pub(crate) const VEC_SESSION_AUTH_HASH: &str =
        include_str!("testvectors/session_auth_hash.txt");
    pub(crate) const VEC_SESSION_JSON: &str = include_str!("testvectors/session_json.txt");
    pub(crate) const VEC_SESSION_DATA: &str = include_str!("testvectors/session_data.txt");
    pub(crate) const VEC_SIGNED_PLAIN: &str = include_str!("testvectors/signed_plain.txt");
}
