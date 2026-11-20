//! Light UI under `/rs/app/` (docs/FRONTEND-LIGHT.md §6, ROADMAP-NEXT P4/P5).
//!
//! Fixed mount like API v2 (not in `routes.toml`), before the Django fallback;
//! `PAPERRUST_LIGHT_UI=off|false|0|no` switches it off. Source: `PAPERRUST_LIGHT_UI_DIR`, otherwise
//! `<PAPERLESS_STATICDIR>/../light` (package: `light\` next to `static\`, Docker:
//! `/usr/src/paperless/light`) – the content of `frontend-light/dist` (Vite build with
//! precompressed `*.br`/`*.gz`).
//!
//! * Files under `assets/` (hash in the name): public, `Cache-Control: public, max-age=31536000,
//!   immutable`; other files `no-cache`. Strong ETag from the content, `If-None-Match` → 304,
//!   `Accept-Encoding` → `br`/`gzip` variant (`Vary: Accept-Encoding`), `Range` on the
//!   uncompressed file (206/416).
//! * Every other path = SPA route → `index.html` (`no-cache`), but only when logged in; otherwise
//!   `302 /accounts/login/?next=/rs/app/…` (existing login page).
//! * CSP `default-src 'self'; img-src 'self' blob: data:; worker-src 'self' blob:` (pdf.js worker),
//!   without `unsafe-inline`; `X-Content-Type-Options: nosniff`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, Request};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_core::AppState;

/// Path prefix of the light UI.
pub const PREFIX: &str = "/rs/app";

/// CSP of the light UI (docs/FRONTEND-LIGHT.md §6).
pub const CSP: &str = "default-src 'self'; img-src 'self' blob: data:; worker-src 'self' blob:";

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";

/// One variant of a file (uncompressed, br, gzip). It is only read on first access and
/// then kept (restart measurement L2, 2026-11-19: previously startup read all ~13 MB including
/// precompressed variants and held them in memory, also languages and pdf.js parts that nobody ever loads).
#[derive(Debug)]
struct Variant {
    encoding: Option<&'static str>,
    /// `None` = already in memory (entry points with injected head tags).
    path: Option<PathBuf>,
    cell: OnceLock<Option<bytes::Bytes>>,
}

impl Variant {
    fn loaded(encoding: Option<&'static str>, b: Vec<u8>) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(Some(bytes::Bytes::from(b)));
        Variant { encoding, path: None, cell }
    }
    fn lazy(encoding: Option<&'static str>, path: PathBuf) -> Self {
        Variant { encoding, path: Some(path), cell: OnceLock::new() }
    }
    /// Content (from disk on first access); `None` if the file has gone missing in the meantime.
    fn bytes(&self) -> Option<&bytes::Bytes> {
        self.cell
            .get_or_init(|| self.path.as_ref().and_then(|p| std::fs::read(p).ok()).map(bytes::Bytes::from))
            .as_ref()
    }
}

#[derive(Debug)]
struct Asset {
    content_type: &'static str,
    etag: String,
    immutable: bool,
    /// first = uncompressed, then br, gzip
    variants: Vec<Variant>,
}

/// The files of the UI (list at startup, contents on demand).
#[derive(Debug, Default)]
pub struct LightUi {
    files: HashMap<String, Asset>,
    pub root: PathBuf,
}

/// FNV-1a 64 (ETag; no cryptography needed).
fn fnv64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for x in b {
        h ^= *x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

impl LightUi {
    /// Read in the directory (if it is missing or `index.html` is → `None`).
    pub fn scan(root: &Path) -> Option<LightUi> {
        let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
        if !root.join("index.html").is_file() {
            return None;
        }
        let mut paths = Vec::new();
        walk(&root, &mut paths);
        let have: std::collections::HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let mut files = HashMap::new();
        for p in &paths {
            let s = p.to_string_lossy();
            if s.ends_with(".br") || s.ends_with(".gz") {
                continue;
            }
            let Ok(rel) = p.strip_prefix(&root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let entry = rel == "index.html" || rel == MODERN_ENTRY;
            let (etag, plain) = if entry {
                // Read entry points immediately (small; `inject_head` changes them), ETag from the content
                let Ok(b) = std::fs::read(p) else { continue };
                (format!("\"{:016x}-{:x}\"", fnv64(&b), b.len()), Variant::loaded(None, b))
            } else {
                // others: metadata only; ETag from modification time and size (whitenoise style)
                let Ok(md) = std::fs::metadata(p) else { continue };
                let mtime = md
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                (format!("\"{mtime:x}-{:x}\"", md.len()), Variant::lazy(None, p.clone()))
            };
            let mut variants = vec![plain];
            for (enc, ext) in [("br", "br"), ("gzip", "gz")] {
                let vp = PathBuf::from(format!("{s}.{ext}"));
                if have.contains(vp.as_path()) {
                    variants.push(Variant::lazy(Some(enc), vp));
                }
            }
            let content_type = crate::frontend::statics::media_type(p);
            let immutable = rel.starts_with("assets/");
            files.insert(rel, Asset { content_type, etag, immutable, variants });
        }
        Some(LightUi { files, root })
    }

    /// Additional tags before `</head>` of the SPA entry points (`index.html`, `m.html`), e.g. the
    /// share hints of desktop operation ([`crate::frontend::share_meta`]). The precompressed
    /// variants of these two small files are then dropped, ETag is recalculated.
    pub fn inject_head(&mut self, tags: &str) {
        if tags.is_empty() {
            return;
        }
        for name in ["index.html", MODERN_ENTRY] {
            let Some(a) = self.files.get_mut(name) else { continue };
            let Some(cur) = a.variants[0].bytes() else { continue };
            let html = String::from_utf8_lossy(cur).into_owned();
            let Some(i) = html.find("</head>") else { continue };
            let mut out = html;
            out.insert_str(i, &format!("{tags}
"));
            let plain = out.into_bytes();
            a.etag = format!("\"{:016x}-{:x}\"", fnv64(&plain), plain.len());
            a.variants = vec![Variant::loaded(None, plain)];
        }
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Directory per configuration; `None` = switched off.
pub fn configured_dir(get: impl Fn(&str) -> Option<String>, static_root: &Path) -> Option<PathBuf> {
    let off = matches!(
        get("PAPERRUST_LIGHT_UI").map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "false" | "0" | "no")
    );
    if off {
        return None;
    }
    match get("PAPERRUST_LIGHT_UI_DIR").filter(|d| !d.trim().is_empty()) {
        Some(d) => Some(PathBuf::from(d.trim())),
        None => Some(static_root.parent().map(|p| p.join("light")).unwrap_or_else(|| PathBuf::from("light"))),
    }
}

fn hv(h: &HeaderMap, n: header::HeaderName) -> Option<&str> {
    h.get(n).and_then(|v| v.to_str().ok())
}

fn accepts(ae: &str, enc: &str) -> bool {
    ae.split(',').any(|p| {
        let mut it = p.split(';');
        let name = it.next().unwrap_or("").trim();
        let q0 = it.any(|x| x.trim().replace(' ', "") == "q=0");
        name.eq_ignore_ascii_case(enc) && !q0
    })
}

fn base_headers(r: &mut Response, a: &Asset) {
    let h = r.headers_mut();
    let ct = if a.content_type.starts_with("text/") { format!("{}; charset=utf-8", a.content_type) } else { a.content_type.to_string() };
    if let Ok(v) = HeaderValue::from_str(&ct) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(if a.immutable { IMMUTABLE } else { NO_CACHE }));
    if let Ok(v) = HeaderValue::from_str(&a.etag) {
        h.insert(header::ETAG, v);
    }
    if a.variants.len() > 1 {
        h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
}

/// `Range: bytes=a-b|a-|-n` → Some(Ok) / Some(Err)=416 / None=whole.
fn range(h: Option<&str>, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = h?.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        return if n == 0 || len == 0 { Some(Err(())) } else { Some(Ok((len.saturating_sub(n), len - 1))) };
    }
    let s: u64 = a.parse().ok()?;
    if s >= len {
        return Some(Err(()));
    }
    let e = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok().filter(|e| *e >= s)?.min(len - 1) };
    Some(Ok((s, e)))
}

/// Serve a file.
fn serve(a: &Asset, method: &Method, req: &HeaderMap) -> Response {
    let mut r = Response::new(Body::empty());
    base_headers(&mut r, a);
    if hv(req, header::IF_NONE_MATCH).is_some_and(|v| v.split(',').any(|e| e.trim() == a.etag || e.trim() == "*")) {
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        r.headers_mut().remove(header::CONTENT_TYPE);
        return r;
    }
    let head = method == Method::HEAD;
    let Some(plain) = a.variants[0].bytes() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Range only on the uncompressed file (like whitenoise)
    if let Some(rg) = range(hv(req, header::RANGE), plain.len() as u64)
        && hv(req, header::IF_RANGE).is_none_or(|v| v.trim() == a.etag)
    {
        let len = plain.len() as u64;
        return match rg {
            Ok((s, e)) => {
                *r.status_mut() = StatusCode::PARTIAL_CONTENT;
                let h = r.headers_mut();
                h.insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes {s}-{e}/{len}")).unwrap_or(HeaderValue::from_static("")));
                h.insert(header::CONTENT_LENGTH, HeaderValue::from(e - s + 1));
                if !head {
                    *r.body_mut() = Body::from(plain.slice(s as usize..=e as usize));
                }
                r
            }
            Err(()) => {
                *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                r.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes */{len}")).unwrap_or(HeaderValue::from_static("")));
                r
            }
        };
    }
    let ae = hv(req, header::ACCEPT_ENCODING).unwrap_or("");
    // matching precompressed variant (if its file has gone missing meanwhile: uncompressed)
    let (enc, body) = a
        .variants
        .iter()
        .skip(1)
        .filter(|v| v.encoding.is_some_and(|e| accepts(ae, e)))
        .find_map(|v| v.bytes().map(|b| (v.encoding, b)))
        .unwrap_or((None, plain));
    if let Some(enc) = enc {
        r.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
    }
    r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    if !head {
        // Bytes: without copying (previously the content was copied per response)
        *r.body_mut() = Body::from(body.clone());
    }
    // do not apply pl-server compression again
    r.extensions_mut().insert(crate::compress::Streaming);
    r
}

fn login_redirect(path_and_query: &str) -> Response {
    // like `encodeURIComponent` (Angular/light UI)
    let next: String = path_and_query
        .bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (c as char).to_string(),
            _ => format!("%{c:02X}"),
        })
        .collect();
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::FOUND;
    if let Ok(v) = HeaderValue::from_str(&format!("/accounts/login/?next={next}")) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

async fn handle(ui: Arc<LightUi>, state: AppState, req: Request) -> Response {
    let method = req.method().clone();
    if method != Method::GET && method != Method::HEAD {
        let mut r = StatusCode::METHOD_NOT_ALLOWED.into_response();
        r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return r;
    }
    let path = req.uri().path().to_string();
    if path == PREFIX {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::MOVED_PERMANENTLY;
        r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/rs/app/"));
        return r;
    }
    let rel = path.strip_prefix(PREFIX).unwrap_or("").trim_start_matches('/');
    // File (without `..`, without index.html directly – it goes through login like every SPA route)
    if !rel.is_empty() && rel != "index.html" && rel != MODERN_ENTRY && !rel.split('/').any(|s| s == ".." || s.is_empty())
        && let Some(a) = ui.files.get(rel)
    {
        return serve(a, &method, req.headers());
    }
    if rel.starts_with("assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    // SPA route → index.html, only when logged in
    let pq = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| path.clone());
    let (mut parts, _) = req.into_parts();
    let logged_in = matches!(pl_authz::MaybeUser::from_request_parts(&mut parts, &state).await, Ok(u) if u.user().is_some());
    if !logged_in {
        return login_redirect(&pq);
    }
    match ui.files.get(spa_entry(rel, &ui)) {
        Some(a) => serve(a, &method, &parts.headers),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Marker in the `AppState`: the light UI is mounted. Then it is the default UI: the
/// former Angular index page (`ui.index`, `/` and every SPA route) redirects to `/rs/app/…`.
#[derive(Debug, Clone, Copy)]
pub struct Mounted;

/// Target of the redirect from an old Angular address: same path under `/rs/app/`, query stays.
/// The light UI knows the Angular paths including their redirects (`router.svelte.ts`).
pub fn legacy_location(raw_path: &str, raw_query: Option<&str>) -> String {
    let rest = raw_path.trim_start_matches('/');
    match raw_query {
        Some(q) if !q.is_empty() => format!("{PREFIX}/{rest}?{q}"),
        _ => format!("{PREFIX}/{rest}"),
    }
}

/// `302` to [`legacy_location`].
pub fn legacy_redirect(raw_path: &str, raw_query: Option<&str>) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::FOUND;
    if let Ok(v) = HeaderValue::from_str(&legacy_location(raw_path, raw_query)) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Page under `/rs/app/…` if no light UI is mounted (test log 2026-11-16 D-100):
/// `/` always redirects here, the old Angular page is no longer served. Without a path
/// (no internals); the directory is in the server log.
pub const MISSING_PAGE: &str = "<!doctype html>
<html lang=\"de\">
<head><meta charset=\"utf-8\"><title>PaperRust</title></head>
<body>
<h1>PaperRust: Weboberfläche nicht installiert</h1>
<p>Der Server läuft, aber die Oberfläche (Ordner <code>light</code> neben <code>static</code>, Einstellung <code>PAPERRUST_LIGHT_UI_DIR</code>) wurde nicht gefunden oder ist mit <code>PAPERRUST_LIGHT_UI=off</code> abgeschaltet. Details stehen im Server-Log. Die API unter <code>/api/</code> ist erreichbar.</p>
<p lang=\"en\">The web UI is not installed (folder <code>light</code> next to <code>static</code>, setting <code>PAPERRUST_LIGHT_UI_DIR</code>) or switched off. See the server log.</p>
</body>
</html>
";

/// Response to `/rs/app/…` without light UI: `503` with [`MISSING_PAGE`].
pub fn missing_response() -> Response {
    let mut r = Response::new(Body::from(MISSING_PAGE));
    *r.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Router for `/rs/app` without light UI (instead of a bare 404 after the redirect from `/`).
pub fn missing_router() -> Router<AppState> {
    let h = || axum::routing::any(|| async { missing_response() });
    Router::new().route(PREFIX, h()).route(&format!("{PREFIX}/"), h()).route(&format!("{PREFIX}/{{*rest}}"), h())
}

/// Entry point of the second UI (docs/DESIGN2.md): own Vite page `m.html`.
pub const MODERN_ENTRY: &str = "m.html";

/// SPA entry per route: `/rs/app/m` and `/rs/app/m/…` → `m.html` (PaperRust design), otherwise `index.html`.
/// If `m.html` is missing (older build), it stays with `index.html`.
fn spa_entry(rel: &str, ui: &LightUi) -> &'static str {
    if (rel == "m" || rel.starts_with("m/")) && ui.files.contains_key(MODERN_ENTRY) { MODERN_ENTRY } else { "index.html" }
}

/// Router for `/rs/app` and below (mounted in `build_app` before the Django fallback).
pub fn router(ui: Arc<LightUi>) -> Router<AppState> {
    let route = |ui: Arc<LightUi>| {
        axum::routing::any(move |axum::extract::State(s): axum::extract::State<AppState>, req: Request| handle(ui.clone(), s, req))
    };
    Router::new()
        .route(PREFIX, route(ui.clone()))
        .route(&format!("{PREFIX}/"), route(ui.clone()))
        .route(&format!("{PREFIX}/{{*rest}}"), route(ui))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("assets")).unwrap();
        std::fs::write(d.path().join("index.html"), "<!doctype html><title>PaperRust</title>").unwrap();
        std::fs::write(d.path().join("index.html.br"), b"BR-INDEX").unwrap();
        std::fs::write(d.path().join("assets/index-abc.js"), "console.log(1)").unwrap();
        std::fs::write(d.path().join("assets/index-abc.js.gz"), b"GZ").unwrap();
        d
    }

    #[test]
    fn scan_and_serve() {
        let d = dist();
        let ui = LightUi::scan(d.path()).unwrap();
        assert_eq!(ui.len(), 2);
        let a = &ui.files["assets/index-abc.js"];
        assert!(a.immutable);
        let mut h = HeaderMap::new();
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));
        let r = serve(a, &Method::GET, &h);
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(r.headers()[header::CACHE_CONTROL], IMMUTABLE);
        assert_eq!(r.headers()[header::CONTENT_SECURITY_POLICY], CSP);
        let etag = r.headers()[header::ETAG].to_str().unwrap().to_string();
        let mut h2 = HeaderMap::new();
        h2.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&etag).unwrap());
        assert_eq!(serve(a, &Method::GET, &h2).status(), 304);
        let mut h3 = HeaderMap::new();
        h3.insert(header::RANGE, HeaderValue::from_static("bytes=0-6"));
        let r = serve(a, &Method::GET, &h3);
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes 0-6/14");
        let idx = &ui.files["index.html"];
        assert!(!idx.immutable);
        let mut h4 = HeaderMap::new();
        h4.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("br;q=0, gzip"));
        assert!(serve(idx, &Method::GET, &h4).headers().get(header::CONTENT_ENCODING).is_none(), "br refused, no gz");
        assert!(LightUi::scan(&d.path().join("missing")).is_none());
    }

    /// Restart L2: startup only reads the entry points; other files only on access, afterwards from
    /// memory; if a never-read file is missing later, 404 instead of a panic.
    #[test]
    fn files_are_read_lazily() {
        let d = dist();
        let ui = LightUi::scan(d.path()).unwrap();
        assert!(ui.files["index.html"].variants[0].cell.get().is_some(), "Einstieg sofort gelesen");
        let a = &ui.files["assets/index-abc.js"];
        assert!(a.variants.iter().all(|v| v.cell.get().is_none()), "nichts vorab gelesen");
        assert_eq!(serve(a, &Method::GET, &HeaderMap::new()).status(), 200);
        assert!(a.variants[0].cell.get().is_some());
        assert!(a.variants[1].cell.get().is_none(), "gzip nicht angefragt, nicht gelesen");
        // File gone after the first access: keep serving from memory
        std::fs::remove_file(d.path().join("assets/index-abc.js")).unwrap();
        assert_eq!(serve(a, &Method::GET, &HeaderMap::new()).status(), 200);
        // never-read file gone: 404
        std::fs::write(d.path().join("assets/late-x.js"), "1").unwrap();
        let ui = LightUi::scan(d.path()).unwrap();
        std::fs::remove_file(d.path().join("assets/late-x.js")).unwrap();
        assert_eq!(serve(&ui.files["assets/late-x.js"], &Method::GET, &HeaderMap::new()).status(), 404);
    }

    #[test]
    fn config_switch_and_default_dir() {
        let none = |_: &str| None::<String>;
        assert_eq!(configured_dir(none, Path::new("/srv/paperless/static")).unwrap(), PathBuf::from("/srv/paperless/light"));
        let off = |k: &str| (k == "PAPERRUST_LIGHT_UI").then(|| "off".to_string());
        assert!(configured_dir(off, Path::new("/x/static")).is_none());
        let dir = |k: &str| (k == "PAPERRUST_LIGHT_UI_DIR").then(|| "/opt/light".to_string());
        assert_eq!(configured_dir(dir, Path::new("/x/static")).unwrap(), PathBuf::from("/opt/light"));
    }

    #[test]
    fn modern_entry_for_m_routes() {
        let d = dist();
        let ui = LightUi::scan(d.path()).unwrap();
        assert_eq!(spa_entry("m/documents", &ui), "index.html", "ohne m.html bleibt index.html");
        std::fs::write(d.path().join("m.html"), "<!doctype html><title>PaperRust</title>").unwrap();
        let ui = LightUi::scan(d.path()).unwrap();
        assert_eq!(spa_entry("m", &ui), MODERN_ENTRY);
        assert_eq!(spa_entry("m/documents/12", &ui), MODERN_ENTRY);
        assert_eq!(spa_entry("mail", &ui), "index.html");
        assert_eq!(spa_entry("documents", &ui), "index.html");
    }

    #[test]
    fn head_tags_injected_into_entries() {
        let d = dist();
        // without </head>: unchanged (including precompressed variant)
        let mut ui = LightUi::scan(d.path()).unwrap();
        let before = ui.files["index.html"].etag.clone();
        ui.inject_head("<meta name=\"a\">");
        assert_eq!(ui.files["index.html"].etag, before);
        assert_eq!(ui.files["index.html"].variants.len(), 2);
        // with </head>: inserted, new ETag, no stale precompressed variants
        std::fs::write(d.path().join("index.html"), "<html><head><title>x</title></head></html>").unwrap();
        let mut ui = LightUi::scan(d.path()).unwrap();
        let before = ui.files["index.html"].etag.clone();
        ui.inject_head("<meta name=\"paperrust_share_mode\" content=\"local\">");
        let a = &ui.files["index.html"];
        assert_ne!(a.etag, before);
        assert_eq!(a.variants.len(), 1);
        let body = String::from_utf8(a.variants[0].bytes().unwrap().to_vec()).unwrap();
        assert!(body.contains("content=\"local\">
</head>"));
        // empty: nothing to do
        ui.inject_head("");
    }

    #[test]
    fn legacy_paths_go_to_light_ui() {
        assert_eq!(legacy_location("/", None), "/rs/app/");
        assert_eq!(legacy_location("/documents/12/details", None), "/rs/app/documents/12/details");
        assert_eq!(legacy_location("/documents", Some("tags__id__all=3&sort=created")), "/rs/app/documents?tags__id__all=3&sort=created");
        assert_eq!(legacy_location("/tags", Some("")), "/rs/app/tags");
        let r = legacy_redirect("/view/4", None);
        assert_eq!(r.status(), 302);
        assert_eq!(r.headers()[header::LOCATION], "/rs/app/view/4");
    }

    #[test]
    fn login_redirect_keeps_next() {
        let r = login_redirect("/rs/app/documents?tags=1");
        assert_eq!(r.status(), 302);
        assert_eq!(r.headers()[header::LOCATION], "/accounts/login/?next=%2Frs%2Fapp%2Fdocuments%3Ftags%3D1");
    }
}
