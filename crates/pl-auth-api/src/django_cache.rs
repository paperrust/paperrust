//! Login state in Django's `default` cache layout (Review 2 M2).
//!
//! upstream: Django 5.2 `django/core/cache/backends/base.py::default_key_func` (`"%s:%s:%s" %
//! (prefix, version, key)`, version 1), `backends/redis.py` (`RedisSerializer`: pickle with
//! `HIGHEST_PROTOCOL` = 5, ints unpickled) – the value encoding is kept as in paperless-ngx.
//!
//! Two pieces of state live there:
//! - allauth 65.20.1 `mfa/totp/internal/auth.py::TOTP._mark_code_used`:
//!   `cache.add("allauth.mfa.totp.used?user=<id>&code=<code>", "y", timeout=TOTP_PERIOD)`
//! - DRF 3.18.3 `throttling.py::SimpleRateThrottle`: `cache.set("throttle_login_<ident>",
//!   history, duration)` with `history` a list of `time.time()` floats (newest first).
//!
//! The throttle update is a compare-and-set, so concurrent requests cannot both slip through.
//!
//! Backend selection (process-local or shared in PostgreSQL), key format and storage live in
//! [`pl_core::cache`] (`AppState::cache()`); this module keeps the pickle helpers. Redis is not
//! used anymore (2026-11-14).

pub use pl_core::cache::{Backend, REDIS_CACHE_BACKEND, backend, make_key};

/// `pickle.dumps("y", 5)` – the value allauth stores for a used TOTP code.
pub const PICKLED_Y: &[u8] = b"\x80\x05\x95\x05\x00\x00\x00\x00\x00\x00\x00\x8c\x01y\x94.";

/// `pickle.dumps(list_of_floats, 5)`, byte-identical to CPython's `_pickle`.
pub fn pickle_float_list(values: &[f64]) -> Vec<u8> {
    let mut body: Vec<u8> = vec![b']', 0x94];
    match values.len() {
        0 => {}
        1 => {
            body.push(b'G');
            body.extend_from_slice(&values[0].to_be_bytes());
            body.push(b'a');
        }
        _ => {
            body.push(b'(');
            for v in values {
                body.push(b'G');
                body.extend_from_slice(&v.to_be_bytes());
            }
            body.push(b'e');
        }
    }
    body.push(b'.');
    let mut out = vec![0x80, 5];
    // CPython only emits a FRAME when the frame holds at least 4 bytes.
    if body.len() >= 4 {
        out.push(0x95);
        out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    }
    out.extend_from_slice(&body);
    out
}

/// Reads a pickled list of numbers (the throttle history). Only the opcodes CPython emits for
/// such a list are accepted; anything else (notably object construction) is an error.
pub fn unpickle_float_list(data: &[u8]) -> Result<Vec<f64>, String> {
    let mut i = 0usize;
    let mut out: Vec<f64> = Vec::new();
    let mut in_list = false;
    let take = |i: &mut usize, n: usize| -> Result<&[u8], String> {
        let s = data.get(*i..*i + n).ok_or_else(|| "truncated pickle".to_string())?;
        *i += n;
        Ok(s)
    };
    loop {
        let op = *data.get(i).ok_or_else(|| "truncated pickle".to_string())?;
        i += 1;
        match op {
            0x80 => {
                take(&mut i, 1)?;
            }
            0x95 => {
                take(&mut i, 8)?;
            }
            b']' => {
                if in_list {
                    return Err("nested list".into());
                }
                in_list = true;
            }
            0x94 | b'(' | b'a' | b'e' => {}
            b'G' => {
                let b = take(&mut i, 8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                out.push(f64::from_be_bytes(a));
            }
            // BININT / BININT1 / BININT2 (a history of ints is possible with a patched timer)
            b'J' => {
                let b = take(&mut i, 4)?;
                out.push(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64);
            }
            b'K' => out.push(take(&mut i, 1)?[0] as f64),
            b'M' => {
                let b = take(&mut i, 2)?;
                out.push(u16::from_le_bytes([b[0], b[1]]) as f64);
            }
            b'.' => break,
            other => return Err(format!("unsupported pickle opcode 0x{other:02x}")),
        }
    }
    if !in_list {
        return Err("pickle is not a list".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte vectors from `python -c "import pickle; print(pickle.dumps(v, 5))"` (CPython 3.12).
    #[test]
    fn pickle_matches_cpython() {
        assert_eq!(pickle_float_list(&[]), b"\x80\x05]\x94.");
        assert_eq!(pickle_float_list(&[1736150400.5]), b"\x80\x05\x95\r\x00\x00\x00\x00\x00\x00\x00]\x94GA\xd9\xde\xe3` \x00\x00a.");
        let two = b"\x80\x05\x95\x17\x00\x00\x00\x00\x00\x00\x00]\x94(GA\xd9\xde\xe3` \x00\x00GA\xd9\xde\xe3_\xd0\x00\x00e.";
        assert_eq!(pickle_float_list(&[1736150400.5, 1736150399.25]), two);
        assert_eq!(unpickle_float_list(two).unwrap(), vec![1736150400.5, 1736150399.25]);
        assert_eq!(unpickle_float_list(b"\x80\x05]\x94.").unwrap(), Vec::<f64>::new());
        // GLOBAL/REDUCE is never accepted
        assert!(unpickle_float_list(b"\x80\x05\x8c\x02os\x94\x93.").is_err());
        assert!(unpickle_float_list(PICKLED_Y).is_err());
    }

    #[test]
    fn django_key_format() {
        assert_eq!(make_key("", "throttle_login_1.2.3.4"), ":1:throttle_login_1.2.3.4");
        assert_eq!(make_key("pl", "allauth.mfa.totp.used?user=3&code=123456"), "pl:1:allauth.mfa.totp.used?user=3&code=123456");
    }

    #[test]
    fn backend_selection() {
        use pl_core::config::Config;
        let cfg = |pairs: &[(&str, &str)]| {
            let mut v: Vec<(&str, &str)> = vec![("PAPERLESS_SECRET_KEY", "test-only-0123456789abcdefghijklmnopqrstuvwxyzABCDEFG")];
            v.extend_from_slice(pairs);
            Config::from_env_map(pl_core::config::Env::from_pairs(v)).unwrap()
        };
        assert_eq!(backend(&cfg(&[])), Backend::Local);
        assert_eq!(backend(&cfg(&[("PAPERLESS_DEBUG", "true")])), Backend::Local);
        // former Redis cache -> shared cache in PostgreSQL
        assert_eq!(backend(&cfg(&[("PAPERLESS_DEBUG", "true"), ("PAPERLESS_CACHE_BACKEND", REDIS_CACHE_BACKEND)])), Backend::Postgres);
        assert_eq!(backend(&cfg(&[("PAPERLESS_CACHE_BACKEND", "django.core.cache.backends.dummy.DummyCache")])), Backend::Local);
    }
}
