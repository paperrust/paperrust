//! `PAPERRUST_DB_REQUIRE_AUTH` – allow only certain authentication methods of the PostgreSQL server
//! (like libpq `require_auth`/`PGREQUIREAUTH`, e.g. `scram-sha-256`).
//!
//! sqlx 0.8 has no such option (sqlx does not use libpq, so `PGREQUIREAUTH` has no effect).
//! PaperRust therefore checks **before** opening the pool with a probe connection: send the startup message,
//! read the server's first authentication request (`R` message), close the connection –
//! **without** sending a password. If the server demands a method that is not allowed (e.g. cleartext
//! or `trust`), startup is aborted.
//!
//! Limits (noted in the contract): check once at startup, not for every later pool connection;
//! probe without TLS – with `sslmode=require|verify-ca|verify-full` nothing is checked, instead startup
//! is aborted with an error message (no silent weakening). Not set = no check.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::config::DbConfig;

/// Authentication method like libpq `require_auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    /// `none` (trust: no login required).
    None,
    /// `password` (cleartext).
    Password,
    /// `md5`.
    Md5,
    /// `scram-sha-256` (SASL).
    ScramSha256,
}

impl AuthMethod {
    fn name(self) -> &'static str {
        match self {
            AuthMethod::None => "none",
            AuthMethod::Password => "password",
            AuthMethod::Md5 => "md5",
            AuthMethod::ScramSha256 => "scram-sha-256",
        }
    }
}

/// List from `PAPERRUST_DB_REQUIRE_AUTH` (comma-separated, e.g. `scram-sha-256` or `scram-sha-256,md5`).
pub fn parse(value: &str) -> Result<Vec<AuthMethod>, String> {
    let mut out = Vec::new();
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let m = match part.to_ascii_lowercase().as_str() {
            "none" => AuthMethod::None,
            "password" => AuthMethod::Password,
            "md5" => AuthMethod::Md5,
            "scram-sha-256" => AuthMethod::ScramSha256,
            other => return Err(format!("PAPERRUST_DB_REQUIRE_AUTH: unsupported method '{other}' (none, password, md5, scram-sha-256)")),
        };
        if !out.contains(&m) {
            out.push(m);
        }
    }
    if out.is_empty() {
        return Err("PAPERRUST_DB_REQUIRE_AUTH: empty list".into());
    }
    Ok(out)
}

/// Evaluate the server's first authentication request (send startup, read `R`/`E`).
pub async fn probe<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, user: &str, database: &str) -> Result<AuthMethod, String> {
    let mut body = Vec::new();
    body.extend_from_slice(&196_608i32.to_be_bytes()); // protocol 3.0
    for (k, v) in [("user", user), ("database", database), ("application_name", "paperrust-auth-probe")] {
        body.extend_from_slice(k.as_bytes());
        body.push(0);
        body.extend_from_slice(v.as_bytes());
        body.push(0);
    }
    body.push(0);
    let mut msg = ((body.len() + 4) as i32).to_be_bytes().to_vec();
    msg.extend_from_slice(&body);
    stream.write_all(&msg).await.map_err(|e| format!("auth probe: {e}"))?;
    let mut head = [0u8; 5];
    stream.read_exact(&mut head).await.map_err(|e| format!("auth probe: {e}"))?;
    let len = i32::from_be_bytes([head[1], head[2], head[3], head[4]]);
    if !(4..=8192).contains(&len) {
        return Err("auth probe: unexpected server answer".into());
    }
    let mut payload = vec![0u8; len as usize - 4];
    stream.read_exact(&mut payload).await.map_err(|e| format!("auth probe: {e}"))?;
    match head[0] {
        b'R' if payload.len() >= 4 => {
            let code = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            match code {
                0 => Ok(AuthMethod::None),
                3 => Ok(AuthMethod::Password),
                5 => Ok(AuthMethod::Md5),
                10 => {
                    let mechs: Vec<&[u8]> = payload[4..].split(|b| *b == 0).filter(|m| !m.is_empty()).collect();
                    if mechs.contains(&&b"SCRAM-SHA-256"[..]) {
                        Ok(AuthMethod::ScramSha256)
                    } else {
                        Err("auth probe: server offers SASL without SCRAM-SHA-256".into())
                    }
                }
                other => Err(format!("auth probe: unsupported authentication request {other}")),
            }
        }
        // Server error (e.g. no pg_hba entry): only the hint, no internals.
        b'E' => Err("auth probe: the database server rejected the connection".into()),
        _ => Err("auth probe: unexpected server answer".into()),
    }
}

fn check_method(found: AuthMethod, allowed: &[AuthMethod]) -> Result<(), String> {
    if allowed.contains(&found) {
        Ok(())
    } else {
        let names: Vec<&str> = allowed.iter().map(|m| m.name()).collect();
        Err(format!(
            "database server requested authentication '{}', but PAPERRUST_DB_REQUIRE_AUTH allows only {}",
            found.name(),
            names.join(", ")
        ))
    }
}

/// Check at startup if `PAPERRUST_DB_REQUIRE_AUTH` is set (otherwise immediately `Ok`).
pub async fn check(db: &DbConfig) -> Result<(), String> {
    let Some(raw) = db.require_auth.as_deref() else { return Ok(()) };
    let allowed = parse(raw)?;
    if matches!(db.sslmode.as_str(), "require" | "verify-ca" | "verify-full") {
        return Err(format!(
            "PAPERRUST_DB_REQUIRE_AUTH cannot be checked with sslmode={} (the probe runs without TLS)",
            db.sslmode
        ));
    }
    let port = db.port.unwrap_or(5432);
    let host = db.host.clone().unwrap_or_else(|| "localhost".into());
    let timeout = Duration::from_secs(db.connect_timeout.unwrap_or(10).max(1));
    let found = tokio::time::timeout(timeout, async {
        #[cfg(unix)]
        if host.starts_with('/') {
            let path = format!("{}/.s.PGSQL.{port}", host.trim_end_matches('/'));
            let mut s = tokio::net::UnixStream::connect(path).await.map_err(|e| format!("auth probe: {e}"))?;
            return probe(&mut s, &db.user, &db.name).await;
        }
        let mut s = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| format!("auth probe: {e}"))?;
        probe(&mut s, &db.user, &db.name).await
    })
    .await
    .map_err(|_| "auth probe: timeout".to_string())??;
    check_method(found, &allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn answer(reply: Vec<u8>) -> Result<AuthMethod, String> {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let srv = tokio::spawn(async move {
            let mut len = [0u8; 4];
            server.read_exact(&mut len).await.unwrap();
            let mut rest = vec![0u8; i32::from_be_bytes(len) as usize - 4];
            server.read_exact(&mut rest).await.unwrap();
            assert_eq!(&rest[..4], &196_608i32.to_be_bytes());
            assert!(rest.windows(5).any(|w| w == b"user\0"));
            server.write_all(&reply).await.unwrap();
        });
        let r = probe(&mut client, "paperless", "paperless").await;
        srv.await.unwrap();
        r
    }

    fn auth(code: i32, extra: &[u8]) -> Vec<u8> {
        let mut m = vec![b'R'];
        m.extend_from_slice(&((8 + extra.len()) as i32).to_be_bytes());
        m.extend_from_slice(&code.to_be_bytes());
        m.extend_from_slice(extra);
        m
    }

    #[tokio::test]
    async fn detects_methods() {
        assert_eq!(answer(auth(10, b"SCRAM-SHA-256\0\0")).await, Ok(AuthMethod::ScramSha256));
        assert_eq!(answer(auth(3, b"")).await, Ok(AuthMethod::Password));
        assert_eq!(answer(auth(5, b"salt")).await, Ok(AuthMethod::Md5));
        assert_eq!(answer(auth(0, b"")).await, Ok(AuthMethod::None));
        assert!(answer(auth(10, b"OTHER\0\0")).await.is_err());
    }

    #[test]
    fn policy() {
        let scram = parse("scram-sha-256").unwrap();
        assert!(check_method(AuthMethod::ScramSha256, &scram).is_ok());
        assert!(check_method(AuthMethod::Password, &scram).is_err());
        assert!(check_method(AuthMethod::None, &scram).is_err());
        assert_eq!(parse(" SCRAM-SHA-256 , md5 ").unwrap(), vec![AuthMethod::ScramSha256, AuthMethod::Md5]);
        assert!(parse("kerberos").is_err() && parse(" ").is_err());
    }

    #[tokio::test]
    async fn unset_means_no_check_and_tls_modes_refuse() {
        let env = |pairs: &[(&str, &str)]| {
            let mut v = vec![("PAPERLESS_SECRET_KEY", "x"), ("PAPERLESS_DBHOST", "127.0.0.1"), ("PAPERLESS_DBPORT", "1")];
            v.extend_from_slice(pairs);
            crate::Config::from_env_map(crate::config::Env::from_pairs(v)).unwrap().db
        };
        let db = env(&[]);
        assert!(db.require_auth.is_none());
        assert!(check(&db).await.is_ok(), "nicht gesetzt: keine Probe (Port 1 wäre sonst ein Fehler)");
        let db = env(&[("PAPERRUST_DB_REQUIRE_AUTH", "scram-sha-256"), ("PAPERLESS_DBSSLMODE", "require")]);
        assert!(check(&db).await.unwrap_err().contains("sslmode=require"));
    }
}
