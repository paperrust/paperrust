//! Automatic login in desktop mode (docs/DESKTOP.md "Anmeldung im Desktop-Betrieb").
//!
//! * A new random key on every start (32 bytes from the operating system's randomness, hex) –
//!   [`generate_token`]. It lives in `run/desktop-login.token` (data folder with ACL "current
//!   user only", Unix 0600) and reaches the server through the environment (`PAPERRUST_DESKTOP_LOGIN_TOKEN`,
//!   never the command line, masked in `Debug`/logs), together with the owner
//!   (`PAPERRUST_DESKTOP_OWNER`, `desktop.toml` `[setup] owner_user`).
//! * Shell/browser open `http://127.0.0.1:<port>/rs/desktop/login?token=…&next=…`
//!   ([`login_url`], [`login_url_for`]); the server checks key, loopback and owner
//!   (crates/pl-accounts/src/desktop.rs).
//! * First start without a form: owner from the operating system user name
//!   ([`owner_name_from_os`]) with a random password that is stored nowhere
//!   ([`random_password`]).

use std::path::Path;

use rand::RngCore;

/// File name in the `run/` folder.
pub const TOKEN_FILE: &str = "desktop-login.token";
pub const TOKEN_ENV: &str = "PAPERRUST_DESKTOP_LOGIN_TOKEN";
pub const OWNER_ENV: &str = "PAPERRUST_DESKTOP_OWNER";
/// Path of the server route.
pub const LOGIN_PATH: &str = "/rs/desktop/login";
/// Length of the key in bytes (hex: twice as many characters).
pub const TOKEN_BYTES: usize = 32;
/// Fallback name if the operating system user name yields nothing usable.
pub const FALLBACK_OWNER: &str = "local";

/// New key: 32 bytes from `OsRng`, as 64 hex characters.
pub fn generate_token() -> String {
    let mut b = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Valid format (exactly 64 lowercase hex characters) – anything else is not used.
pub fn is_valid_token(t: &str) -> bool {
    t.len() == TOKEN_BYTES * 2 && t.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Write the key to `run/desktop-login.token` (readable only by the user).
pub fn write_token(run_dir: &Path, token: &str) -> std::io::Result<()> {
    crate::config::write_private(&run_dir.join(TOKEN_FILE), token.as_bytes())
}

/// Read the key of the running instance (`None`: no file or invalid content).
pub fn read_token(run_dir: &Path) -> Option<String> {
    let t = std::fs::read_to_string(run_dir.join(TOKEN_FILE)).ok()?;
    let t = t.trim();
    is_valid_token(t).then(|| t.to_string())
}

/// Remove the key file (on exit; an old key is worthless after the next start anyway).
pub fn remove_token(run_dir: &Path) {
    let _ = std::fs::remove_file(run_dir.join(TOKEN_FILE));
}

/// Like `encodeURIComponent`.
fn encode_component(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                (c as char).to_string()
            }
            _ => format!("%{c:02X}"),
        })
        .collect()
}

/// Login URL: `base` = `http://127.0.0.1:<port>` (with or without `/`), `next` = relative path
/// (e.g. `/` or `/rs/app/m/`; empty = server default).
pub fn login_url(base: &str, token: &str, next: &str) -> String {
    let base = base.trim_end_matches('/');
    let mut u = format!("{base}{LOGIN_PATH}?token={}", encode_component(token));
    if !next.is_empty() {
        u.push_str("&next=");
        u.push_str(&encode_component(next));
    }
    u
}

/// Login URL with the key from `run/` (`None`: no running instance with a key – then
/// use the normal address).
pub fn login_url_for(run_dir: &Path, base: &str, next: &str) -> Option<String> {
    read_token(run_dir).map(|t| login_url(base, &t, next))
}

/// User name for the local owner from the operating system user name: only characters
/// that Django user names allow (`[A-Za-z0-9@.+_-]`, ASCII), others are dropped, at most
/// 150 characters; empty → [`FALLBACK_OWNER`].
pub fn sanitize_owner(os_name: &str) -> String {
    let s: String =
        os_name.trim().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '+' | '-' | '_')).take(150).collect();
    if s.is_empty() || s.chars().all(|c| matches!(c, '.' | '-' | '_' | '+' | '@')) { FALLBACK_OWNER.into() } else { s }
}

/// Owner name from `USERNAME` (Windows) or `USER`/`LOGNAME` (Unix).
pub fn owner_name_from_os(get: impl Fn(&str) -> Option<String>) -> String {
    let raw = ["USERNAME", "USER", "LOGNAME"].iter().find_map(|k| get(k).filter(|v| !v.trim().is_empty())).unwrap_or_default();
    sanitize_owner(&raw)
}

/// Strong random password for the automatically created owner (stored or shown nowhere;
/// whoever needs a password on the LAN sets it in the profile).
pub fn random_password() -> String {
    use rand::distributions::{Alphanumeric, DistString};
    Alphanumeric.sample_string(&mut rand::rngs::OsRng, 48)
}

/// SQL string (standard SQL, `standard_conforming_strings = on`).
pub fn sql_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Query: owner if active, otherwise the first active superuser (smallest id).
pub fn owner_query(current: &str) -> String {
    let lit = sql_literal(current);
    format!(
        "SELECT username FROM auth_user WHERE is_active AND (username = {lit} OR is_superuser) \
         ORDER BY (username = {lit}) DESC, id LIMIT 1"
    )
}

/// What happens to the owner after [`owner_query`] has returned `found`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerDecision {
    /// Registered owner is active.
    Keep,
    /// New owner (none so far, or the previous one is missing/inactive).
    Adopt(String),
    /// No active superuser: automatic login off.
    None,
}

pub fn owner_decision(current: &str, found: &str) -> OwnerDecision {
    match found.trim() {
        "" => OwnerDecision::None,
        f if f == current => OwnerDecision::Keep,
        f => OwnerDecision::Adopt(f.to_string()),
    }
}

/// `base`, from `i = 2` with a counter (`jane2`), at most 150 characters in total.
pub fn numbered_name(base: &str, i: u32) -> String {
    if i <= 1 {
        return base.to_string();
    }
    let suffix = i.to_string();
    let keep = 150usize.saturating_sub(suffix.len());
    format!("{}{suffix}", base.chars().take(keep).collect::<String>())
}

/// Key and owner into the server environment (only if there is an owner).
pub fn add_server_env(env: &mut std::collections::BTreeMap<String, String>, token: &str, owner: &str) {
    if owner.is_empty() || !is_valid_token(token) {
        return;
    }
    env.insert(TOKEN_ENV.into(), token.into());
    env.insert(OWNER_ENV.into(), owner.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_takeover() {
        // existing installation without entry: first active superuser (e.g. "Jane") is adopted
        assert_eq!(owner_decision("", "Jane"), OwnerDecision::Adopt("Jane".into()));
        // registered owner active
        assert_eq!(owner_decision("Jane", "Jane"), OwnerDecision::Keep);
        // registered owner deleted/deactivated → another superuser
        assert_eq!(owner_decision("alt", "admin\n"), OwnerDecision::Adopt("admin".into()));
        // no active superuser at all
        assert_eq!(owner_decision("Jane", ""), OwnerDecision::None);
        assert_eq!(owner_decision("", " "), OwnerDecision::None);
        // the query prefers the registered one, otherwise the smallest id
        let q = owner_query("Jane");
        assert!(q.contains("is_active AND (username = 'Jane' OR is_superuser)"));
        assert!(q.ends_with("ORDER BY (username = 'Jane') DESC, id LIMIT 1"));
    }

    #[test]
    fn numbered_owner_names() {
        assert_eq!(numbered_name("jane", 1), "jane");
        assert_eq!(numbered_name("jane", 2), "jane2");
        let long = "x".repeat(150);
        assert_eq!(numbered_name(&long, 12).len(), 150);
        assert!(numbered_name(&long, 12).ends_with("12"));
    }

    #[test]
    fn server_env_only_with_owner() {
        let t = generate_token();
        let mut e = std::collections::BTreeMap::new();
        add_server_env(&mut e, &t, "");
        assert!(e.is_empty());
        add_server_env(&mut e, "kurz", "jane");
        assert!(e.is_empty());
        add_server_env(&mut e, &t, "jane");
        assert_eq!(e.get(TOKEN_ENV), Some(&t));
        assert_eq!(e.get(OWNER_ENV).map(String::as_str), Some("jane"));
        // the key counts as a secret (Debug/logs masked)
        assert!(crate::server::is_secret(TOKEN_ENV));
    }

    #[test]
    fn tokens_are_random_hex() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert!(is_valid_token(&a) && is_valid_token(&b));
        assert_eq!(a.len(), 64);
        assert!(!is_valid_token("abc"));
        assert!(!is_valid_token(&a.to_uppercase()));
        assert!(!is_valid_token(&format!("{}g", &a[..63])));
    }

    #[test]
    fn token_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_token(dir.path()).is_none());
        let t = generate_token();
        write_token(dir.path(), &t).unwrap();
        assert_eq!(read_token(dir.path()).as_deref(), Some(t.as_str()));
        assert_eq!(
            login_url_for(dir.path(), "http://127.0.0.1:58080/", "/").unwrap(),
            format!("http://127.0.0.1:58080/rs/desktop/login?token={t}&next=%2F")
        );
        std::fs::write(dir.path().join(TOKEN_FILE), "kaputt").unwrap();
        assert!(read_token(dir.path()).is_none());
        remove_token(dir.path());
        assert!(!dir.path().join(TOKEN_FILE).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            write_token(dir.path(), &t).unwrap();
            let m = std::fs::metadata(dir.path().join(TOKEN_FILE)).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
    }

    #[test]
    fn login_urls() {
        assert_eq!(
            login_url("http://127.0.0.1:58080", "ab", "/rs/app/m/"),
            "http://127.0.0.1:58080/rs/desktop/login?token=ab&next=%2Frs%2Fapp%2Fm%2F"
        );
        assert_eq!(login_url("http://127.0.0.1:1/", "ab", ""), "http://127.0.0.1:1/rs/desktop/login?token=ab");
        assert_eq!(
            login_url("http://127.0.0.1:1", "ab", "/d?x=1&y=2"),
            "http://127.0.0.1:1/rs/desktop/login?token=ab&next=%2Fd%3Fx%3D1%26y%3D2"
        );
    }

    #[test]
    fn owner_names() {
        assert_eq!(sanitize_owner("Jane"), "Jane");
        assert_eq!(sanitize_owner("  jane.doe "), "jane.doe");
        assert_eq!(sanitize_owner("Jürg Müller"), "JrgMller");
        assert_eq!(sanitize_owner("DOMAIN\\user"), "DOMAINuser");
        assert_eq!(sanitize_owner(""), FALLBACK_OWNER);
        assert_eq!(sanitize_owner("Ä Ö"), FALLBACK_OWNER);
        assert_eq!(sanitize_owner("..."), FALLBACK_OWNER);
        assert_eq!(sanitize_owner(&"x".repeat(200)).len(), 150);
        let env =
            |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert_eq!(owner_name_from_os(env(&[("USERNAME", "Jane"), ("USER", "x")])), "Jane");
        assert_eq!(owner_name_from_os(env(&[("USERNAME", " "), ("USER", "anna")])), "anna");
        assert_eq!(owner_name_from_os(env(&[])), FALLBACK_OWNER);
    }

    #[test]
    fn passwords_are_strong_and_random() {
        let a = random_password();
        assert_eq!(a.len(), 48);
        assert_ne!(a, random_password());
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn owner_query_escapes() {
        assert_eq!(sql_literal("o'neil"), "'o''neil'");
        let q = owner_query("x'; DROP TABLE auth_user; --");
        assert!(q.contains("'x''; DROP TABLE auth_user; --'"));
        assert!(q.contains("ORDER BY (username = 'x''; DROP TABLE auth_user; --') DESC, id LIMIT 1"));
    }
}
