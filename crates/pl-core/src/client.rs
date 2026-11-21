//! Client address and port for the audit log (django-auditlog `AuditlogMiddleware`).
//!
//! upstream: auditlog/middleware.py (django-auditlog 3.x, `_get_remote_addr`,
//! `_get_remote_port`); columns `auditlog_logentry.remote_addr` (`inet`) and
//! `remote_port` (`PositiveIntegerField`).
//!
//! Hardening (Review M4/M5): upstream writes the raw header text into the `inet`/`integer`
//! columns, so a malformed `X-Forwarded-For`/`-Port` makes PostgreSQL reject the insert and
//! the whole request rolls back (500). Here only a valid IP address and a port in
//! `0..=65535` are returned, anything else becomes `NULL`.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

/// Header value as Django's `request.headers.get` sees it (latin-1, duplicates joined by `,`).
fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut out: Option<String> = None;
    for v in headers.get_all(name) {
        let s: String = v.as_bytes().iter().map(|&b| b as char).collect();
        match &mut out {
            None => out = Some(s),
            Some(o) => {
                o.push(',');
                o.push_str(&s);
            }
        }
    }
    out
}

/// `_get_remote_addr` (without `AUDITLOG_DISABLE_REMOTE_ADDR`, which paperless does not set),
/// restricted to valid IP addresses.
pub fn audit_remote_addr(headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
    let xff = header(headers, "x-forwarded-for").filter(|s| !s.is_empty());
    let raw = match xff {
        None => return peer.map(|p| p.to_canonical()),
        Some(x) => {
            let mut a = x.split(',').next().unwrap_or("").to_string();
            if a.contains('.') && a.contains(':') {
                a = a.split(':').next().unwrap_or("").to_string();
            } else if a.contains('[') {
                a = a.chars().skip(1).collect::<String>().split(']').next().unwrap_or("").to_string();
            }
            a
        }
    };
    raw.trim().parse::<IpAddr>().ok()
}

/// `_get_remote_port` (`int(X-Forwarded-Port)`, else `None`), restricted to `0..=65535`.
pub fn audit_remote_port(headers: &HeaderMap) -> Option<i32> {
    let raw = header(headers, "x-forwarded-port")?;
    let t = raw.trim();
    let t = t.strip_prefix('+').unwrap_or(t);
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit() || b == b'_') || t.starts_with('_') || t.ends_with('_') || t.contains("__") {
        return None;
    }
    let n: u32 = t.replace('_', "").parse().ok()?;
    (n <= 65535).then_some(n as i32)
}

/// Both values as strings/ints ready to bind (`$n::inet`, integer).
pub fn audit_remote(headers: &HeaderMap, peer: Option<SocketAddr>) -> (Option<String>, Option<i32>) {
    (audit_remote_addr(headers, peer.map(|p| p.ip())).map(|a| a.to_string()), audit_remote_port(headers))
}

/// How the request user became known – decides django-auditlog's `actor`.
///
/// `AuditlogMiddleware` runs **before** DRF authentication and stores `request.user` as it is at
/// that point (`_get_actor`: a real `User` that `is_authenticated`, else `None`). `request.user`
/// there comes from Django middlewares only:
///
/// * `AuthenticationMiddleware` – the session user (DRF `SessionAuthentication`);
/// * `AutoLoginMiddleware` (`PAPERLESS_AUTO_LOGIN_USERNAME`);
/// * `HttpRemoteUserMiddleware` – for `/api/` only when `PAPERLESS_ENABLE_HTTP_REMOTE_USER_API`
///   (then DRF's `PaperlessRemoteUserAuthentication` sees the same user).
///
/// DRF `TokenAuthentication`, `PaperlessBasicAuthentication` and the DEBUG-only
/// `AngularApiAuthenticationOverride` set the user later → `actor_id`/`actor_email` stay `NULL`.
///
/// upstream: auditlog/middleware.py (`_get_actor`, `get_extra_data`), auditlog/context.py
/// (`_set_actor`: `actor_email = getattr(actor, "email", None)`), paperless/settings (MIDDLEWARE:
/// AuditlogMiddleware is appended last, after the remote-user middleware).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestUserSource {
    /// Set by a Django middleware (session, auto-login, remote-user middleware).
    DjangoMiddleware,
    /// Set by a DRF authentication class (token, basic, DEBUG angular override).
    DrfAuthentication,
}

/// `(actor_id, actor_email)` for an `auditlog_logentry` written during an API request.
pub fn audit_actor(user_id: i64, email: &str, source: RequestUserSource) -> (Option<i64>, Option<String>) {
    match source {
        RequestUserSource::DjangoMiddleware => (Some(user_id), Some(email.to_string())),
        RequestUserSource::DrfAuthentication => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(*k, HeaderValue::from_static(v));
        }
        m
    }

    #[test]
    fn addresses() {
        let peer: Option<IpAddr> = Some("127.0.0.1".parse().unwrap());
        assert_eq!(audit_remote_addr(&h(&[]), peer), peer);
        assert_eq!(audit_remote_addr(&h(&[("x-forwarded-for", "10.0.0.1:443, 1.2.3.4")]), peer), Some("10.0.0.1".parse().unwrap()));
        assert_eq!(audit_remote_addr(&h(&[("x-forwarded-for", "[::1]:80")]), peer), Some("::1".parse().unwrap()));
        assert_eq!(audit_remote_addr(&h(&[("x-forwarded-for", "2001:db8::1")]), peer), Some("2001:db8::1".parse().unwrap()));
        assert_eq!(audit_remote_addr(&h(&[("x-forwarded-for", "not-an-ip")]), peer), None);
        assert_eq!(audit_remote_addr(&h(&[("x-forwarded-for", "1.2.3.4'; drop")]), peer), None);
        assert_eq!(audit_remote_addr(&h(&[]), Some("::ffff:10.1.2.3".parse().unwrap())), Some("10.1.2.3".parse().unwrap()));
    }

    #[test]
    fn actor() {
        assert_eq!(audit_actor(3, "a@b", RequestUserSource::DjangoMiddleware), (Some(3), Some("a@b".into())));
        assert_eq!(audit_actor(3, "", RequestUserSource::DjangoMiddleware), (Some(3), Some(String::new())));
        assert_eq!(audit_actor(3, "a@b", RequestUserSource::DrfAuthentication), (None, None));
    }

    #[test]
    fn ports() {
        assert_eq!(audit_remote_port(&h(&[])), None);
        assert_eq!(audit_remote_port(&h(&[("x-forwarded-port", " 8443 ")])), Some(8443));
        assert_eq!(audit_remote_port(&h(&[("x-forwarded-port", "+1_0")])), Some(10));
        assert_eq!(audit_remote_port(&h(&[("x-forwarded-port", "65536")])), None);
        assert_eq!(audit_remote_port(&h(&[("x-forwarded-port", "99999999999")])), None);
        assert_eq!(audit_remote_port(&h(&[("x-forwarded-port", "-1")])), None);
        assert_eq!(audit_remote_port(&h(&[("x-forwarded-port", "abc")])), None);
    }
}
