//! Device tokens for the SMTP intake.
//!
//! * Form: `scan-` + 26 characters `[a-z0-9]` (random from the OS RNG, ≈ 134 bit). Only lowercase letters
//!   and digits, so that the token also works as a recipient address (`scan-…@host`) – some
//!   devices lowercase addresses. Total length 31 characters: fits into password fields with a
//!   32-character limit and into the local part of an address (≤ 64).
//! * Only `sha256(token)` (hex) is stored. The plaintext is shown exactly once.
//! * The check compares hashes (index lookup), not the plaintext.
//! * These tokens are valid **only** in the SMTP intake. The HTTP login (pl-authz) does not know them:
//!   no reading, no deleting, only uploading.

use rand::RngCore as _;
use sha2::{Digest, Sha256};

pub const PREFIX: &str = "scan-";
pub const BODY_LEN: usize = 26;
const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// New token (plaintext). Only for one-time output.
pub fn generate() -> String {
    let mut out = String::with_capacity(PREFIX.len() + BODY_LEN);
    out.push_str(PREFIX);
    let mut rng = rand::rngs::OsRng;
    while out.len() < PREFIX.len() + BODY_LEN {
        let mut b = [0u8; 1];
        rng.fill_bytes(&mut b);
        // 252 = 7 · 36: uniformly distributed without modulo bias
        if b[0] < 252 {
            out.push(ALPHABET[(b[0] % 36) as usize] as char);
        }
    }
    out
}

/// Normal form for comparing (case-insensitive, whitespace removed).
pub fn normalize(candidate: &str) -> String {
    candidate.trim().to_ascii_lowercase()
}

/// Formally valid? Invalid forms are rejected without DB access.
pub fn is_well_formed(candidate: &str) -> bool {
    let c = normalize(candidate);
    c.strip_prefix(PREFIX)
        .is_some_and(|b| b.len() == BODY_LEN && b.bytes().all(|x| x.is_ascii_lowercase() || x.is_ascii_digit()))
}

/// `sha256(normalize(token))` as 64 hex characters.
pub fn hash(token: &str) -> String {
    let d = Sha256::digest(normalize(token).as_bytes());
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Short hint for recognition in the list (`scan-ab…`), reveals only 2 characters.
pub fn hint(token: &str) -> String {
    let n = normalize(token);
    format!("{}…", n.chars().take(PREFIX.len() + 2).collect::<String>())
}

/// Token from a recipient address: `scan-…@host` or `name+scan-…@host`.
pub fn from_address(addr: &str) -> Option<String> {
    let addr = addr.trim().trim_start_matches('<').trim_end_matches('>');
    let local = addr.rsplit_once('@').map(|(l, _)| l).unwrap_or(addr);
    let local = normalize(local);
    let cand = local.rsplit_once('+').map(|(_, t)| t.to_string()).unwrap_or(local);
    is_well_formed(&cand).then_some(cand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_and_check() {
        let t = generate();
        assert_eq!(t.len(), 31);
        assert!(is_well_formed(&t));
        assert!(is_well_formed(&t.to_ascii_uppercase()));
        assert_ne!(generate(), t);
        assert_eq!(hash(&t), hash(&format!(" {} ", t.to_ascii_uppercase())));
        assert_eq!(hash(&t).len(), 64);
        assert!(hint(&t).starts_with("scan-") && hint(&t).ends_with('…'));
    }

    #[test]
    fn malformed() {
        assert!(!is_well_formed("scan-short"));
        assert!(!is_well_formed("prt_aaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(!is_well_formed("scan-aaaaaaaaaaaaaaaaaaaaaaaaa!"));
    }

    #[test]
    fn address_forms() {
        let t = "scan-abcdefghijklmnopqrstuvwxyz";
        assert_eq!(from_address(&format!("<{t}@paperrust.local>")).as_deref(), Some(t));
        assert_eq!(from_address(&format!("office+{t}@x")).as_deref(), Some(t));
        assert_eq!(from_address(&format!("{}@x", t.to_ascii_uppercase())).as_deref(), Some(t));
        assert_eq!(from_address("someone@example.com"), None);
        assert_eq!(from_address("<>"), None);
    }
}
