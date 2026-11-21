//! Streamable HTTP transport for MCP (specification 2025-06-18, "Transports").
//!
//! | Request | Response |
//! |---|---|
//! | `POST /mcp` with JSON-RPC request | `200 application/json` (one response) |
//! | `POST /mcp` with notification/response | `202 Accepted`, empty body |
//! | `POST /mcp` with batch (array) | `400` + JSON-RPC `-32600` (batches removed since 2025-06-18) |
//! | invalid JSON | `400` + `-32700` |
//! | missing/invalid token | `401`, `WWW-Authenticate: Bearer realm="paperrust-mcp"` |
//! | `Origin` not allowed | `403` |
//! | rate limit exceeded | `429`, `Retry-After` |
//! | without `Mcp-Session-Id` (except `initialize`) | `400` |
//! | unknown/expired/foreign session | `404` (client must re-initialize) |
//! | `MCP-Protocol-Version` not supported | `400` |
//! | `GET /mcp` | `405` (no server-initiated SSE stream) |
//! | `DELETE /mcp` | `204` (session ended) or `404` |

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};

use super::backend::{Actor, Backend};
use super::{McpService, PROTOCOL_VERSIONS, codes, error_response, negotiate_version};

pub const SESSION_HEADER: &str = "mcp-session-id";
pub const VERSION_HEADER: &str = "mcp-protocol-version";

struct Session {
    user_id: Option<i64>,
    version: String,
    last: Instant,
}

/// Sessions in memory (process-local).
pub struct Sessions {
    inner: Mutex<HashMap<String, Session>>,
    max: usize,
    max_per_user: usize,
    idle: Duration,
}

impl Sessions {
    pub fn new(max: usize, idle_secs: u64) -> Self {
        Self { inner: Mutex::new(HashMap::new()), max, max_per_user: max, idle: Duration::from_secs(idle_secs) }
    }

    /// Upper limit per user (at least 1).
    pub fn per_user(mut self, n: usize) -> Self {
        self.max_per_user = n.max(1);
        self
    }

    /// New session. Security wave 3 AI-N4: only the oldest session of the **same**
    /// user is evicted (per-user upper limit); if the store is full overall and the user has no
    /// session of their own, the request is rejected (`None` → 503) instead of ending a foreign session.
    pub(crate) fn create(&self, user_id: Option<i64>, version: &str) -> Option<String> {
        self.create_at(user_id, version, Instant::now())
    }

    pub(crate) fn create_at(&self, user_id: Option<i64>, version: &str, now: Instant) -> Option<String> {
        let mut g = self.inner.lock().ok()?;
        let idle = self.idle;
        g.retain(|_, s| now.saturating_duration_since(s.last) < idle);
        let own = g.values().filter(|s| s.user_id == user_id).count();
        if own >= self.max_per_user || g.len() >= self.max {
            // evict own sessions only
            let oldest = g.iter().filter(|(_, s)| s.user_id == user_id).min_by_key(|(_, s)| s.last).map(|(k, _)| k.clone())?;
            g.remove(&oldest);
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        g.insert(id.clone(), Session { user_id, version: version.to_string(), last: now });
        Some(id)
    }

    #[cfg(test)]
    fn contains(&self, id: &str) -> bool {
        self.inner.lock().map(|g| g.contains_key(id)).unwrap_or(false)
    }

    /// Checks and touches a session. `None` = unknown/expired/foreign.
    fn touch(&self, id: &str, user_id: Option<i64>) -> Option<String> {
        let mut g = self.inner.lock().ok()?;
        let now = Instant::now();
        let s = g.get_mut(id)?;
        if now.duration_since(s.last) >= self.idle || s.user_id != user_id {
            return None;
        }
        s.last = now;
        Some(s.version.clone())
    }

    fn remove(&self, id: &str, user_id: Option<i64>) -> bool {
        let Ok(mut g) = self.inner.lock() else { return false };
        match g.get(id) {
            Some(s) if s.user_id == user_id => g.remove(id).is_some(),
            _ => false,
        }
    }
}

/// Short cache of successful logins (key: SHA-256 of the token).
/// Security wave 3 M3.
pub struct AuthCache {
    ttl: Duration,
    inner: Mutex<HashMap<[u8; 32], (Actor, Instant)>>,
}

const AUTH_CACHE_MAX: usize = 1000;

impl AuthCache {
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, inner: Mutex::new(HashMap::new()) }
    }

    fn key(token: &str) -> [u8; 32] {
        use sha2::Digest as _;
        sha2::Sha256::digest(token.as_bytes()).into()
    }

    fn get(&self, token: &str) -> Option<Actor> {
        if self.ttl.is_zero() {
            return None;
        }
        let g = self.inner.lock().ok()?;
        let (a, t) = g.get(&Self::key(token))?;
        (t.elapsed() < self.ttl && a.credential.expose() == token).then(|| a.clone())
    }

    fn put(&self, token: &str, actor: &Actor) {
        if self.ttl.is_zero() {
            return;
        }
        let Ok(mut g) = self.inner.lock() else { return };
        if g.len() >= AUTH_CACHE_MAX {
            let ttl = self.ttl;
            g.retain(|_, (_, t)| t.elapsed() < ttl);
            if g.len() >= AUTH_CACHE_MAX {
                g.clear();
            }
        }
        g.insert(Self::key(token), (actor.clone(), Instant::now()));
    }
}

fn too_many(wait: u64) -> Response {
    let mut r = rpc_error(StatusCode::TOO_MANY_REQUESTS, -32003, "Rate limit exceeded");
    if let Ok(v) = HeaderValue::from_str(&wait.to_string()) {
        r.headers_mut().insert(header::RETRY_AFTER, v);
    }
    r
}

/// Key of the global emergency brake for failed attempts (security review final M2).
const AUTH_FAIL_GLOBAL_KEY: &str = "auth-failures";

/// Does `ip` match an entry of `PAPERLESS_TRUSTED_PROXIES` (`a.b.c.d`, `a.b.c.d/nn`,
/// IPv6 accordingly)?
fn ip_in(ip: IpAddr, nets: &[String]) -> bool {
    let ip = canonical_ip(ip);
    nets.iter().any(|n| {
        let n = n.trim();
        let (addr, bits) = match n.split_once('/') {
            Some((a, b)) => match b.trim().parse::<u32>() {
                Ok(b) => (a, Some(b)),
                Err(_) => return false,
            },
            None => (n, None),
        };
        let Ok(net) = addr.trim().parse::<IpAddr>() else { return false };
        match (ip, canonical_ip(net)) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                let bits = bits.unwrap_or(32).min(32);
                let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
                u32::from(a) & mask == u32::from(b) & mask
            }
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                let bits = bits.unwrap_or(128).min(128);
                let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
                u128::from(a) & mask == u128::from(b) & mask
            }
            _ => false,
        }
    })
}

/// IPv4-mapped IPv6 (`::ffff:a.b.c.d`) as IPv4.
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Client IP for rate limiting and failed-attempt counter (security review final M2/M3):
/// the peer of the TCP connection; only if it is a trusted proxy, the
/// rightmost address from `X-Forwarded-For` that is not itself a trusted proxy.
/// Key: IPv4 in full, IPv6 per /64 (a connection typically has a whole /64).
pub fn client_key(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[String]) -> String {
    let Some(peer) = peer.map(canonical_ip) else { return "ip:unknown".to_string() };
    let mut client = peer;
    if ip_in(peer, trusted) {
        let xff: Vec<&str> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect();
        for hop in xff.iter().rev() {
            let Ok(ip) = hop.parse::<IpAddr>().map(canonical_ip) else { break };
            client = ip;
            if !ip_in(ip, trusted) {
                break;
            }
        }
    }
    match client {
        IpAddr::V4(v4) => format!("ip:{v4}"),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("ip6:{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let mut r = (status, axum::Json(body)).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn rpc_error(status: StatusCode, code: i64, msg: &str) -> Response {
    json_response(status, error_response(None, code, msg))
}

/// Bearer/Token from `Authorization`.
pub fn extract_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, rest) = v.split_once(' ')?;
    let rest = rest.trim();
    if (scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("token")) && !rest.is_empty() && !rest.contains(' ') {
        Some(rest.to_string())
    } else {
        None
    }
}

/// Check `Origin`: missing → allowed (non-browser clients); otherwise it must be in
/// `PAPERRUST_AI_MCP_ALLOWED_ORIGINS` (`*` = all, insecure) or be a loopback origin
/// (`localhost`, `127.0.0.1`, `[::1]`).
///
/// Review 2 N2: no comparison with the `Host` header any more – with DNS rebinding Origin and
/// Host match (`rebind.attacker:8080`), and that is exactly what the check is meant to protect
/// against according to the MCP specification.
pub fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    if origin == "null" {
        return false;
    }
    if allowed.iter().any(|a| a == "*" || a.eq_ignore_ascii_case(origin)) {
        return true;
    }
    is_loopback_origin(origin)
}

/// `http(s)://localhost[:port]`, `http(s)://127.0.0.1[:port]`, `http(s)://[::1][:port]`.
fn is_loopback_origin(origin: &str) -> bool {
    let lower = origin.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("http://").or_else(|| lower.strip_prefix("https://")) else { return false };
    // "" or ":<digits>"
    let port_ok = |p: &str| p.is_empty() || p.strip_prefix(':').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    if let Some(p) = rest.strip_prefix("[::1]") {
        return port_ok(p);
    }
    ["localhost", "127.0.0.1"].iter().any(|h| rest.strip_prefix(h).is_some_and(port_ok))
}

fn unauthorized(msg: &str) -> Response {
    let mut r = rpc_error(StatusCode::UNAUTHORIZED, -32001, msg);
    r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"paperrust-mcp\""));
    r
}

#[allow(clippy::result_large_err)] // a response as an error is the point here
async fn authenticate<B: Backend>(svc: &McpService<B>, peer: Option<IpAddr>, headers: &HeaderMap) -> Result<Actor, Response> {
    if !origin_allowed(headers, &svc.settings.allowed_origins) {
        return Err(rpc_error(StatusCode::FORBIDDEN, -32002, "Origin not allowed"));
    }
    let Some(token) = extract_token(headers) else {
        return Err(unauthorized("Authentication required: send 'Authorization: Token <api token>'"));
    };
    let actor = match svc.auth_cache.get(&token) {
        Some(a) => a,
        None => {
            // Security wave 3 M3 / security review final M2+M3: rate limiting BEFORE the
            // token check, per client IP (not per token: changing tokens would otherwise create a new
            // entry per request), so that unauthenticated requests trigger no internal
            // API call. Recently logged-in tokens get through via the cache
            // and count only against their own limit.
            let ip = client_key(peer, headers, &svc.settings.trusted_proxies);
            if let Err(wait) = svc.limiter.check_request(&format!("pre:{ip}")) {
                return Err(too_many(wait));
            }
            // Failed attempts per client IP; global only as an emergency brake with a high threshold.
            if let Some(wait) = svc.auth_failures.exhausted(&ip).or_else(|| svc.auth_failures_global.exhausted(AUTH_FAIL_GLOBAL_KEY)) {
                return Err(too_many(wait));
            }
            match svc.backend.authenticate(&token).await {
                Ok(Some(a)) if a.principal.is_active => {
                    svc.auth_cache.put(&token, &a);
                    a
                }
                Ok(_) => {
                    svc.auth_failures.record(&ip);
                    svc.auth_failures_global.record(AUTH_FAIL_GLOBAL_KEY);
                    return Err(unauthorized("Invalid token."));
                }
                Err(super::backend::BackendError::PermissionDenied(m)) => return Err(rpc_error(StatusCode::FORBIDDEN, -32002, &m)),
                Err(_) => return Err(rpc_error(StatusCode::INTERNAL_SERVER_ERROR, codes::INTERNAL_ERROR, "Internal error")),
            }
        }
    };
    if let Err(wait) = svc.limiter.check_request(&actor.rate_key) {
        return Err(too_many(wait));
    }
    Ok(actor)
}

fn accepts_json(headers: &HeaderMap) -> bool {
    match headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(a) => a
            .split(',')
            .map(|p| p.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
            .any(|m| m == "application/json" || m == "application/*" || m == "*/*" || m == "text/event-stream"),
    }
}

/// `POST /mcp`.
pub async fn handle_post<B: Backend>(svc: &McpService<B>, headers: &HeaderMap, body: &[u8]) -> Response {
    handle_post_from(svc, None, headers, body).await
}

/// `POST /mcp` with a known peer (`peer` = TCP peer, for per-client-IP limits).
pub async fn handle_post_from<B: Backend>(svc: &McpService<B>, peer: Option<IpAddr>, headers: &HeaderMap, body: &[u8]) -> Response {
    let actor = match authenticate(svc, peer, headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !accepts_json(headers) {
        return rpc_error(StatusCode::NOT_ACCEPTABLE, codes::INVALID_REQUEST, "Accept must include application/json");
    }
    let msg: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return rpc_error(StatusCode::BAD_REQUEST, codes::PARSE_ERROR, "Parse error"),
    };
    if msg.is_array() {
        return rpc_error(StatusCode::BAD_REQUEST, codes::INVALID_REQUEST, "Batch requests are not supported");
    }
    if !msg.is_object() {
        return rpc_error(StatusCode::BAD_REQUEST, codes::INVALID_REQUEST, "Invalid Request");
    }
    for v in headers.get_all(VERSION_HEADER).iter() {
        if !v.to_str().is_ok_and(|v| PROTOCOL_VERSIONS.contains(&v)) {
            return rpc_error(StatusCode::BAD_REQUEST, codes::INVALID_REQUEST, "Unsupported MCP-Protocol-Version");
        }
    }
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "initialize" {
        let requested = msg.pointer("/params/protocolVersion").and_then(Value::as_str);
        let version = negotiate_version(requested);
        let Some(resp) = svc.handle_message(&actor, &msg).await else {
            return StatusCode::ACCEPTED.into_response();
        };
        let Some(sid) = svc.sessions.create(actor.user_id(), version) else {
            return rpc_error(StatusCode::SERVICE_UNAVAILABLE, codes::INTERNAL_ERROR, "Too many sessions; try again later");
        };
        let mut r = json_response(StatusCode::OK, resp);
        if let Ok(v) = HeaderValue::from_str(&sid) {
            r.headers_mut().insert(SESSION_HEADER, v);
        }
        return r;
    }
    let Some(sid) = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok()) else {
        return rpc_error(StatusCode::BAD_REQUEST, codes::INVALID_REQUEST, "Missing Mcp-Session-Id; call initialize first");
    };
    if svc.sessions.touch(sid, actor.user_id()).is_none() {
        return rpc_error(StatusCode::NOT_FOUND, codes::INVALID_REQUEST, "Session not found; initialize a new session");
    }
    match svc.handle_message(&actor, &msg).await {
        None => StatusCode::ACCEPTED.into_response(),
        Some(resp) => json_response(StatusCode::OK, resp),
    }
}

/// `DELETE /mcp`.
pub async fn handle_delete<B: Backend>(svc: &McpService<B>, headers: &HeaderMap) -> Response {
    handle_delete_from(svc, None, headers).await
}

/// `DELETE /mcp` with a known peer.
pub async fn handle_delete_from<B: Backend>(svc: &McpService<B>, peer: Option<IpAddr>, headers: &HeaderMap) -> Response {
    let actor = match authenticate(svc, peer, headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let Some(sid) = headers.get(SESSION_HEADER).and_then(|v| v.to_str().ok()) else {
        return rpc_error(StatusCode::BAD_REQUEST, codes::INVALID_REQUEST, "Missing Mcp-Session-Id");
    };
    if svc.sessions.remove(sid, actor.user_id()) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        rpc_error(StatusCode::NOT_FOUND, codes::INVALID_REQUEST, "Session not found")
    }
}

/// `GET /mcp`: no server-initiated stream.
pub fn handle_get() -> Response {
    let mut r = StatusCode::METHOD_NOT_ALLOWED.into_response();
    r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST, DELETE"));
    r
}

/// TCP peer from the request extensions (`ConnectInfo`, set by pl-server or
/// `into_make_service_with_connect_info`).
pub fn peer_ip(ext: &axum::http::Extensions) -> Option<IpAddr> {
    ext.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip())
}

async fn post_h<B: Backend>(
    State(svc): State<Arc<McpService<B>>>,
    ext: axum::http::Extensions,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_post_from(&svc, peer_ip(&ext), &headers, &body).await
}

async fn delete_h<B: Backend>(State(svc): State<Arc<McpService<B>>>, ext: axum::http::Extensions, headers: HeaderMap) -> Response {
    handle_delete_from(&svc, peer_ip(&ext), &headers).await
}

async fn get_h() -> Response {
    handle_get()
}

/// Standalone router (`/mcp`) – for tests and embedded use.
pub fn router<B: Backend>(svc: Arc<McpService<B>>) -> Router {
    let limit = svc.settings.max_upload_bytes.saturating_mul(4) / 3 + 256 * 1024;
    Router::new()
        .route("/mcp", post(post_h::<B>).get(get_h).delete(delete_h::<B>))
        .layer(axum::extract::DefaultBodyLimit::max(limit))
        .with_state(svc)
}

/// Small JSON-RPC test client (for local tests only).
pub mod testclient {
    use super::*;

    pub struct Client {
        pub base: String,
        pub token: Option<String>,
        pub session: Option<String>,
        pub http: reqwest::Client,
        next_id: i64,
    }

    impl Client {
        pub fn new(base: &str, token: Option<&str>) -> Self {
            Self { base: base.to_string(), token: token.map(str::to_string), session: None, http: reqwest::Client::new(), next_id: 1 }
        }

        pub async fn raw(&self, body: &Value, extra: &[(&str, &str)]) -> Result<(u16, HeaderMap, Value), reqwest::Error> {
            let mut req = self.http.post(format!("{}/mcp", self.base)).header("accept", "application/json, text/event-stream").json(body);
            if let Some(t) = &self.token {
                req = req.header("authorization", format!("Token {t}"));
            }
            if let Some(s) = &self.session {
                req = req.header(SESSION_HEADER, s).header(VERSION_HEADER, "2025-06-18");
            }
            for (k, v) in extra {
                req = req.header(*k, *v);
            }
            let r = req.send().await?;
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let text = r.text().await?;
            let v = serde_json::from_str(&text).unwrap_or(Value::Null);
            Ok((status, headers, v))
        }

        pub async fn initialize(&mut self) -> Result<Value, reqwest::Error> {
            let body = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "pl-ai-test", "version": "0"}}});
            let (_, h, v) = self.raw(&body, &[]).await?;
            self.session = h.get(SESSION_HEADER).and_then(|x| x.to_str().ok()).map(str::to_string);
            let _ = self.raw(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), &[]).await?;
            Ok(v)
        }

        pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, reqwest::Error> {
            self.next_id += 1;
            let (_, _, v) = self.raw(&json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}), &[]).await?;
            Ok(v)
        }

        /// `tools/call` → `result`.
        pub async fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, reqwest::Error> {
            let v = self.request("tools/call", json!({"name": tool, "arguments": arguments})).await?;
            Ok(v.get("result").cloned().unwrap_or(v))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(origin: &str, host: &str) -> HeaderMap {
        let mut m = HeaderMap::new();
        m.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        m.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        m
    }

    /// Review 2 N2: DNS rebinding (origin host == Host header) is no longer let through.
    #[test]
    fn origin_check_resists_dns_rebinding() {
        assert!(!origin_allowed(&h("http://rebind.attacker:8080", "rebind.attacker:8080"), &[]));
        assert!(origin_allowed(&h("http://localhost:5173", "x"), &[]));
        assert!(origin_allowed(&h("http://127.0.0.1", "x"), &[]));
        assert!(origin_allowed(&h("http://[::1]:8080", "x"), &[]));
        assert!(!origin_allowed(&h("http://localhost.attacker", "x"), &[]));
        assert!(!origin_allowed(&h("http://127.0.0.1.nip.io", "x"), &[]));
        assert!(origin_allowed(&h("https://paperless.example", "x"), &["https://paperless.example".into()]));
        assert!(!origin_allowed(&h("null", "x"), &["*".into()]));
        assert!(origin_allowed(&HeaderMap::new(), &[]));
    }

    /// Security wave 3 AI-N4: a user cannot evict foreign sessions via an `initialize` loop
    /// – only their own (per-user upper limit).
    #[test]
    fn session_eviction_only_own() {
        let s = Sessions::new(4, 3600).per_user(2);
        let t0 = Instant::now();
        let victim = s.create_at(Some(1), "v", t0).unwrap();
        let victim2 = s.create_at(Some(2), "v", t0).unwrap();
        let mut own = Vec::new();
        for i in 0..50u64 {
            own.push(s.create_at(Some(3), "v", t0 + Duration::from_millis(i + 1)).unwrap());
        }
        assert!(s.contains(&victim) && s.contains(&victim2), "fremde Sitzungen bleiben");
        assert_eq!(own.iter().filter(|id| s.contains(id)).count(), 2, "höchstens 2 eigene");
        assert!(s.contains(own.last().unwrap()), "neueste eigene bleibt");
        // store full (4) and user 4 without a session of their own → rejected, nothing evicted
        assert!(s.create_at(Some(4), "v", t0 + Duration::from_secs(1)).is_none());
        assert!(s.contains(&victim) && s.contains(&victim2));
    }
}
