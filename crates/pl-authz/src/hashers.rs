//! Password hashing like Django's default `PASSWORD_HASHERS` (paperless sets none).
//!
//! upstream: django/contrib/auth/hashers.py@5.2 (make_password, check_password/verify_password,
//!           identify_hasher, is_password_usable, PBKDF2PasswordHasher, PBKDF2SHA1PasswordHasher,
//!           must_update_salt), django/utils/crypto.py@5.2 (get_random_string, RANDOM_STRING_CHARS).
//!
//! Default list in Django 5.2: PBKDF2 (sha256, preferred), PBKDF2SHA1, Argon2, BCryptSHA256,
//! Scrypt (`Argon2PasswordHasher`, `BCryptSHA256PasswordHasher`, `ScryptPasswordHasher`). All five
//! are verified here; new hashes are always pbkdf2_sha256 and every other algorithm is upgraded at
//! the next successful login (`hasher_changed` -> `must_update`). paperless v3.4.2 does not install
//! `argon2-cffi` or `bcrypt` (see uv.lock), so Django answers such hashes with a 500 - PaperRust
//! verifies them (Known differences in docs/contract/pl-authz.md: migration from other tools).
//!
//! PBKDF2 with 1 000 000 iterations is CPU-heavy: call [`check_password`] / [`make_password`]
//! from `tokio::task::spawn_blocking` (the async helpers in `authn` do that).

use crate::signing::{b64_std, constant_time_eq};
use rand::Rng as _;

/// `PBKDF2PasswordHasher.iterations` in Django 5.2.
pub const PBKDF2_ITERATIONS: u32 = 1_000_000;
/// `UNUSABLE_PASSWORD_PREFIX`.
pub const UNUSABLE_PASSWORD_PREFIX: &str = "!";
/// `UNUSABLE_PASSWORD_SUFFIX_LENGTH`.
pub const UNUSABLE_PASSWORD_SUFFIX_LENGTH: usize = 40;
/// `django.utils.crypto.RANDOM_STRING_CHARS`.
pub const RANDOM_STRING_CHARS: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// `get_random_string(length)`.
pub fn get_random_string(length: usize) -> String {
    let mut rng = rand::rngs::OsRng;
    (0..length)
        .map(|_| RANDOM_STRING_CHARS[rng.gen_range(0..RANDOM_STRING_CHARS.len())] as char)
        .collect()
}

/// `BasePasswordHasher.salt()`: ceil(128 / log2(62)) = 22 characters.
pub fn new_salt() -> String {
    get_random_string(22)
}

/// `must_update_salt(salt, 128)`: `len(salt) * log2(62) < 128`.
fn must_update_salt(salt: &str) -> bool {
    (salt.chars().count() as f64) * (62f64).log2() < 128.0
}

/// `is_password_usable(encoded)`.
pub fn is_password_usable(encoded: Option<&str>) -> bool {
    match encoded {
        None => true,
        Some(e) => !e.starts_with(UNUSABLE_PASSWORD_PREFIX),
    }
}

/// `make_password(None)`: an unusable password.
pub fn make_unusable_password() -> String {
    format!(
        "{UNUSABLE_PASSWORD_PREFIX}{}",
        get_random_string(UNUSABLE_PASSWORD_SUFFIX_LENGTH)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algo {
    Sha256,
    Sha1,
}

/// Upper memory bound for verifying a stored Argon2/Scrypt hash (bytes). Hashes come from the
/// own database, but a corrupt row must not exhaust the RAM; above the bound the hash counts as
/// wrong (Django would try and possibly fail with `MemoryError`).
const MAX_HASH_MEMORY: u64 = 1 << 30;

fn pbkdf2_b64(algo: Algo, password: &str, salt: &str, iterations: u32) -> String {
    match algo {
        Algo::Sha256 => {
            let mut out = [0u8; 32];
            pbkdf2::pbkdf2_hmac::<sha2::Sha256>(
                password.as_bytes(),
                salt.as_bytes(),
                iterations,
                &mut out,
            );
            b64_std(&out)
        }
        Algo::Sha1 => {
            let mut out = [0u8; 20];
            pbkdf2::pbkdf2_hmac::<sha1::Sha1>(
                password.as_bytes(),
                salt.as_bytes(),
                iterations,
                &mut out,
            );
            b64_std(&out)
        }
    }
}

fn algo_name(a: Algo) -> &'static str {
    match a {
        Algo::Sha256 => "pbkdf2_sha256",
        Algo::Sha1 => "pbkdf2_sha1",
    }
}

/// `PBKDF2PasswordHasher.encode(password, salt, iterations)`.
pub fn encode_pbkdf2_sha256(password: &str, salt: &str, iterations: u32) -> String {
    format!(
        "pbkdf2_sha256${iterations}${salt}${}",
        pbkdf2_b64(Algo::Sha256, password, salt, iterations)
    )
}

/// `make_password(password)` with the default hasher (pbkdf2_sha256, 1 000 000 iterations,
/// 22-char random salt).
pub fn make_password(password: &str) -> String {
    encode_pbkdf2_sha256(password, &new_salt(), PBKDF2_ITERATIONS)
}

/// Result of [`verify_password`] (`django.contrib.auth.hashers.verify_password`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    pub is_correct: bool,
    /// The stored hash should be replaced by `make_password(raw)` (Django's `setter`).
    pub must_update: bool,
}

struct Decoded<'a> {
    algo: Algo,
    iterations: u32,
    salt: &'a str,
}

fn decode(encoded: &str) -> Option<Decoded<'_>> {
    // algorithm, iterations, salt, hash = encoded.split("$", 3)
    let mut it = encoded.splitn(4, '$');
    let algo = match it.next()? {
        "pbkdf2_sha256" => Algo::Sha256,
        "pbkdf2_sha1" => Algo::Sha1,
        _ => return None,
    };
    let iterations: u32 = it.next()?.parse().ok()?;
    let salt = it.next()?;
    it.next()?;
    Some(Decoded {
        algo,
        iterations,
        salt,
    })
}

/// `verify_password(password, encoded)` with the default preferred hasher.
pub fn verify_password(password: &str, encoded: &str) -> Verified {
    let no = Verified {
        is_correct: false,
        must_update: false,
    };
    if !is_password_usable(Some(encoded)) {
        return no;
    }
    // identify_hasher: algorithm = encoded.split("$", 1)[0]. The other default hashers differ
    // from the preferred one, so a correct password always upgrades (hasher_changed) and a
    // wrong one runs no harden_runtime.
    let other = match encoded.split_once('$').map(|(a, _)| a) {
        Some("argon2") => Some(verify_argon2(password, encoded)),
        Some("bcrypt_sha256") => Some(verify_bcrypt_sha256(password, encoded)),
        Some("scrypt") => Some(verify_scrypt(password, encoded)),
        _ => None,
    };
    if let Some(is_correct) = other {
        return Verified {
            is_correct,
            must_update: true,
        };
    }
    // unknown/uninstalled algorithm -> (False, False)
    let Some(d) = decode(encoded) else {
        return no;
    };
    if d.iterations == 0 || d.salt.contains('$') {
        return no;
    }
    let hasher_changed = d.algo != Algo::Sha256;
    let must_update =
        hasher_changed || d.iterations != PBKDF2_ITERATIONS || must_update_salt(d.salt);
    let recomputed = format!(
        "{}${}${}${}",
        algo_name(d.algo),
        d.iterations,
        d.salt,
        pbkdf2_b64(d.algo, password, d.salt, d.iterations)
    );
    let is_correct = constant_time_eq(encoded.as_bytes(), recomputed.as_bytes());
    if !is_correct && !hasher_changed && must_update {
        // harden_runtime: run the missing iterations
        if PBKDF2_ITERATIONS > d.iterations {
            let _ = pbkdf2_b64(d.algo, password, d.salt, PBKDF2_ITERATIONS - d.iterations);
        }
    }
    Verified {
        is_correct,
        must_update,
    }
}

/// Base64 without padding (argon2-cffi / PHC strings: standard alphabet, no `=`).
fn b64_nopad(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(s.trim_end_matches('='))
        .ok()
}

/// `Argon2PasswordHasher.verify`: `argon2.PasswordHasher().verify("$" + rest, password)`.
/// Encoded `argon2$argon2id$v=19$m=102400,t=2,p=8$<salt>$<hash>`; `argon2i`/`argon2d` and the
/// old form without `v=` (version 0x10, as argon2-cffi reads it) are accepted, too.
fn verify_argon2(password: &str, encoded: &str) -> bool {
    let Some(rest) = encoded.strip_prefix("argon2$") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('$').collect();
    let (variety, version, params, salt, hash) = match parts.as_slice() {
        [t, v, p, s, h] => (*t, Some(*v), *p, *s, *h),
        [t, p, s, h] => (*t, None, *p, *s, *h),
        _ => return false,
    };
    let algorithm = match variety {
        "argon2id" => argon2::Algorithm::Argon2id,
        "argon2i" => argon2::Algorithm::Argon2i,
        "argon2d" => argon2::Algorithm::Argon2d,
        _ => return false,
    };
    let version = match version {
        None | Some("v=16") => argon2::Version::V0x10,
        Some("v=19") => argon2::Version::V0x13,
        Some(_) => return false,
    };
    let (mut m, mut t, mut p) = (None, None, None);
    for kv in params.split(',') {
        let Some((k, v)) = kv.split_once('=') else {
            return false;
        };
        let Ok(n) = v.parse::<u32>() else {
            return false;
        };
        match k {
            "m" => m = Some(n),
            "t" => t = Some(n),
            "p" => p = Some(n),
            _ => return false,
        }
    }
    let (Some(m), Some(t), Some(p)) = (m, t, p) else {
        return false;
    };
    if u64::from(m) * 1024 > MAX_HASH_MEMORY {
        return false;
    }
    let (Some(salt), Some(expected)) = (b64_nopad(salt), b64_nopad(hash)) else {
        return false;
    };
    let Ok(params) = argon2::Params::new(m, t, p, Some(expected.len())) else {
        return false;
    };
    let mut out = vec![0u8; expected.len()];
    if argon2::Argon2::new(algorithm, version, params)
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .is_err()
    {
        return false;
    }
    constant_time_eq(&out, &expected)
}

/// `BCryptSHA256PasswordHasher.verify`: `bcrypt.hashpw(hexlify(sha256(password)), data)` and a
/// constant-time comparison (`bcrypt::verify` recomputes with salt and cost taken from `data`).
fn verify_bcrypt_sha256(password: &str, encoded: &str) -> bool {
    use sha2::Digest as _;
    let Some(data) = encoded.strip_prefix("bcrypt_sha256$") else {
        return false;
    };
    let digest = sha2::Sha256::digest(password.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    bcrypt::verify(hex.as_bytes(), data).unwrap_or(false)
}

/// `ScryptPasswordHasher.verify`: `scrypt$<n>$<salt>$<r>$<p>$<b64(dk)>`, recomputed with
/// `hashlib.scrypt(password, salt, n, r, p, dklen=64)` and compared in constant time.
fn verify_scrypt(password: &str, encoded: &str) -> bool {
    let parts: Vec<&str> = encoded.splitn(6, '$').collect();
    let [_, n, salt, r, p, _hash] = parts.as_slice() else {
        return false;
    };
    let (Ok(n), Ok(r), Ok(p)) = (n.parse::<u64>(), r.parse::<u32>(), p.parse::<u32>()) else {
        return false;
    };
    // hashlib.scrypt: n must be a power of 2 greater than 1
    if n < 2 || !n.is_power_of_two() {
        return false;
    }
    if 128u64.saturating_mul(n).saturating_mul(u64::from(r)) > MAX_HASH_MEMORY {
        return false;
    }
    let Ok(log_n) = u8::try_from(n.trailing_zeros()) else {
        return false;
    };
    let Ok(params) = scrypt::Params::new(log_n, r, p, 64) else {
        return false;
    };
    let mut out = [0u8; 64];
    if scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out).is_err() {
        return false;
    }
    let recomputed = format!("scrypt${n}${salt}${r}${p}${}", b64_std(&out));
    constant_time_eq(encoded.as_bytes(), recomputed.as_bytes())
}

/// `check_password(password, encoded)` without setter (see [`verify_password`] for rehash).
pub fn check_password(password: &str, encoded: &str) -> bool {
    verify_password(password, encoded).is_correct
}

/// Run the default hasher once to reduce timing differences for unknown users
/// (`ModelBackend.authenticate`: `UserModel().set_password(password)`).
pub fn dummy_hash(password: &str) {
    let _ = make_password(password);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Generated by testvectors/gen_vectors.py (hashlib.pbkdf2_hmac, base64.b64encode).
    const V256: &str = include_str!("testvectors/pbkdf2_sha256.txt");
    const V1: &str = include_str!("testvectors/pbkdf2_sha1.txt");

    #[test]
    fn pbkdf2_sha256_vector() {
        let v = verify_password("lètmein", V256);
        assert!(v.is_correct);
        assert!(v.must_update, "1000 iterations != 1_000_000");
        // A wrong password here would run harden_runtime (999 000 extra iterations), too slow
        // for an unoptimised test build; covered for sha1 below (no hardening on hasher change).
        assert_eq!(
            encode_pbkdf2_sha256("lètmein", "seasalt1234567890abcde", 1000),
            V256
        );
    }

    #[test]
    fn pbkdf2_sha1_vector() {
        let v = verify_password("lètmein", V1);
        assert!(v.is_correct);
        assert!(v.must_update);
        assert!(!check_password("x", V1));
    }

    // Django test suite vector (tests/auth_tests/test_hashers.py::test_pbkdf2):
    // make_password("lètmein", "seasalt", "pbkdf2_sha256") with 1_000_000 iterations.
    // Not hard-coded here because computing 1M iterations in an unoptimised debug build
    // takes too long for a unit test; the formula is identical to the 1000-iteration vector.

    #[test]
    fn unusable_and_garbage() {
        assert!(!check_password("x", "!abc"));
        assert!(!check_password("x", ""));
        assert!(!check_password("x", "argon2$argon2id$v=19$m=102400,t=2,p=8$abc$def"));
        assert!(!check_password("x", "bcrypt_sha256$garbage"));
        assert!(!check_password("x", "scrypt$1000$salt$8$1$abc"));
        // plain bcrypt is not in Django's default list
        assert!(!check_password(
            "U*U",
            "bcrypt$$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"
        ));
        assert!(!check_password("x", "pbkdf2_sha256$abc"));
        assert!(!is_password_usable(Some(&make_unusable_password())));
        assert_eq!(make_unusable_password().len(), 41);
    }

    #[test]
    fn salt_rules() {
        assert_eq!(new_salt().len(), 22);
        assert!(must_update_salt("seasalt"));
        assert!(must_update_salt(&"a".repeat(21)));
        assert!(!must_update_salt(&"a".repeat(22)));
    }

    /// Scrypt vectors generated with Python 3.12 `hashlib.scrypt` in Django's format
    /// (`ScryptPasswordHasher.encode`, dklen 64).
    #[test]
    fn scrypt_like_django() {
        let v = "scrypt$1024$seasalt$8$1$+qO2jTkVUbPNlniTkHY96ldSJKs4U0WQif8UbWlfO3wJDNhKOg+pPtDckiT6Zw0qkEvKIQ1MdONfGxsWrpoiNg==";
        assert_eq!(verify_password("l\u{e8}tmein", v), Verified { is_correct: true, must_update: true });
        assert_eq!(verify_password("letmein", v), Verified { is_correct: false, must_update: true });
        let v16 = "scrypt$16384$seasalt$8$1$Qj3+9PPyRjSJIebHnG81TMjsqtaIGxNQG/aEB/NYafTJ7tibgfYz71m0ldQESkXFRkdVCBhhY8mx7rQwite/Pw==";
        assert!(check_password("l\u{e8}tmein", v16));
        // n must be a power of two; memory bound
        assert!(!check_password("l\u{e8}tmein", &v.replace("$1024$", "$1000$")));
        assert!(!check_password("l\u{e8}tmein", "scrypt$1073741824$seasalt$8$1$x"));
    }

    /// Argon2 vectors of the reference implementation (phc-winner-argon2 `src/test.c`,
    /// "password"/"somesalt", t=2, m=2^16, p=1) in Django's `argon2$...` form.
    #[test]
    fn argon2_reference_vectors() {
        let v13 = "argon2$argon2i$v=19$m=65536,t=2,p=1$c29tZXNhbHQ$wWKIMhR9lyDFvRz9YTZweHKfbftvj+qf+YFY4NeBbtA";
        assert_eq!(verify_password("password", v13), Verified { is_correct: true, must_update: true });
        assert!(!check_password("passwort", v13));
        // old encoding without `v=` = version 0x10
        let v10 = "argon2$argon2i$m=65536,t=2,p=1$c29tZXNhbHQ$9sTbSlTio3Biev89thdrlKKiCaYsjjYVJxGAL3swxpQ";
        assert!(check_password("password", v10));
        let id = "argon2$argon2id$v=19$m=65536,t=2,p=1$c29tZXNhbHQ$CTFhFdXPJO1aFaMaO6Mm5c8y7cJHAph8ArZWb2GRPPc";
        assert!(check_password("password", id));
        assert!(!check_password("password", &id.replace("argon2id", "argon2x")));
        assert!(!check_password("password", &id.replace("m=65536", "m=99999999")));
    }

    /// bcrypt_sha256 = bcrypt over the hex SHA-256 of the password. The bcrypt core is checked
    /// with the classic OpenBSD vector ("U*U"), the composition with a fixed salt.
    #[test]
    fn bcrypt_sha256_composition() {
        assert!(bcrypt::verify("U*U", "$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW").unwrap_or(false));
        // hexlify(sha256("l\u{e8}tmein")) (Python 3.12 hashlib)
        let hex = "1af7ea1e8c993e69aa1fe000aedd64a4586cc74b978bf3b951eda5009d2eba2d";
        let parts = bcrypt::hash_with_salt(hex, 4, *b"0123456789abcdef").map(|h| h.format_for_version(bcrypt::Version::TwoB));
        let Ok(data) = parts else { panic!("bcrypt hash") };
        let encoded = format!("bcrypt_sha256${data}");
        assert_eq!(verify_password("l\u{e8}tmein", &encoded), Verified { is_correct: true, must_update: true });
        assert!(!check_password("letmein", &encoded));
    }
}
