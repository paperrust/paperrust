//! Outermost layer: internal health endpoint, what Django's middleware decides before any view
//! (CORS preflight, disallowed host), Django-like response finishing, access log.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::FutureExt;

use crate::compress;
use crate::config::DjangoSettings;
use crate::django_mw::{self, RequestInfo};

/// Prefix for PaperRust internal endpoints; no upstream URL starts with it
/// (upstream: src/paperless/urls.py@v3.4.2 – Django answers 404 there).
pub const INTERNAL_PREFIX: &str = "/__rs/";
pub const HEALTH_PATH: &str = "/__rs/health";

pub struct Frontline {
    pub settings: DjangoSettings,
    pub rust_routes: usize,
    /// PaperRust branding (`PAPERRUST_BRANDING`); `enabled == false` = byte-transparent.
    pub branding: crate::branding::Branding,
    /// Native `/ws/status/` (`None` = no event bus, e.g. tests: the path is then ordinary HTTP).
    pub status_ws: Option<StatusSocket>,
}

/// What the native status socket needs (see `crate::ws`).
#[derive(Clone)]
pub struct StatusSocket {
    pub authz: pl_authz::Authz,
    pub bus: pl_queue::EventBus,
}

fn peer(req: &Request) -> Option<SocketAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0)
}

/// What Django's middleware decides before any view. Percent-encoded paths are routed in Rust
/// (axum matches the raw path; see the contract, Known differences); a CORS preflight is answered
/// like `CorsMiddleware`; a disallowed host gets Django's 400 page.
fn before_views(req: &Request, s: &DjangoSettings) -> Option<Response> {
    if req.method() == Method::OPTIONS && req.headers().contains_key("access-control-request-method") {
        let mut r = Response::new(Body::empty());
        r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        r.extensions_mut().insert(django_mw::PreflightOnly);
        return Some(r);
    }
    if !django_mw::host_allowed(req.headers(), s) {
        tracing::warn!(target: "django.security.DisallowedHost", "Invalid HTTP_HOST header");
        return Some(pl_core::ApiError::django_bad_request().into_response());
    }
    None
}

/// Body of `/__rs/health`; `instance` (from `PAPERRUST_INSTANCE_ID`) only when set and non-empty.
pub fn health_body(rust_routes: usize, instance: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "status": "ok",
        "service": pl_core::brand::BIN_NAME,
        "version": pl_core::brand::PAPERRUST_VERSION,
        "rust_routes": rust_routes,
    });
    if let Some(i) = instance.filter(|i| !i.trim().is_empty()) {
        body["instance"] = serde_json::Value::from(i);
    }
    body
}

fn health(f: &Frontline) -> Response {
    let instance = pl_core::config::var("PAPERRUST_INSTANCE_ID");
    let body = health_body(f.rust_routes, instance.as_deref());
    let mut r = Response::new(Body::from(body.to_string()));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Request-scoped timing shared by the access log and [`crate::body::gate`]. Cloned into the
/// request extensions; all clones share the same counters.
#[derive(Clone, Default)]
pub struct AccessTrace(Arc<TraceInner>);

#[derive(Default)]
struct TraceInner {
    /// Microseconds the body gate spent buffering the request body (+1, 0 = not set).
    gate_us: AtomicU64,
}

impl AccessTrace {
    pub fn set_gate(&self, d: std::time::Duration) {
        self.0.gate_us.store(d.as_micros() as u64 + 1, Ordering::Relaxed);
    }
    fn ms(v: &AtomicU64) -> Ms {
        match v.load(Ordering::Relaxed) {
            0 => Ms(None),
            us => Ms(Some((us - 1) / 1000)),
        }
    }
}

/// `upstream_ms=12` / `upstream_ms=-` in the access log.
struct Ms(Option<u64>);

impl std::fmt::Display for Ms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(v) => write!(f, "{v}"),
            None => f.write_str("-"),
        }
    }
}

/// Writes the access-log line even when the request future is dropped before a response
/// exists (client closed the connection, e.g. navigated away while the handler was working):
/// then `status=499` (nginx "client closed request") and `outcome="client closed"`.
struct AccessGuard {
    start: Instant,
    method: Method,
    path: String,
    peer: Option<SocketAddr>,
    trace: AccessTrace,
    /// Branch taken (`rust` or `internal`).
    via: &'static str,
    done: bool,
}

impl AccessGuard {
    fn emit(&self, status: u16, via: &'static str, outcome: &'static str) {
        tracing::info!(
            target: "paperrust::access",
            method = %self.method,
            path = %log_path(&self.path),
            status,
            ms = self.start.elapsed().as_millis() as u64,
            via,
            peer = %self.peer.map(|p| p.ip().to_string()).unwrap_or_default(),
            gate_ms = %AccessTrace::ms(&self.trace.0.gate_us),
            outcome,
        );
    }
}

impl Drop for AccessGuard {
    fn drop(&mut self) {
        if !self.done {
            self.emit(499, self.via, "client closed");
        }
    }
}

/// Path for the access log: share links are capability URLs and are redacted.
pub fn log_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/share/")
        && !rest.is_empty()
    {
        return "/share/<redacted>".to_owned();
    }
    path.to_owned()
}

/// `/__rs/health` only for direct local callers. Review 2 N1: behind a reverse proxy on the
/// same host every client appears as `127.0.0.1`; a forwarding header marks such requests as
/// not local.
pub fn health_is_local(peer: Option<std::net::SocketAddr>, headers: &axum::http::HeaderMap) -> bool {
    peer.is_some_and(|p| p.ip().to_canonical().is_loopback())
        && !["x-forwarded-for", "forwarded", "x-real-ip"].iter().any(|h| headers.contains_key(*h))
}

pub async fn frontline(State(f): State<Arc<Frontline>>, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let peer_addr = peer(&req);
    let trace = AccessTrace::default();
    let mut guard = AccessGuard {
        start: Instant::now(),
        method: method.clone(),
        path: path.clone(),
        peer: peer_addr,
        trace: trace.clone(),
        via: "rust",
        done: false,
    };
    let mut req = req;
    req.extensions_mut().insert(trace);

    // Branding (HTML navigations only): the handler answers uncompressed so the style can be
    // inserted; the response is then compressed here by Django's rules with the original
    // Accept-Encoding.
    let brand = f.branding.is_candidate(&method, &path, req.headers());
    let original_ae =
        django_mw::header_str(req.headers(), &header::ACCEPT_ENCODING).unwrap_or_default();
    if brand {
        req.headers_mut().remove(header::ACCEPT_ENCODING);
    }

    let (resp, via) = if path == HEALTH_PATH
        && (method == Method::GET || method == Method::HEAD)
        && health_is_local(peer_addr, req.headers())
    {
        (health(&f), "internal")
    } else if let Some(asset) = f
        .branding
        .enabled
        .then(|| crate::branding::asset_response(&method, &path))
        .flatten()
    {
        (asset, "internal")
    } else if let Some(ws) = f
        .status_ws
        .as_ref()
        .filter(|_| path == crate::ws::PATH && req.headers().contains_key(header::UPGRADE))
    {
        // Channels routes websockets without Django's HTTP middleware (no host check). A plain
        // HTTP GET on /ws/status/ is ordinary Django HTTP (urlpatterns → IndexView catch-all).
        let resp = crate::ws::status_socket(axum::Extension(ws.authz.clone()), axum::Extension(ws.bus.clone()), req).await;
        (resp, "rust")
    } else {
        let mut info = RequestInfo::capture(&method, &path, req.headers(), &f.settings);
        info.accept_encoding = original_ae.clone();
        // pl-authz notes authentication / session-cookie deletion here.
        let notes = pl_core::RequestNotes::new();
        let mut req = req;
        req.extensions_mut().insert(notes.clone());
        let early = before_views(&req, &f.settings);
        // A panicking handler must not kill the connection: answer like Django's
        // generic 500 page (the middleware headers still apply, as in Django).
        let mut resp = match early {
            Some(r) => r,
            None => match std::panic::AssertUnwindSafe(next.run(req)).catch_unwind().await {
                Ok(r) => r,
                Err(_) => {
                    tracing::error!(method = %method, path = %log_path(&path), "handler panicked");
                    pl_core::ApiError::internal("handler panicked").into_response()
                }
            },
        };
        apply_notes(&mut resp, &notes);
        django_mw::finish_headers(&mut resp, &info, &f.settings);
        if brand {
            resp = f.branding.inject_response(resp).await;
        }
        if compression_enabled(&f.settings) {
            resp = compress::compress(resp, &info.accept_encoding).await;
        }
        (resp, "rust")
    };

    guard.done = true;
    guard.emit(resp.status().as_u16(), via, "ok");
    resp
}

/// What Django's `ApiVersionMiddleware`/`SessionMiddleware` would derive from the
/// authentication that happened inside the handler.
fn apply_notes(resp: &mut Response, notes: &pl_core::RequestNotes) {
    let n = notes.snapshot();
    if n.authenticated {
        resp.extensions_mut().insert(pl_core::Authenticated);
    }
    for c in n.set_cookies {
        resp.headers_mut().append(header::SET_COOKIE, c);
    }
}

/// `PAPERLESS_ENABLE_COMPRESSION` live (value from the UI, docs/SETTINGS.md WP4b); invalid
/// or missing values → start value.
fn compression_enabled(s: &crate::config::DjangoSettings) -> bool {
    pl_core::config::var("PAPERLESS_ENABLE_COMPRESSION")
        .and_then(|v| pl_core::config::str_to_bool(&v))
        .unwrap_or(s.enable_compression)
}

#[cfg(test)]
mod health_tests {
    use super::health_is_local;
    use axum::http::{HeaderMap, HeaderValue};

    /// Review 2 N1: loopback peer with a forwarding header (same-host proxy) is not local.
    #[test]
    fn health_instance_only_when_set() {
        let b = super::health_body(3, None);
        assert!(b.get("instance").is_none());
        assert_eq!(b["service"], "paperrust");
        assert!(super::health_body(3, Some(" ")).get("instance").is_none());
        assert_eq!(super::health_body(3, Some("desk-1"))["instance"], "desk-1");
    }

    #[test]
    fn health_not_local_behind_same_host_proxy() {
        let lo: std::net::SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let ext: std::net::SocketAddr = "192.0.2.1:5000".parse().unwrap();
        let mut h = HeaderMap::new();
        assert!(health_is_local(Some(lo), &h));
        assert!(!health_is_local(Some(ext), &h));
        assert!(!health_is_local(None, &h));
        h.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));
        assert!(!health_is_local(Some(lo), &h));
        let mut h = HeaderMap::new();
        h.insert("forwarded", HeaderValue::from_static("for=203.0.113.9"));
        assert!(!health_is_local(Some(lo), &h));
    }
}
