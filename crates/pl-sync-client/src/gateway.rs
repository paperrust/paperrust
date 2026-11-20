//! Gateway for the shell's `prapi://` scheme (docs/SYNC.md §5, docs/REMOTE-APP.md §5).
//!
//! The light UI speaks **one** API client against `prapi://localhost/…` (Windows:
//! `http://prapi.localhost/…`). The gateway decides per request:
//!
//! * `/rs/api/v2/…` and `/api/…` (v1, transitional): online → forwarded to the server (token
//!   from the core, TLS pinning in the core); offline or `?source=local` → from the replica (read-only;
//!   writing offline → `503 offline`).
//! * `/local/…`: core functions (status, log in/out, selection, limits, search, sync now,
//!   `/local/ws` = address of the `/ws/status/` relay, [`crate::wsrelay`]).
//! * other page paths (`/documents/…`, `/settings` …) = links of the light UI into the classic
//!   UI: notice page (online: link to the server, opens in the system browser; offline: "only
//!   available online"), [`crate::app::classic_page`]; the shell already intercepts the navigation online
//!   beforehand ([`Gateway::classic_url`]).
//!
//! Every response carries `X-PaperRust-Source: remote|local`. The token never leaves the core: the
//! WebView can neither set `Authorization` nor cookies (they are discarded) nor have device tokens
//! issued (`/auth/devices`, `/api/token/` and the like are blocked). The origin check
//! (`Origin`/`Referer` = app origin) is done by the shell before the call ([`origin_allowed`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use pl_domain::query::DocQuery;
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::engine::SyncClient;
use crate::error::Error;
use crate::query;
use crate::selection::Limits;
use crate::store::{META_KINDS, SelectionItem};

/// Request from the WebView (built by the shell from the Tauri request).
#[derive(Debug, Clone, Default)]
pub struct GwRequest {
    pub method: String,
    /// Path without query (`/rs/api/v2/documents`).
    pub path: String,
    /// Query without `?`.
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Response to the WebView.
#[derive(Debug, Clone)]
pub struct GwResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl GwResponse {
    pub fn json(status: u16, v: &Value, source: &str) -> GwResponse {
        GwResponse {
            status,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("cache-control".into(), "no-store".into()),
                ("x-paperrust-source".into(), source.into()),
            ],
            body: serde_json::to_vec(v).unwrap_or_default(),
        }
    }

    pub fn error(status: u16, code: &str, message: &str, source: &str) -> GwResponse {
        GwResponse::json(status, &json!({"error": {"code": code, "message": message}}), source)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Origin check: `Origin` (or alternatively `Referer`) must be one of the app origins.
pub fn origin_allowed(headers: &[(String, String)], allowed: &[&str]) -> bool {
    let get = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.trim().to_string());
    let origin = match get("origin").filter(|o| !o.is_empty() && o != "null") {
        Some(o) => o,
        None => match get("referer").and_then(|r| url::Url::parse(&r).ok()) {
            // not `origin()`: for own schemes (tauri://) it would be opaque ("null")
            Some(u) => match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{}://{h}:{p}", u.scheme()),
                (Some(h), None) => format!("{}://{h}", u.scheme()),
                _ => return false,
            },
            None => return false,
        },
    };
    let origin = origin.trim_end_matches('/');
    allowed.iter().any(|a| a.trim_end_matches('/').eq_ignore_ascii_case(origin))
}

/// Headers that are forwarded to the server (everything else – above all `Authorization`,
/// `Cookie`, `X-CSRFToken` – is discarded).
const FORWARD_HEADERS: [&str; 9] =
    ["accept", "accept-language", "content-type", "if-match", "if-none-match", "range", "if-range", "idempotency-key", "x-requested-with"];

/// Response headers from the server that the WebView sees.
const RETURN_HEADERS: [&str; 10] = [
    "content-type",
    "content-length",
    "content-range",
    "content-disposition",
    "etag",
    "accept-ranges",
    "cache-control",
    "last-modified",
    "x-api-version",
    "x-version",
];

/// Server paths blocked from the WebView (token/device issuance, login). The decoded,
/// lower-case path is checked; dot segments and `//` are always blocked.
fn blocked(path: &str) -> bool {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy().to_ascii_lowercase();
    if decoded.contains("/../") || decoded.contains("/./") || decoded.ends_with("/..") || decoded.contains("//") || decoded.contains('\\') {
        return true;
    }
    let path = decoded.as_str();
    const BLOCK: [&str; 7] = [
        "/rs/api/v2/auth/",
        "/api/token/",
        "/api/profile/generate_auth_token/",
        "/api/profile/totp/",
        "/api/oauth/",
        "/api/auth/",
        "/api/profile/social_account_providers/",
    ];
    BLOCK.iter().any(|b| path.starts_with(b))
}

fn parse_query(q: Option<&str>) -> HashMap<String, String> {
    url::form_urlencoded::parse(q.unwrap_or("").as_bytes()).into_owned().collect()
}

fn query_without_source(q: Option<&str>) -> Option<String> {
    let pairs: Vec<(String, String)> =
        url::form_urlencoded::parse(q.unwrap_or("").as_bytes()).into_owned().filter(|(k, _)| k != "source").collect();
    if pairs.is_empty() {
        return None;
    }
    Some(url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish())
}

fn err_response(e: &Error, source: &str) -> GwResponse {
    match e {
        Error::Api { status, code, message } => GwResponse::error(*status, code, message, source),
        Error::NotFound => GwResponse::error(404, "not_found", "Not found.", source),
        Error::Invalid(m) => GwResponse::error(400, "invalid", m, source),
        Error::LimitExceeded(m) => GwResponse::error(422, "limit_exceeded", m, source),
        Error::Unauthenticated => GwResponse::error(401, "not_authenticated", "Login required.", source),
        Error::Offline(_) => GwResponse::error(503, "offline", "Server not reachable.", source),
        Error::PinMismatch => GwResponse::error(495, "pin_mismatch", "Server certificate does not match the pinned fingerprint.", source),
        Error::Tls(_) => GwResponse::error(495, "tls", "TLS error.", source),
        other => {
            tracing::warn!("gateway error: {other}");
            GwResponse::error(500, "internal", "Internal error.", source)
        }
    }
}

/// The gateway of a profile.
pub struct Gateway {
    client: SyncClient,
    force_offline: AtomicBool,
    /// Light UI build (`frontend-light/dist`) for `/rs/app/` (mode 2b, page via prapi).
    app_dir: Option<std::path::PathBuf>,
    /// `/ws/status/` relay (mode 2b), set by the shell after startup.
    ws_relay: std::sync::OnceLock<crate::wsrelay::RelayInfo>,
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
struct LogoutBody {
    #[serde(default)]
    wipe: bool,
}

#[derive(Deserialize)]
struct SelectionBody {
    items: Vec<SelectionItem>,
}

#[derive(Deserialize)]
struct ModeBody {
    force_offline: bool,
}

#[derive(Deserialize)]
struct BatchBody {
    kind: String,
    uuids: Vec<String>,
    #[serde(default)]
    include: Vec<String>,
}

impl Gateway {
    pub fn new(client: SyncClient) -> Gateway {
        Gateway { client, force_offline: AtomicBool::new(false), app_dir: None, ws_relay: std::sync::OnceLock::new() }
    }

    /// Serve the light UI under `/rs/app/` from this directory (SPA fallback to index.html).
    pub fn with_app_dir(mut self, dir: Option<std::path::PathBuf>) -> Gateway {
        self.app_dir = dir.filter(|d| d.join("index.html").is_file());
        self
    }

    /// Announce the address of the `/ws/status/` relay (once per gateway).
    pub fn set_ws_relay(&self, info: crate::wsrelay::RelayInfo) {
        let _ = self.ws_relay.set(info);
    }

    pub fn ws_relay(&self) -> Option<&crate::wsrelay::RelayInfo> {
        self.ws_relay.get()
    }

    /// Target on the server for a link into the classic UI – only if online (otherwise the
    /// gateway shows "only available online"). The shell opens it in the system browser.
    pub fn classic_url(&self, path: &str, query: Option<&str>) -> Option<String> {
        if !crate::app::is_classic_path(path) || !self.online() {
            return None;
        }
        let mut u = self.client.base_url().join(path.trim_start_matches('/')).ok()?;
        if u.origin() != self.client.base_url().origin() {
            return None;
        }
        u.set_query(query.filter(|q| !q.is_empty()));
        Some(u.to_string())
    }

    pub fn client(&self) -> &SyncClient {
        &self.client
    }

    /// Online path possible (logged in, not known as offline, not forced offline)?
    pub fn online(&self) -> bool {
        let st = self.client.status();
        !self.force_offline.load(Ordering::SeqCst) && st.logged_in && st.online
    }

    pub async fn handle(&self, req: GwRequest) -> GwResponse {
        let path = req.path.clone();
        if path.starts_with("/local/") {
            return self.local_fn(&req).await;
        }
        if path == crate::app::PREFIX || path.starts_with("/rs/app/") {
            let mut r = crate::app::serve_app(self.app_dir.as_deref(), &req).await;
            // Light UI may reach the `/ws/status/` relay (otherwise `connect-src` = 'self')
            if let Some(ws) = self.ws_relay.get() {
                for (k, v) in r.headers.iter_mut() {
                    if k.eq_ignore_ascii_case("content-security-policy") {
                        *v = format!("{v}; connect-src 'self' {}", ws.csp_source());
                    }
                }
            }
            return r;
        }
        if path.starts_with("/accounts/") {
            return self.accounts(&req).await;
        }
        let is_v2 = path == "/rs/api/v2" || path.starts_with("/rs/api/v2/");
        let is_v1 = path.starts_with("/api/");
        if !is_v2 && !is_v1 && matches!(req.method.to_ascii_uppercase().as_str(), "GET" | "HEAD") && crate::app::is_classic_path(&path) {
            return crate::app::classic_page(self.classic_url(&path, req.query.as_deref()).as_deref());
        }
        if !is_v2 && !is_v1 {
            return GwResponse::error(404, "not_found", "Not found.", "local");
        }
        if blocked(&path) {
            return GwResponse::error(403, "permission_denied", "Not available from the app.", "local");
        }
        // not logged in → 401, so the light UI switches to the (own) login page
        if !self.client.status().logged_in {
            return GwResponse::error(401, "not_authenticated", "Login required.", "local");
        }
        let params = parse_query(req.query.as_deref());
        let want_local = params.get("source").is_some_and(|s| s == "local");
        // paperless-ngx knows no API v2 → v2 always from the replica
        let ngx_v2 = is_v2 && self.client.is_ngx();
        if !want_local && !ngx_v2 && self.online() {
            match self.forward(&req).await {
                Ok(r) => {
                    if is_v1 && req.method.eq_ignore_ascii_case("GET") && r.status == 200 && cacheable(&path) {
                        let key = cache_key(&path, req.query.as_deref());
                        let _ = self.client.replica().cache_put(&key, r.status, r.header("content-type"), &r.body).await;
                    }
                    return r;
                }
                Err(Error::Offline(_)) => {
                    // switch offline, core keeps checking in the background
                    self.client.mark_offline();
                }
                Err(e) => return err_response(&e, "remote"),
            }
        }
        if is_v1 {
            return self.local_v1(&req).await;
        }
        self.local_v2(&req, &params).await
    }

    async fn forward(&self, req: &GwRequest) -> crate::Result<GwResponse> {
        let headers: Vec<(String, String)> = req
            .headers
            .iter()
            .filter(|(k, _)| FORWARD_HEADERS.iter().any(|f| f.eq_ignore_ascii_case(k)))
            .cloned()
            .collect();
        let mut pq = req.path.trim_start_matches('/').to_string();
        if let Some(q) = query_without_source(req.query.as_deref()) {
            pq.push('?');
            pq.push_str(&q);
        }
        let r = self.client.raw(&req.method, &pq, &headers, req.body.clone()).await?;
        if r.status == 401 {
            self.client.mark_unauthenticated();
        }
        let mut out_headers: Vec<(String, String)> =
            r.headers.into_iter().filter(|(k, _)| RETURN_HEADERS.iter().any(|f| f.eq_ignore_ascii_case(k))).collect();
        out_headers.push(("x-paperrust-source".into(), "remote".into()));
        out_headers.push(("x-content-type-options".into(), "nosniff".into()));
        // Server HTML (e.g. Django pages) never runs with the prapi origin: sandbox without scripts
        if out_headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("html")) {
            out_headers.push(("content-security-policy".into(), "sandbox; default-src 'none'".into()));
        }
        Ok(GwResponse { status: r.status, headers: out_headers, body: r.body.to_vec() })
    }

    // ------------------------------------------------------------------ Anmeldung (Seite)

    async fn accounts(&self, req: &GwRequest) -> GwResponse {
        let p = parse_query(req.query.as_deref());
        match req.path.as_str() {
            "/accounts/login/" => crate::app::login_page(),
            "/accounts/login.js" => crate::app::login_js(),
            "/accounts/login.css" => crate::app::login_css(),
            "/accounts/logout/" => {
                // Log out from the UI: revoke device, token gone; replica stays
                let _ = self.client.logout(false).await;
                let next = p.get("next").map(String::as_str).unwrap_or("/rs/app/");
                crate::app::redirect(&format!("/accounts/login/?next={}", crate::app::encode(crate::app::safe_next(next))))
            }
            _ => GwResponse::error(404, "not_found", "Not found.", "local"),
        }
    }

    // ------------------------------------------------------------------ lokal (v1-Formen)

    async fn local_v1(&self, req: &GwRequest) -> GwResponse {
        match self.local_v1_inner(req).await {
            Ok(r) => r,
            Err(e) => {
                let mut r = err_response(&e, "local");
                // v1-Fehlerform (DRF)
                if let Error::NotFound = e {
                    r.body = br#"{"detail":"Not found."}"#.to_vec();
                }
                r
            }
        }
    }

    async fn local_v1_inner(&self, req: &GwRequest) -> crate::Result<GwResponse> {
        use crate::v1local as v1;
        let r = self.client.replica();
        let ok = |v: Value| Ok(GwResponse::json(200, &v, "local"));
        let method = req.method.to_ascii_uppercase();
        if method != "GET" && method != "HEAD" {
            return Ok(GwResponse::error(503, "offline", "Changes are not possible offline.", "local"));
        }
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(req.query.as_deref().unwrap_or("").as_bytes())
            .into_owned()
            .filter(|(k, _)| k != "source")
            .collect();
        let get = |k: &str| pairs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        let seg: Vec<&str> = req.path.trim_start_matches("/api/").trim_end_matches('/').split('/').filter(|s| !s.is_empty()).collect();
        // Documents first from the replica (current, selection-dependent)
        match seg.as_slice() {
            ["documents"] => return ok(v1::list_documents(r, &req.path, &pairs).await?),
            ["documents", id] if id.parse::<i64>().is_ok() => {
                return ok(v1::get_document(r, id.parse().unwrap_or(0), get("fields")).await?);
            }
            ["documents", id, sub] if id.parse::<i64>().is_ok() => {
                let id: i64 = id.parse().unwrap_or(0);
                return match *sub {
                    "notes" => ok(v1::notes(r, id).await?),
                    "metadata" => ok(v1::metadata(r, id).await?),
                    "suggestions" => ok(json!({"correspondents": [], "tags": [], "document_types": [], "storage_paths": [], "dates": []})),
                    "thumb" | "preview" | "download" => {
                        let key = v1::doc_key(r, id).await?;
                        let original = get("original").is_some_and(|v| v == "true" || v == "1");
                        let variant = match (*sub, original) {
                            ("thumb", _) => "thumbnail",
                            (_, true) => "original",
                            _ => "preview",
                        };
                        let mut resp = self.local_file(req, &key, variant).await?;
                        if *sub == "download" && resp.status == 200 {
                            let d = r.document_json(&key).await?.unwrap_or(Value::Null);
                            let name = d["original_filename"].as_str().unwrap_or("document").to_string();
                            resp.headers.push(("content-disposition".into(), crate::app::attachment(&name)));
                        }
                        Ok(resp)
                    }
                    _ => Ok(GwResponse::error(503, "offline", "Not available offline.", "local")),
                };
            }
            _ => {}
        }
        // otherwise: last response seen online (exact path + query)
        if let Some((status, ct, body)) = r.cache_get(&cache_key(&req.path, req.query.as_deref())).await? {
            return Ok(GwResponse {
                status,
                headers: vec![
                    ("content-type".into(), ct.unwrap_or_else(|| "application/json".into())),
                    ("cache-control".into(), "no-store".into()),
                    ("x-paperrust-source".into(), "local".into()),
                ],
                body,
            });
        }
        match seg.as_slice() {
            ["ui_settings"] => ok(v1::ui_settings(r).await?),
            [plural] if v1::kind_of_v1(plural).is_some() => ok(v1::list_objects(r, v1::kind_of_v1(plural).unwrap_or_default()).await?),
            [plural, id] if v1::kind_of_v1(plural).is_some() && id.parse::<i64>().is_ok() => {
                ok(v1::get_object(r, v1::kind_of_v1(plural).unwrap_or_default(), id.parse().unwrap_or(0)).await?)
            }
            _ => Ok(GwResponse::error(503, "offline", "Not available offline.", "local")),
        }
    }

    // ------------------------------------------------------------------ lokal (v2-Formen)

    async fn local_v2(&self, req: &GwRequest, params: &HashMap<String, String>) -> GwResponse {
        match self.local_v2_inner(req, params).await {
            Ok(r) => r,
            Err(e) => err_response(&e, "local"),
        }
    }

    async fn local_v2_inner(&self, req: &GwRequest, params: &HashMap<String, String>) -> crate::Result<GwResponse> {
        let r = self.client.replica();
        let rel = req.path.trim_start_matches("/rs/api/v2").trim_matches('/');
        let seg: Vec<&str> = if rel.is_empty() { vec![] } else { rel.split('/').collect() };
        let method = req.method.to_ascii_uppercase();
        let ok = |v: Value| Ok(GwResponse::json(200, &v, "local"));
        let read_only = method == "GET" || method == "HEAD";
        let include: Vec<String> =
            params.get("include").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
        match (method.as_str(), seg.as_slice()) {
            ("GET", [] | ["server"]) => {
                let row = r.remote().await?.unwrap_or_default();
                ok(json!({"product": "paperrust", "api": 2, "features": ["local"], "server_uuid": row.server_uuid,
                          "source": "local", "last_sync": row.last_contact}))
            }
            ("GET", ["me"]) => match r.remote_text("me_json").await? {
                Some(j) => ok(serde_json::from_str(&j)?),
                None => Err(Error::NotFound),
            },
            ("GET", ["ui_settings"]) => match r.remote_text("ui_settings").await? {
                Some(j) => ok(serde_json::from_str(&j)?),
                None => ok(json!({"settings": {}})),
            },
            ("GET", ["documents"]) => {
                let q = DocQuery {
                    filter: None,
                    sort: params.get("sort").cloned(),
                    limit: match params.get("limit") {
                        Some(l) => Some(l.parse().map_err(|_| Error::Invalid("limit".into()))?),
                        None => None,
                    },
                    cursor: params.get("cursor").cloned(),
                    include,
                    count: params.get("count").is_some_and(|c| c == "true" || c == "1"),
                };
                ok(query::query_documents(r, &q).await?)
            }
            ("POST", ["documents", "query"]) => {
                let q: DocQuery = serde_json::from_slice(&req.body)
                    .map_err(|e| Error::Api { status: 400, code: "invalid".into(), message: e.to_string() })?;
                ok(query::query_documents(r, &q).await?)
            }
            ("GET", ["documents", uuid]) => {
                let v = r
                    .document_full(uuid, include.iter().any(|i| i == "content"), include.iter().any(|i| i == "notes"))
                    .await?
                    .ok_or(Error::NotFound)?;
                ok(v)
            }
            ("GET", ["documents", uuid, "notes"]) => {
                let v = r.document_full(uuid, false, true).await?.ok_or(Error::NotFound)?;
                ok(v["notes"].clone())
            }
            ("GET" | "HEAD", ["documents", uuid, "files", variant]) => self.local_file(req, uuid, variant).await,
            ("POST", ["objects", "batch"]) => {
                let b: BatchBody = serde_json::from_slice(&req.body)
                    .map_err(|e| Error::Api { status: 400, code: "invalid".into(), message: e.to_string() })?;
                let mut items = Vec::new();
                let mut missing = Vec::new();
                let content = b.include.iter().any(|i| i == "content");
                let notes = b.include.iter().any(|i| i == "notes");
                for u in b.uuids.iter().take(500) {
                    let v = if b.kind == "document" { r.document_full(u, content, notes).await? } else { r.object(&b.kind, u).await? };
                    match v {
                        Some(v) => items.push(v),
                        None => missing.push(Value::String(u.clone())),
                    }
                }
                ok(json!({"items": items, "missing": missing}))
            }
            ("GET", [plural]) if kind_of(plural).is_some() => {
                let kind = kind_of(plural).unwrap_or_default();
                let limit = match params.get("limit") {
                    Some(l) => Some(l.parse::<i64>().map_err(|_| Error::Invalid("limit".into()))?),
                    None => None,
                };
                ok(query::list_objects(
                    r,
                    kind,
                    params.get("cursor").map(String::as_str),
                    limit,
                    params.get("sort").map(String::as_str),
                    params.get("name_contains").map(String::as_str),
                    params.get("count").is_some_and(|c| c == "true" || c == "1"),
                )
                .await?)
            }
            ("GET", [plural, uuid]) if kind_of(plural).is_some() => {
                let kind = kind_of(plural).unwrap_or_default();
                ok(r.object(kind, uuid).await?.ok_or(Error::NotFound)?)
            }
            _ if !read_only => Ok(GwResponse::error(503, "offline", "Changes are not possible offline.", "local")),
            _ => Ok(GwResponse::error(503, "offline", "Not available offline.", "local")),
        }
    }

    /// File from the cache (ETag/304, Range/206/416), `preview` = archive if local, otherwise original.
    async fn local_file(&self, req: &GwRequest, uuid: &str, variant: &str) -> crate::Result<GwResponse> {
        let r = self.client.replica();
        let files = self.client.files();
        let doc = r.document_json(uuid).await?.ok_or(Error::NotFound)?;
        let candidates: &[&str] = match variant {
            "original" => &["original"],
            "archive" => &["archive"],
            "thumbnail" => &["thumb"],
            "preview" => &["archive", "original"],
            _ => return Err(Error::NotFound),
        };
        let mut found = None;
        for c in candidates {
            if let Some(f) = files.local(r, uuid, c).await? {
                found = Some(f);
                break;
            }
        }
        let Some(f) = found else {
            return Ok(GwResponse::error(404, "not_cached", "File is not available offline.", "local"));
        };
        let data = tokio::fs::read(&f.path).await?;
        let etag = match (&f.etag, &f.checksum) {
            (Some(e), _) => e.clone(),
            (None, Some(c)) => format!("\"{c}\""),
            _ => format!("W/\"{}\"", data.len()),
        };
        let content_type = match f.variant.as_str() {
            "thumb" => "image/webp".to_string(),
            "archive" => "application/pdf".to_string(),
            _ => doc["mime_type"].as_str().unwrap_or("application/octet-stream").to_string(),
        };
        let hv = |n: &str| req.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str());
        let mut headers = vec![
            ("content-type".to_string(), content_type),
            ("etag".to_string(), etag.clone()),
            ("accept-ranges".to_string(), "bytes".to_string()),
            ("cache-control".to_string(), "private, max-age=0".to_string()),
            ("x-paperrust-source".to_string(), "local".to_string()),
        ];
        if hv("if-none-match").is_some_and(|v| v.split(',').any(|e| e.trim() == etag || e.trim() == "*")) {
            return Ok(GwResponse { status: 304, headers, body: vec![] });
        }
        let len = data.len() as u64;
        let range_ok = hv("if-range").is_none_or(|v| v.trim() == etag);
        let range = if range_ok { parse_range(hv("range"), len) } else { None };
        let head = req.method.eq_ignore_ascii_case("HEAD");
        match range {
            Some(Err(())) => {
                headers.push(("content-range".into(), format!("bytes */{len}")));
                Ok(GwResponse { status: 416, headers, body: vec![] })
            }
            Some(Ok((a, b))) => {
                headers.push(("content-range".into(), format!("bytes {a}-{b}/{len}")));
                headers.push(("content-length".into(), (b - a + 1).to_string()));
                let body = if head { vec![] } else { data[a as usize..=b as usize].to_vec() };
                Ok(GwResponse { status: 206, headers, body })
            }
            None => {
                headers.push(("content-length".into(), len.to_string()));
                Ok(GwResponse { status: 200, headers, body: if head { vec![] } else { data } })
            }
        }
    }

    // ------------------------------------------------------------------ /local/…

    async fn local_fn(&self, req: &GwRequest) -> GwResponse {
        match self.local_fn_inner(req).await {
            Ok(r) => r,
            Err(e) => err_response(&e, "local"),
        }
    }

    async fn local_fn_inner(&self, req: &GwRequest) -> crate::Result<GwResponse> {
        let c = &self.client;
        let method = req.method.to_ascii_uppercase();
        let body = |b: &[u8]| -> crate::Result<Value> {
            if b.is_empty() { Ok(json!({})) } else { serde_json::from_slice(b).map_err(|e| Error::Invalid(e.to_string())) }
        };
        let ok = |v: Value| Ok(GwResponse::json(200, &v, "local"));
        match (method.as_str(), req.path.as_str()) {
            ("GET", "/local/status") => {
                let st = c.status();
                let row = c.replica().remote().await?.unwrap_or_default();
                ok(json!({
                    "status": st,
                    "force_offline": self.force_offline.load(Ordering::SeqCst),
                    "source": if self.online() { "remote" } else { "local" },
                    "profile": {"id": c.config().profile_id, "url": row.url, "user_name": row.user_name,
                                "server_uuid": row.server_uuid, "last_full_sync": row.last_full_sync},
                    "limits": c.limits().await?,
                    "selection": c.replica().selection().await?,
                    "cache_bytes": c.replica().cache_bytes().await?,
                    "documents": c.replica().count_documents(true).await?,
                    "objects": {
                        "tag": c.replica().count_objects("tag").await?,
                        "correspondent": c.replica().count_objects("correspondent").await?,
                        "document_type": c.replica().count_objects("document_type").await?,
                        "saved_view": c.replica().count_objects("saved_view").await?,
                    },
                }))
            }
            ("POST", "/local/login") => {
                let b: LoginBody = serde_json::from_slice(&req.body).map_err(|_| Error::Invalid("username/password".into()))?;
                let LoginBody { username, password, code } = b;
                let password = Zeroizing::new(password);
                c.login(&username, password, code.as_deref()).await?;
                ok(json!({"ok": true}))
            }
            ("POST", "/local/logout") => {
                let b: LogoutBody = serde_json::from_value(body(&req.body)?).map_err(|e| Error::Invalid(e.to_string()))?;
                let revoked = c.logout(b.wipe).await?;
                ok(json!({"ok": true, "revoked": revoked}))
            }
            ("GET", "/local/selection") => ok(json!({"items": c.replica().selection().await?, "limits": c.limits().await?})),
            ("POST", "/local/selection/preview") => {
                let b: SelectionBody = serde_json::from_slice(&req.body).map_err(|e| Error::Invalid(e.to_string()))?;
                ok(serde_json::to_value(c.preview_selection(&b.items).await?)?)
            }
            ("PUT", "/local/selection") => {
                let b: SelectionBody = serde_json::from_slice(&req.body).map_err(|e| Error::Invalid(e.to_string()))?;
                ok(serde_json::to_value(c.apply_selection(&b.items).await?)?)
            }
            ("PUT", "/local/limits") => {
                let l: Limits = serde_json::from_slice(&req.body).map_err(|e| Error::Invalid(e.to_string()))?;
                c.set_limits(l).await?;
                ok(serde_json::to_value(l)?)
            }
            ("GET", "/local/ws") => {
                // `url` only if the server supports the socket with device token (PaperRust); the
                // light UI then connects and tries again later when offline
                let url = self.ws_relay.get().filter(|_| !c.is_ngx()).map(|w| w.url());
                ok(json!({"url": url, "online": self.online()}))
            }
            ("POST", "/local/sync") => {
                c.wake();
                ok(json!({"ok": true}))
            }
            ("POST", "/local/mode") => {
                let b: ModeBody = serde_json::from_slice(&req.body).map_err(|e| Error::Invalid(e.to_string()))?;
                self.force_offline.store(b.force_offline, Ordering::SeqCst);
                ok(json!({"force_offline": b.force_offline}))
            }
            ("GET", "/local/search") => {
                let p = parse_query(req.query.as_deref());
                let limit = p.get("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
                ok(query::search(c.replica(), p.get("q").map(String::as_str).unwrap_or(""), limit).await?)
            }
            _ => Ok(GwResponse::error(404, "not_found", "Not found.", "local")),
        }
    }
}

/// v1 responses that are cached online and delivered 1:1 offline (framework,
/// master data, statistics) – no documents (those come from the replica).
fn cacheable(path: &str) -> bool {
    const PREFIXES: [&str; 17] = [
        "/api/ui_settings/",
        "/api/profile/",
        "/api/statistics/",
        "/api/remote_version/",
        "/api/tasks/status_counts/",
        "/api/tags/",
        "/api/correspondents/",
        "/api/document_types/",
        "/api/storage_paths/",
        "/api/custom_fields/",
        "/api/saved_views/",
        "/api/users/",
        "/api/groups/",
        "/api/status/",
        "/api/config/",
        "/api/mail_accounts/",
        "/api/workflows/",
    ];
    PREFIXES.iter().any(|p| path.starts_with(p))
}

/// Key of the response cache: path + query without `source`.
fn cache_key(path: &str, query: Option<&str>) -> String {
    match query_without_source(query) {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
}

fn kind_of(plural: &str) -> Option<&'static str> {
    let k = pl_domain::Kind::from_plural(plural)?;
    META_KINDS.iter().copied().find(|m| *m == k.as_str())
}

/// `Range: bytes=a-b | a- | -n` → Some(Ok((a, b))) / Some(Err) = 416 / None = whole file.
pub fn parse_range(h: Option<&str>, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = h?.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 || len == 0 {
            return Some(Err(()));
        }
        return Some(Ok((len.saturating_sub(n), len - 1)));
    }
    let start: u64 = a.parse().ok()?;
    if start >= len {
        return Some(Err(()));
    }
    let end = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok().filter(|e| *e >= start)?.min(len - 1) };
    Some(Ok((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn origins() {
        let allowed = ["tauri://localhost", "http://tauri.localhost"];
        assert!(origin_allowed(&h(&[("Origin", "http://tauri.localhost")]), &allowed));
        assert!(origin_allowed(&h(&[("referer", "tauri://localhost/index.html")]), &allowed));
        assert!(!origin_allowed(&h(&[("Origin", "https://evil.example")]), &allowed));
        assert!(!origin_allowed(&h(&[("Origin", "null")]), &allowed));
        assert!(!origin_allowed(&h(&[]), &allowed));
        assert!(!origin_allowed(&h(&[("Origin", "http://tauri.localhost.evil.example")]), &allowed));
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 10), None);
        assert_eq!(parse_range(Some("bytes=0-3"), 10), Some(Ok((0, 3))));
        assert_eq!(parse_range(Some("bytes=5-"), 10), Some(Ok((5, 9))));
        assert_eq!(parse_range(Some("bytes=-4"), 10), Some(Ok((6, 9))));
        assert_eq!(parse_range(Some("bytes=8-100"), 10), Some(Ok((8, 9))));
        assert_eq!(parse_range(Some("bytes=10-"), 10), Some(Err(())));
        assert_eq!(parse_range(Some("bytes=0-1,3-4"), 10), None);
        assert_eq!(parse_range(Some("items=0-1"), 10), None);
    }

    #[test]
    fn blocklist_and_query() {
        assert!(blocked("/rs/api/v2/auth/devices"));
        assert!(blocked("/api/token/"));
        assert!(!blocked("/api/documents/"));
        assert!(blocked("/rs/api/v2/%61uth/devices"));
        assert!(blocked("/rs/api/v2/documents/../auth/devices"));
        assert!(blocked("/API/TOKEN/"));
        assert_eq!(query_without_source(Some("source=local&limit=5")), Some("limit=5".into()));
        assert_eq!(query_without_source(Some("source=local")), None);
        assert_eq!(kind_of("tags"), Some("tag"));
        assert_eq!(kind_of("documents"), None);
    }
}
