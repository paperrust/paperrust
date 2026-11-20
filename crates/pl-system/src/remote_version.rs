//! `GET /api/remote_version/` – update check against the **PaperRust** releases.
//!
//! upstream: src/documents/views.py@v3.4.2 (`RemoteVersionView.get`: GitHub `releases/latest`,
//! `tag_name`, cache 15 min, error → `"0.0.0"`, `update_available = remote > current`).
//! PaperRust (R4, 2026-11-09): the query goes to `https://github.com/paperrust/paperrust`
//! (GitHub API), compared with the PaperRust version (not with paperless-ngx). Response shape the same for
//! the unchanged UI (`version`, `update_available`), plus `paperrust`.
//!
//! * can be switched off: `PAPERRUST_ENABLE_UPDATE_CHECK=false` (old `PAPERLESS_ENABLE_UPDATE_CHECK`) →
//!   no outgoing request.
//! * Timeout 3 s (connection 2 s), no redirects, response at most 256 KiB.
//! * Cache: hit 15 min, failure 60 s; concurrent calls share one request.
//! * without network / invalid response: `version = "0.0.0"`, `update_available = false`,
//!   `paperrust.status = "unknown"` (never 500).
//!
//! No permission check (DRF `AllowAny`, like upstream).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use pl_authz::MaybeUser;
use pl_core::{AppState, Config, Drf, RequestContext};
use serde_json::{Map, Value};

use crate::error::Result;
use crate::pep440::Version;
use pl_core::brand::{PAPERRUST_VERSION, RELEASES_API_URL, REPO_URL, USER_AGENT};

/// Version to compare against.
pub const CURRENT_VERSION: &str = PAPERRUST_VERSION;
/// The GitHub query (latest PaperRust release).
pub const GITHUB_LATEST_URL: &str = RELEASES_API_URL;
/// Response for "unknown" (like upstream on errors).
pub const UNKNOWN_VERSION: &str = "0.0.0";
/// Validity of a hit (`60 * 15` like upstream).
const CACHE_TTL: Duration = Duration::from_secs(60 * 15);
/// Validity of a failure (review 2 N15: no request per anonymous call).
const NEGATIVE_TTL: Duration = Duration::from_secs(60);
/// Overall timeout of the GitHub request.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(3);
/// Connection timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Largest accepted response (GitHub returns a few KiB).
const MAX_BODY: usize = 256 * 1024;

/// Result of the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Latest release (without prefix `v`).
    Latest(String),
    /// Network error, status ≠ 2xx, invalid response.
    Unknown,
    /// Check switched off.
    Disabled,
}

/// Cache (result, stored at, validity).
#[derive(Default)]
pub struct VersionCache {
    slot: Mutex<Option<(Check, Instant, Duration)>>,
    /// Single-flight: only one outgoing request at a time.
    flight: tokio::sync::Mutex<()>,
}

impl VersionCache {
    pub const fn new() -> Self {
        Self { slot: Mutex::new(None), flight: tokio::sync::Mutex::const_new(()) }
    }

    fn get(&self) -> Option<Check> {
        let g = self.slot.lock().ok()?;
        match &*g {
            Some((v, at, ttl)) if at.elapsed() < *ttl => Some(v.clone()),
            _ => None,
        }
    }

    fn set(&self, v: Check, ttl: Duration) {
        if let Ok(mut g) = self.slot.lock() {
            *g = Some((v, Instant::now(), ttl));
        }
    }
}

static CACHE: VersionCache = VersionCache::new();

/// `tag_name` → version number: prefixes `paperrust-` and `v`/`V` removed; must be PEP 440.
pub fn parse_body(body: &[u8]) -> Check {
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(body) else {
        return Check::Unknown;
    };
    let Some(Value::String(tag)) = m.get("tag_name") else {
        return Check::Unknown;
    };
    let t = tag.trim();
    let t = t.strip_prefix("paperrust-").unwrap_or(t);
    let t = t.strip_prefix(['v', 'V']).unwrap_or(t);
    if t.is_empty() || t.len() > 64 || Version::parse(t).is_err() {
        return Check::Unknown;
    }
    Check::Latest(t.to_string())
}

/// One GitHub request (`Accept: application/vnd.github+json`), status ≠ 2xx → unknown.
pub async fn fetch(client: &reqwest::Client, url: &str) -> Check {
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(FETCH_TIMEOUT)
        .send()
        .await;
    let mut resp = match resp {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::debug!("PaperRust update check: status {}", r.status());
            return Check::Unknown;
        }
        Err(e) => {
            tracing::debug!("PaperRust update check failed: {e}");
            return Check::Unknown;
        }
    };
    let mut body = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(c)) => {
                if body.len() + c.len() > MAX_BODY {
                    return Check::Unknown;
                }
                body.extend_from_slice(&c);
            }
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("PaperRust update check failed: {e}");
                return Check::Unknown;
            }
        }
    }
    parse_body(&body)
}

/// Response for a check result against `current`.
pub fn response_body(check: &Check, current: &str) -> Value {
    let (version, update_available) = match check {
        Check::Latest(v) => {
            let newer = match (Version::parse(v), Version::parse(current)) {
                (Ok(remote), Ok(cur)) => remote > cur,
                _ => false,
            };
            (v.as_str(), newer)
        }
        Check::Unknown | Check::Disabled => (UNKNOWN_VERSION, false),
    };
    let status = match check {
        Check::Latest(_) => "ok",
        Check::Unknown => "unknown",
        Check::Disabled => "disabled",
    };
    let mut pr = Map::new();
    pr.insert("current_version".into(), Value::from(current));
    pr.insert(
        "latest_version".into(),
        match check {
            Check::Latest(v) => Value::from(v.as_str()),
            _ => Value::Null,
        },
    );
    pr.insert("status".into(), Value::from(status));
    pr.insert("releases_url".into(), Value::from(format!("{REPO_URL}/releases")));
    let mut m = Map::new();
    m.insert("version".into(), Value::from(version));
    m.insert("update_available".into(), Value::Bool(update_available));
    m.insert("paperrust".into(), Value::Object(pr));
    Value::Object(m)
}

/// Check with cache and single-flight.
pub async fn check_with(cache: &VersionCache, client: &reqwest::Client, url: &str) -> Check {
    if let Some(v) = cache.get() {
        return v;
    }
    let _guard = cache.flight.lock().await;
    // another call has asked in the meantime
    if let Some(v) = cache.get() {
        return v;
    }
    let v = fetch(client, url).await;
    let ttl = if matches!(v, Check::Latest(_)) { CACHE_TTL } else { NEGATIVE_TTL };
    cache.set(v.clone(), ttl);
    v
}

/// Is the update check enabled? Off only with an explicit
/// `PAPERRUST_ENABLE_UPDATE_CHECK=false` (= upstream `PAPERLESS_ENABLE_UPDATE_CHECK`, alias from
/// pl-core). Default (`"default"`) and `true`: on – the UI additionally asks the user.
pub fn update_check_enabled(cfg: &Config) -> bool {
    cfg.enable_update_check != Some(false)
}

/// Query URL: test hook `PAPERRUST_REMOTE_VERSION_URL` (alt `PAPERLESS_RS_REMOTE_VERSION_URL`).
fn check_url(cfg: &Config) -> &str {
    cfg.env
        .get("PAPERRUST_REMOTE_VERSION_URL")
        .or_else(|| cfg.env.get("PAPERLESS_RS_REMOTE_VERSION_URL"))
        .unwrap_or(GITHUB_LATEST_URL)
}

fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(FETCH_TIMEOUT)
            // GitHub requires a User-Agent
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_default()
    })
}

/// `GET /api/remote_version/`
pub async fn get(State(state): State<AppState>, _ctx: RequestContext, _auth: MaybeUser) -> Result<Response> {
    let cfg = state.config();
    let check = if update_check_enabled(cfg) {
        check_with(&CACHE, client(), check_url(cfg)).await
    } else {
        Check::Disabled
    };
    Ok(Drf::ok(response_body(&check, CURRENT_VERSION)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies() {
        assert_eq!(parse_body(br#"{"tag_name": "v1.6.0"}"#), Check::Latest("1.6.0".into()));
        assert_eq!(parse_body(br#"{"tag_name": "0.2.0"}"#), Check::Latest("0.2.0".into()));
        assert_eq!(parse_body(br#"{"tag_name": "paperrust-v0.3.1"}"#), Check::Latest("0.3.1".into()));
        assert_eq!(parse_body(br#"{ "blah":"#), Check::Unknown);
        assert_eq!(parse_body(br#"{"x": 1}"#), Check::Unknown);
        assert_eq!(parse_body(br#"[1]"#), Check::Unknown);
        assert_eq!(parse_body(br#"{"tag_name": "nightly"}"#), Check::Unknown);
        assert_eq!(parse_body(br#"{"tag_name": ""}"#), Check::Unknown);
        assert_eq!(parse_body(br#"{"tag_name": 5}"#), Check::Unknown);
    }

    #[test]
    fn update_available() {
        let b = response_body(&Check::Latest("0.0.9".into()), "0.1.0");
        assert_eq!(b["version"], "0.0.9");
        assert_eq!(b["update_available"], false);
        assert_eq!(response_body(&Check::Latest("0.1.0".into()), "0.1.0")["update_available"], false);
        assert_eq!(response_body(&Check::Latest("0.2.0".into()), "0.1.0")["update_available"], true);
        let u = response_body(&Check::Unknown, "0.1.0");
        assert_eq!(u["version"], UNKNOWN_VERSION);
        assert_eq!(u["update_available"], false);
        assert_eq!(u["paperrust"]["status"], "unknown");
        assert_eq!(u["paperrust"]["latest_version"], Value::Null);
        assert_eq!(response_body(&Check::Disabled, "0.1.0")["paperrust"]["status"], "disabled");
        // Key order: the UI reads version + update_available
        let keys: Vec<&String> = u.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["version", "update_available", "paperrust"]);
    }

    #[test]
    fn switch() {
        let cfg = |pairs: &[(&str, &str)]| {
            let mut all = vec![("PAPERLESS_SECRET_KEY", "test-only-secret-key-0123456789")];
            all.extend_from_slice(pairs);
            Config::from_env_map(pl_core::config::Env::from_pairs(all)).unwrap()
        };
        assert!(update_check_enabled(&cfg(&[])));
        assert!(!update_check_enabled(&cfg(&[("PAPERLESS_ENABLE_UPDATE_CHECK", "false")])));
        assert!(update_check_enabled(&cfg(&[("PAPERLESS_ENABLE_UPDATE_CHECK", "true")])));
        assert!(update_check_enabled(&cfg(&[("PAPERLESS_ENABLE_UPDATE_CHECK", "default")])));
    }

    /// Local HTTP stub (no external network): one canned response per connection.
    async fn stub(status: &'static str, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}/releases/latest")
    }

    #[tokio::test]
    async fn fetch_against_local_stub() {
        let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let ok = stub("200 OK", r#"{"tag_name":"v1.6.0"}"#).await;
        assert_eq!(fetch(&c, &ok).await, Check::Latest("1.6.0".into()));
        let err = stub("503 Service Unavailable", "").await;
        assert_eq!(fetch(&c, &err).await, Check::Unknown);
        let redirect = stub("301 Moved Permanently", "").await;
        assert_eq!(fetch(&c, &redirect).await, Check::Unknown);
    }

    /// No server reachable → cleanly "unknown" within the timeout.
    #[tokio::test]
    async fn offline_is_unknown() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let start = Instant::now();
        let cache = VersionCache::new();
        let v = check_with(&cache, client(), &format!("http://{addr}/x")).await;
        assert_eq!(v, Check::Unknown);
        assert!(start.elapsed() <= FETCH_TIMEOUT + Duration::from_secs(1));
    }

    /// Review 2 N15: a failed check is cached for a while (one outbound request, not one per call),
    /// also for concurrent callers (single flight).
    #[tokio::test]
    async fn failure_is_negatively_cached() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let hits = Arc::new(AtomicUsize::new(0));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let h = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                h.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            }
        });
        let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let url = format!("http://{addr}/releases/latest");
        let cache = Arc::new(VersionCache::new());
        let mut js = Vec::new();
        for _ in 0..4 {
            let (cache, c, url) = (cache.clone(), c.clone(), url.clone());
            js.push(tokio::spawn(async move { check_with(&cache, &c, &url).await }));
        }
        for j in js {
            assert_eq!(j.await.unwrap(), Check::Unknown);
        }
        assert_eq!(check_with(&cache, &c, &url).await, Check::Unknown);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
