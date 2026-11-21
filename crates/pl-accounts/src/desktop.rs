//! Automatic login in desktop mode: `GET /rs/desktop/login?token=…&next=…`.
//!
//! PaperRust extension (no upstream counterpart). The desktop supervisor (`apps/desktop`)
//! generates a random key on every start, stores it in its `run/` folder (ACL "current
//! user only") and passes it to the server via the environment:
//!
//! * `PAPERRUST_DESKTOP_LOGIN_TOKEN` – key (at least [`MIN_TOKEN_LEN`] characters),
//! * `PAPERRUST_DESKTOP_OWNER` – user name of the local owner.
//!
//! If either variable is missing, the route answers like an unknown path (404). Otherwise:
//!
//! 1. The TCP peer must be loopback (127.0.0.0/8, `::1`; `X-Forwarded-For` & co. are deliberately
//!    ignored) – otherwise 403. Also applies with LAN sharing: other devices log in
//!    normally.
//! 2. Throttle: after [`RATES`] failed attempts 403 (even with the correct key), counter in the
//!    shared cache like the allauth throttles.
//! 3. Key comparison in constant time (SHA-256 of both sides, then `constant_time_eq`) –
//!    wrong/missing → 403 without hint, log line like a failed login.
//! 4. The owner must exist and be active, otherwise 403 (reason only in the log).
//! 5. Session as in the normal login (`auth::login`: rotate session, rotate CSRF,
//!    `last_login`), then 302 to `next`, only if relative ([`safe_next`]), otherwise `/rs/app/`.
//!
//! All responses: `Cache-Control: no-store`, `Referrer-Policy: no-referrer` (the key is in
//! the URL and must not reach follow-up pages as a referer or end up in a cache).
//! Two-factor is deliberately not asked for here: the key itself is the proof of "this
//! Windows account" (docs/DESKTOP.md "Login in desktop mode").

use std::net::{IpAddr, SocketAddr};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderValue, Request, header};
use axum::response::Response;
use pl_core::AppState;
use sha2::{Digest, Sha256};

use crate::ratelimit::{self, Subject};
use crate::views::run;
use crate::web::{Ctx, Internal, R, forbidden, not_found, redirect};
use crate::{auth, flow};

/// Route id (for `routes.toml`; ids not listed are enabled).
pub const ROUTE_ID: &str = "rs.desktop.login";
pub const PATH: &str = "/rs/desktop/login";
pub const TOKEN_ENV: &str = "PAPERRUST_DESKTOP_LOGIN_TOKEN";
pub const OWNER_ENV: &str = "PAPERRUST_DESKTOP_OWNER";
/// Shorter keys count as a misconfiguration (route stays off).
pub const MIN_TOKEN_LEN: usize = 32;
/// Default target after login.
pub const DEFAULT_NEXT: &str = "/rs/app/";
const ACTION: &str = "desktop_login_failed";
/// Failed attempts per loopback address.
pub const RATES: &str = "10/m/ip,30/h/ip";

/// Configuration from the environment of the server process.
#[derive(Clone, PartialEq, Eq)]
pub struct DesktopLogin {
    token: String,
    pub owner: String,
}

impl std::fmt::Debug for DesktopLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopLogin").field("token", &"***").field("owner", &self.owner).finish()
    }
}

impl DesktopLogin {
    /// `None` = route off (variable missing/empty or key too short).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let token = pl_core::config::lookup(&get, TOKEN_ENV)?.trim().to_string();
        let owner = pl_core::config::lookup(&get, OWNER_ENV)?.trim().to_string();
        if token.is_empty() || owner.is_empty() {
            return None;
        }
        if token.len() < MIN_TOKEN_LEN {
            tracing::warn!("{TOKEN_ENV} is shorter than {MIN_TOKEN_LEN} characters – desktop auto-login stays off");
            return None;
        }
        Some(Self { token, owner })
    }

    /// Process environment only (not the values stored in the UI): the key
    /// is set by the supervisor alone.
    pub fn from_process() -> Option<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn token_matches(&self, given: &str) -> bool {
        token_matches(&self.token, given)
    }
}

/// Comparison in constant time, also for different lengths (both sides are hashed).
pub fn token_matches(expected: &str, given: &str) -> bool {
    if expected.is_empty() || given.is_empty() {
        return false;
    }
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(given.as_bytes());
    pl_authz::signing::constant_time_eq(&a, &b)
}

/// TCP peer is loopback (`127.0.0.0/8`, `::1`, also as `::ffff:127.0.0.1`).
pub fn is_loopback(peer: Option<IpAddr>) -> bool {
    peer.is_some_and(|ip| ip.to_canonical().is_loopback())
}

/// `next` only as a relative path on this server: starts with exactly one `/`, no `//` or
/// `/\` (protocol-relative), no control characters/backslashes/spaces. Otherwise [`DEFAULT_NEXT`].
pub fn safe_next(next: Option<&str>) -> String {
    let Some(n) = next else { return DEFAULT_NEXT.into() };
    let ok = n.starts_with('/')
        && !n.starts_with("//")
        && n.len() <= 2048
        && !n.chars().any(|c| c == '\\' || c.is_control() || c.is_whitespace());
    if ok { n.to_string() } else { DEFAULT_NEXT.into() }
}

fn harden(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    r
}

/// `GET /rs/desktop/login`.
pub async fn desktop_login(State(st): State<AppState>, req: Request<Body>) -> Response {
    let Some(cfg) = DesktopLogin::from_process() else {
        return not_found(PATH);
    };
    handle(st, req, cfg).await
}

/// Flow with the given configuration (tests without process environment).
pub async fn handle(st: AppState, req: Request<Body>, cfg: DesktopLogin) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
    if !is_loopback(peer) {
        tracing::warn!(target: "paperless.auth", "Desktop auto-login refused: not a loopback connection");
        return harden(forbidden());
    }
    harden(run(st, req, move |ctx| Box::pin(async move { view(ctx, &cfg).await })).await)
}

async fn view(ctx: &mut Ctx, cfg: &DesktopLogin) -> R<Response> {
    let ip = ctx.remote_addr.clone();
    let subj = Subject { ip: ip.as_deref(), ..Default::default() };
    // Check the throttle first (without counting) – only failed attempts are counted.
    if ratelimit::consume(&ctx.cache, ACTION, RATES, &subj, true).await.map_err(|e| Internal(e.0))?.is_none() {
        tracing::warn!(target: "paperless.auth", "Desktop auto-login refused: too many failed attempts");
        return Ok(forbidden());
    }
    if !cfg.token_matches(ctx.get("token").unwrap_or("")) {
        ratelimit::consume(&ctx.cache, ACTION, RATES, &subj, false).await.map_err(|e| Internal(e.0))?;
        auth::FailedLoginClient::of(ctx).log("(desktop auto-login)");
        return Ok(forbidden());
    }
    let user = pl_authz::user::load_user_by_username(ctx.st.db(), &cfg.owner).await?;
    let Some(user) = user.filter(|u| u.is_active) else {
        tracing::warn!(target: "paperless.auth", "Desktop auto-login refused: owner user `{}` does not exist or is inactive", cfg.owner);
        return Ok(forbidden());
    };
    let next = safe_next(ctx.get("next"));
    let name = user.username.clone();
    // Discard half-finished logins (e.g. open MFA step), then as LoginView.
    flow::clear_login(ctx);
    auth::login(ctx, user, auth::MODEL_BACKEND).await?;
    flow::record_authentication(ctx, "desktop", &[]);
    let remember = !ctx.s.session_expire_at_browser_close;
    ctx.session.set_expiry(if remember { ctx.s.session_cookie_age } else { 0 });
    tracing::info!(target: "paperless.auth", "Desktop auto-login for user `{name}` (loopback)");
    Ok(redirect(&next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const T: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn route_is_off_without_both_variables() {
        assert!(DesktopLogin::from_lookup(env(&[])).is_none());
        assert!(DesktopLogin::from_lookup(env(&[(TOKEN_ENV, T)])).is_none());
        assert!(DesktopLogin::from_lookup(env(&[(OWNER_ENV, "jane")])).is_none());
        assert!(DesktopLogin::from_lookup(env(&[(TOKEN_ENV, " "), (OWNER_ENV, "jane")])).is_none());
        assert!(DesktopLogin::from_lookup(env(&[(TOKEN_ENV, T), (OWNER_ENV, "")])).is_none());
        // too short a key = misconfiguration
        assert!(DesktopLogin::from_lookup(env(&[(TOKEN_ENV, "kurz"), (OWNER_ENV, "jane")])).is_none());
        let c = DesktopLogin::from_lookup(env(&[(TOKEN_ENV, T), (OWNER_ENV, "Jane")])).unwrap();
        assert_eq!(c.owner, "Jane");
        assert!(c.token_matches(T));
        // Debug does not reveal the key
        assert!(!format!("{c:?}").contains(T));
    }

    #[test]
    fn token_comparison() {
        assert!(token_matches(T, T));
        assert!(!token_matches(T, ""));
        assert!(!token_matches("", ""));
        assert!(!token_matches(T, &T[..63]));
        assert!(!token_matches(T, &format!("{T}0")));
        let mut other = T.to_string();
        other.replace_range(0..1, "1");
        assert!(!token_matches(T, &other));
        assert!(!token_matches(T, &T.to_uppercase()));
    }

    #[test]
    fn loopback_only() {
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        assert!(is_loopback(ip("127.0.0.1")));
        assert!(is_loopback(ip("::1")));
        assert!(is_loopback(ip("::ffff:127.0.0.1")));
        assert!(!is_loopback(None));
        assert!(!is_loopback(ip("192.168.1.20")));
        assert!(!is_loopback(ip("10.0.0.1")));
        assert!(!is_loopback(ip("0.0.0.0")));
        assert!(!is_loopback(ip("fe80::1")));
        assert!(!is_loopback(ip("::ffff:192.168.1.20")));
    }

    #[test]
    fn next_must_be_relative() {
        assert_eq!(safe_next(None), DEFAULT_NEXT);
        assert_eq!(safe_next(Some("/rs/app/documents?tags=1")), "/rs/app/documents?tags=1");
        assert_eq!(safe_next(Some("/")), "/");
        assert_eq!(safe_next(Some("/rs/app/m/")), "/rs/app/m/");
        for bad in [
            "",
            "//evil.example/",
            "/\\evil.example/",
            "\\\\evil.example",
            "https://evil.example/",
            "http://127.0.0.1:58080/",
            "javascript:alert(1)",
            "rs/app/",
            "/a\nb",
            "/a\rLocation: x",
            "/a b",
            "/\tevil",
        ] {
            assert_eq!(safe_next(Some(bad)), DEFAULT_NEXT, "{bad:?}");
        }
        assert_eq!(safe_next(Some(&format!("/{}", "a".repeat(3000)))), DEFAULT_NEXT);
    }

    /// State without database: the pool is never connected (lazy, port 1), throttle counters in
    /// process memory.
    fn state() -> AppState {
        let env = pl_core::config::Env::from_pairs([
            ("PAPERLESS_DBHOST", "127.0.0.1"),
            ("PAPERLESS_DBPORT", "1"),
            ("PAPERLESS_SECRET_KEY", "test-only-secret-key-for-desktop-login"),
        ]);
        let st = AppState::new(pl_core::Config::from_env_map(env).unwrap()).unwrap();
        let _ = crate::cache::Cache::backend_only(&st);
        st
    }

    fn req(peer: &str, query: &str) -> Request<Body> {
        let mut r = Request::builder().uri(format!("{PATH}?{query}")).header("host", "127.0.0.1:58080").body(Body::empty()).unwrap();
        r.extensions_mut().insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
        r
    }

    fn cfg() -> DesktopLogin {
        DesktopLogin::from_lookup(env(&[(TOKEN_ENV, T), (OWNER_ENV, "jane")])).unwrap()
    }

    #[tokio::test]
    async fn refuses_non_loopback_even_with_right_token() {
        let st = state();
        for peer in ["192.168.1.20:5000", "[fe80::1]:5000"] {
            let r = handle(st.clone(), req(peer, &format!("token={T}")), cfg()).await;
            assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN, "{peer}");
            assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
            assert!(r.headers().get(header::SET_COOKIE).is_none());
        }
        // X-Forwarded-For does not change the TCP peer
        let mut r = req("192.168.1.20:5000", &format!("token={T}"));
        r.headers_mut().insert("x-forwarded-for", HeaderValue::from_static("127.0.0.1"));
        assert_eq!(handle(st, r, cfg()).await.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn wrong_or_missing_token_is_403_and_throttled() {
        let st = state();
        let peer = "127.0.0.1:5000";
        for q in ["", "token=", "token=falsch", &format!("token={}", &T[..63])] {
            let r = handle(st.clone(), req(peer, q), cfg()).await;
            assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN, "{q}");
            assert_eq!(r.headers()[header::REFERRER_POLICY], "no-referrer");
            assert!(r.headers().get(header::LOCATION).is_none());
        }
        // up to the limit (10/minute) further failed attempts, after that even the correct key is
        // locked (403 before any database access – the pool here is unreachable)
        for _ in 0..6 {
            let _ = handle(st.clone(), req(peer, "token=x"), cfg()).await;
        }
        let r = handle(st, req(peer, &format!("token={T}")), cfg()).await;
        assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[test]
    fn hardened_headers() {
        let r = harden(forbidden());
        assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(r.headers()[header::REFERRER_POLICY], "no-referrer");
    }
}
