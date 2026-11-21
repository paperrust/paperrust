//! `prapi://` scheme (mode 2b/3, docs/REMOTE-APP.md §5, docs/SYNC.md §5).
//!
//! The packed light UI (and the own settings page) talk to the client core via
//! `prapi://localhost/…` (Windows/WebView2: `http://prapi.localhost/…`). All the logic
//! (online → server with token from the keychain, offline → SQLite replica, `/local/…`)
//! lives in `pl_sync_client::Gateway`; here only:
//!
//! * origin check: only own pages (`tauri://localhost`, `http(s)://tauri.localhost`);
//!   everything else (server pages in 2a, foreign origins) → 403, **before** anything happens;
//! * CORS only for these origins (incl. preflight);
//! * translating Tauri request ↔ `GwRequest`/`GwResponse`; the token never leaves the core.
//!
//! Without an active sync profile the scheme answers with `503 not_configured`.

use std::sync::{Arc, RwLock};

use pl_sync_client::gateway::{Gateway, GwRequest, GwResponse, origin_allowed};
use tauri::http::{Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

/// Name of the scheme.
pub const SCHEME: &str = "prapi";

/// Origins of the own, packed pages (Tauri 2: Windows/Android `http(s)://tauri.localhost`,
/// otherwise `tauri://localhost`).
pub const APP_ORIGINS: [&str; 6] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
    // light UI itself, loaded from the scheme (mode 2b)
    "prapi://localhost",
    "http://prapi.localhost",
    "https://prapi.localhost",
];

/// Pages that may be loaded as a document (navigation without origin/referer): only the
/// static app, the login page and the notice page for links into the classic UI
/// (reload; contains only the link to the server or "nur online verfügbar") – no data.
fn is_page(req: &GwRequest) -> bool {
    req.method.eq_ignore_ascii_case("GET")
        && (req.path.starts_with("/rs/app") || req.path.starts_with("/accounts/login") || pl_sync_client::app::is_classic_path(&req.path))
}

/// Allowed request headers (preflight).
const ALLOW_HEADERS: &str =
    "accept, accept-language, content-type, if-match, if-none-match, range, if-range, idempotency-key, x-requested-with";
const ALLOW_METHODS: &str = "GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS";
const EXPOSE_HEADERS: &str = "etag, content-range, content-disposition, x-paperrust-source, x-version, x-api-version";

/// Active gateway (set by [`crate::sync`]), managed as Tauri state.
#[derive(Default)]
pub struct PrapiState {
    gateway: RwLock<Option<Arc<Gateway>>>,
}

impl PrapiState {
    pub fn set(&self, gw: Option<Arc<Gateway>>) {
        if let Ok(mut g) = self.gateway.write() {
            *g = gw;
        }
    }

    pub fn get(&self) -> Option<Arc<Gateway>> {
        self.gateway.read().ok().and_then(|g| g.clone())
    }
}

/// Tauri request → gateway request (path without host, query separate, headers as text).
pub fn to_gw(req: &Request<Vec<u8>>) -> GwRequest {
    let uri = req.uri();
    GwRequest {
        method: req.method().as_str().to_string(),
        path: uri.path().to_string(),
        query: uri.query().map(str::to_string),
        headers: req.headers().iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string()))).collect(),
        body: req.body().clone(),
    }
}

fn origin_of(req: &GwRequest) -> Option<String> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
        .map(|(_, v)| v.clone())
        .filter(|o| APP_ORIGINS.iter().any(|a| a.eq_ignore_ascii_case(o.trim_end_matches('/'))))
}

fn with_cors(mut b: tauri::http::response::Builder, origin: Option<&str>) -> tauri::http::response::Builder {
    if let Some(o) = origin {
        b = b.header("access-control-allow-origin", o).header("access-control-expose-headers", EXPOSE_HEADERS).header("vary", "origin");
    }
    b
}

fn plain(status: u16, code: &str, origin: Option<&str>) -> Response<Vec<u8>> {
    let body = format!("{{\"error\":{{\"code\":\"{code}\",\"message\":\"{code}\"}}}}").into_bytes();
    with_cors(Response::builder().status(status), origin)
        .header("content-type", "application/json")
        .header("x-paperrust-source", "local")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// Gateway response → Tauri response (+ CORS for the app origin).
pub fn from_gw(r: GwResponse, origin: Option<&str>) -> Response<Vec<u8>> {
    let mut b = with_cors(Response::builder().status(StatusCode::from_u16(r.status).unwrap_or(StatusCode::BAD_GATEWAY)), origin);
    for (k, v) in &r.headers {
        // the runtime sets the length itself; do not pass on hop-by-hop headers
        if k.eq_ignore_ascii_case("transfer-encoding") || k.eq_ignore_ascii_case("connection") {
            continue;
        }
        b = b.header(k.as_str(), v.as_str());
    }
    b.body(r.body).unwrap_or_else(|_| Response::new(Vec::new()))
}

fn no_origin(req: &GwRequest) -> bool {
    !req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("origin") || k.eq_ignore_ascii_case("referer"))
}

/// Pre-check without core: `Err(Response)` = answer immediately.
pub fn precheck(req: &GwRequest) -> Result<Option<String>, Box<Response<Vec<u8>>>> {
    if !origin_allowed(&req.headers, &APP_ORIGINS) && !(is_page(req) && no_origin(req)) {
        return Err(Box::new(plain(403, "origin_not_allowed", None)));
    }
    let origin = origin_of(req);
    if req.method.eq_ignore_ascii_case("OPTIONS") {
        let r = with_cors(Response::builder().status(204), origin.as_deref())
            .header("access-control-allow-methods", ALLOW_METHODS)
            .header("access-control-allow-headers", ALLOW_HEADERS)
            .header("access-control-max-age", "600")
            .body(Vec::new())
            .unwrap_or_else(|_| Response::new(Vec::new()));
        return Err(Box::new(r));
    }
    Ok(origin)
}

/// Handler for `register_asynchronous_uri_scheme_protocol`.
pub fn handler<R: Runtime>(ctx: UriSchemeContext<'_, R>, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let req = to_gw(&request);
    let origin = match precheck(&req) {
        Ok(o) => o,
        Err(resp) => return responder.respond(*resp),
    };
    let gw = ctx.app_handle().try_state::<PrapiState>().and_then(|s| s.get());
    let Some(gw) = gw else {
        return responder.respond(plain(503, "not_configured", origin.as_deref()));
    };
    tauri::async_runtime::spawn(async move {
        let r = gw.handle(req).await;
        responder.respond(from_gw(r, origin.as_deref()));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, uri: &str, headers: &[(&str, &str)]) -> Request<Vec<u8>> {
        let mut b = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(b"{}".to_vec()).unwrap()
    }

    #[test]
    fn converts_request() {
        let r =
            to_gw(&req("POST", "http://prapi.localhost/rs/api/v2/documents/query?source=local", &[("Origin", "http://tauri.localhost")]));
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/rs/api/v2/documents/query");
        assert_eq!(r.query.as_deref(), Some("source=local"));
        assert_eq!(r.body, b"{}");
        let r = to_gw(&req("GET", "prapi://localhost/local/status", &[]));
        assert_eq!(r.path, "/local/status");
    }

    #[test]
    fn foreign_origins_are_rejected_before_the_core() {
        // server page (mode 2a) or foreign page → 403, without core
        let r = to_gw(&req("POST", "http://prapi.localhost/local/logout", &[("Origin", "https://docs.example.ch")]));
        let e = precheck(&r).unwrap_err();
        assert_eq!(e.status(), 403);
        assert!(e.headers().get("access-control-allow-origin").is_none());
        // without origin/referer → 403
        let r = to_gw(&req("GET", "http://prapi.localhost/local/status", &[]));
        assert_eq!(precheck(&r).unwrap_err().status(), 403);
        // own page
        let r = to_gw(&req("GET", "http://prapi.localhost/local/status", &[("Origin", "http://tauri.localhost")]));
        assert_eq!(precheck(&r).unwrap().as_deref(), Some("http://tauri.localhost"));
        // image without origin, but with referer of the app
        let r = to_gw(&req(
            "GET",
            "http://prapi.localhost/rs/api/v2/documents/x/files/thumbnail",
            &[("Referer", "http://tauri.localhost/index.html")],
        ));
        assert_eq!(precheck(&r).unwrap(), None);
    }

    #[test]
    fn app_pages_load_as_documents_but_data_needs_an_origin() {
        // navigation of the window to the app (no origin/referer)
        let r = to_gw(&req("GET", "http://prapi.localhost/rs/app/documents", &[]));
        assert!(precheck(&r).is_ok());
        let r = to_gw(&req("GET", "http://prapi.localhost/accounts/login/?next=%2Frs%2Fapp%2F", &[]));
        assert!(precheck(&r).is_ok());
        // data of the app: same origin (prapi) allowed, without origin not
        let r = to_gw(&req("GET", "http://prapi.localhost/api/documents/", &[("Referer", "http://prapi.localhost/rs/app/")]));
        assert!(precheck(&r).is_ok());
        let r = to_gw(&req("GET", "http://prapi.localhost/api/documents/1/download/", &[]));
        assert_eq!(precheck(&r).unwrap_err().status(), 403);
        let r = to_gw(&req("POST", "http://prapi.localhost/rs/app/x", &[]));
        assert_eq!(precheck(&r).unwrap_err().status(), 403);
        // links into the classic UI: notice page also on reload, data paths not
        let r = to_gw(&req("GET", "http://prapi.localhost/documents/12/notes", &[]));
        assert!(precheck(&r).is_ok());
        let r = to_gw(&req("POST", "http://prapi.localhost/documents/12/notes", &[]));
        assert_eq!(precheck(&r).unwrap_err().status(), 403);
        let r = to_gw(&req("GET", "http://prapi.localhost/local/ws", &[]));
        assert_eq!(precheck(&r).unwrap_err().status(), 403);
    }

    #[test]
    fn preflight_only_for_app_origin() {
        let r = to_gw(&req("OPTIONS", "http://prapi.localhost/rs/api/v2/documents/query", &[("Origin", "tauri://localhost")]));
        let p = precheck(&r).unwrap_err();
        assert_eq!(p.status(), 204);
        assert_eq!(p.headers()["access-control-allow-origin"], "tauri://localhost");
        assert!(p.headers()["access-control-allow-headers"].to_str().unwrap().contains("if-match"));
        let r = to_gw(&req("OPTIONS", "http://prapi.localhost/x", &[("Origin", "https://evil.example")]));
        assert_eq!(precheck(&r).unwrap_err().status(), 403);
    }

    #[test]
    fn converts_response_with_cors() {
        let g = GwResponse::json(200, &serde_json::json!({"ok": true}), "local");
        let r = from_gw(g, Some("http://tauri.localhost"));
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["x-paperrust-source"], "local");
        assert_eq!(r.headers()["access-control-allow-origin"], "http://tauri.localhost");
        assert!(r.headers()["access-control-expose-headers"].to_str().unwrap().contains("x-paperrust-source"));
        let r = from_gw(GwResponse::error(495, "pin_mismatch", "x", "remote"), None);
        assert_eq!(r.status().as_u16(), 495);
    }
}
