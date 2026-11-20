//! W22: frontend without Django (`ui.index` catch-all, `static.files`, RedirectViews) and
//! no Django backend (404 for paths without a route). No database: only anonymous requests
//! (no session cookie → the session store is never queried).

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, StatusCode};
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use pl_core::AppState;
use pl_core::config::Env;
use pl_server::config::ServerConfig;
use pl_server::routing::{self, Backend};
use tokio::net::TcpListener;

const T: Duration = Duration::from_secs(20);

fn state(static_dir: &std::path::Path) -> AppState {
    let dir = static_dir.display().to_string();
    let config = pl_core::Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "test-only-secret-key-0123456789"),
        ("PAPERLESS_STATICDIR", dir.as_str()),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
    ]))
    .unwrap();
    let db = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://localhost/unused").unwrap();
    AppState::with_pool(config, db).unwrap()
}

async fn start(static_dir: &std::path::Path, ids: &[&str]) -> SocketAddr {
    let mut m: HashMap<String, String> = HashMap::new();
    m.insert("PAPERLESS_ALLOWED_HOSTS".into(), "127.0.0.1".into());
    let cfg = ServerConfig::from_lookup(|k| m.get(k).cloned()).unwrap();
    // only the listed routes on (all others "off"), like the former opt-in table
    let toml: BTreeMap<String, Backend> = pl_server::all_routes()
        .iter()
        .map(|r| (r.id.to_owned(), if ids.contains(&r.id) { Backend::Rust } else { Backend::Off }))
        .collect();
    let plan = routing::plan(pl_server::all_routes(), &toml).unwrap();
    let app = pl_server::build_app(&cfg, plan, state(static_dir)).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(pl_server::serve::serve(l, app, std::future::pending()));
    addr
}

async fn send(addr: SocketAddr, method: Method, path: &str, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Bytes) {
    let mut b = hyper::Request::builder().method(method).uri(format!("http://{addr}{path}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let resp = tokio::time::timeout(T, client.request(b.body(Body::empty()).unwrap())).await.unwrap().unwrap_or_else(|e| panic!("{path}: {e:?}"));
    let (parts, body) = resp.into_parts();
    let bytes = tokio::time::timeout(T, body.collect()).await.unwrap().unwrap().to_bytes();
    (parts.status, parts.headers, bytes)
}

fn static_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let fe = dir.path().join("frontend").join("en-US");
    std::fs::create_dir_all(&fe).unwrap();
    std::fs::write(fe.join("main.js"), "x".repeat(2000)).unwrap();
    std::fs::write(fe.join("main.js.br"), "b".repeat(50)).unwrap();
    let side = dir.path().join("drf_spectacular_sidecar").join("swagger-ui-dist");
    std::fs::create_dir_all(&side).unwrap();
    std::fs::write(side.join("swagger-ui.css"), "body{}").unwrap();
    dir
}

const ALL_UI: &[&str] = &["ui.index", "static.files", "ui.api_root", "ui.assets", "ui.fetch_doc", "ui.fetch_thumb", "ui.fetch_preview", "static.swagger_ui_dist"];

#[tokio::test]
async fn frontend_without_django() {
    let dir = static_dir();
    let addr = start(dir.path(), ALL_UI).await;
    let h = [("host", "127.0.0.1")];

    // catch-all index: the Angular UI is gone – always to the same path below /rs/app/ (D-100)
    let (s, hd, _) = send(addr, Method::GET, "/documents?sort=created&x=%C3%A4", &h).await;
    assert_eq!(s, StatusCode::FOUND);
    assert_eq!(hd["location"], "/rs/app/documents?sort=created&x=%C3%A4");
    assert_eq!(hd["x-frame-options"], "SAMEORIGIN");
    let (s, hd, _) = send(addr, Method::GET, "/", &h).await;
    assert_eq!((s, hd["location"].to_str().unwrap()), (StatusCode::FOUND, "/rs/app/"));
    // no light UI build here: /rs/app/ explains that instead of the old endless Angular page
    let (s, hd, b) = send(addr, Method::GET, "/rs/app/", &h).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(hd["content-type"], "text/html; charset=utf-8");
    let page = String::from_utf8_lossy(&b);
    assert!(page.contains("Weboberfläche nicht installiert") && !page.contains("pngx-root"));
    let (s, _, _) = send(addr, Method::GET, "/rs/app/documents/1", &h).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    // /api/<unknown>: Django falls through to the index; PaperRust gives API clients DRF's JSON 404
    // (parity run 2026-11-20 F5, Known difference)
    for p in ["/api/does_not_exist/", "/api/users/2", "/api"] {
        let (s, hd, b) = send(addr, Method::GET, p, &h).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{p}");
        assert_eq!(hd["content-type"], "application/json", "{p}");
        assert_eq!(&b[..], br#"{"detail":"Not found."}"#, "{p}");
    }

    // WhiteNoise: precompressed alternative, only security headers
    let (s, hd, b) = send(addr, Method::GET, "/static/frontend/en-US/main.js", &[("host", "127.0.0.1"), ("accept-encoding", "gzip, br")]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(hd["content-encoding"], "br");
    assert_eq!(b.len(), 50);
    assert_eq!(hd["content-type"], "text/javascript; charset=\"utf-8\"");
    assert_eq!(hd["access-control-allow-origin"], "*");
    assert_eq!(hd["x-content-type-options"], "nosniff");
    assert!(hd.get("x-frame-options").is_none());
    assert!(hd.get("content-language").is_none());
    let (s, hd, _) = send(addr, Method::POST, "/static/frontend/en-US/main.js", &h).await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(hd["allow"], "GET, HEAD");
    // sidecar files come from STATIC_ROOT like any other static file (dot in the name)
    let (s, hd, b) = send(addr, Method::GET, "/static/drf_spectacular_sidecar/swagger-ui-dist/swagger-ui.css", &h).await;
    assert_eq!((s, &b[..]), (StatusCode::OK, &b"body{}"[..]));
    assert!(hd.get("etag").is_some());
    // unknown static file → index (anonymous: login)
    let (s, hd, _) = send(addr, Method::GET, "/static/frontend/en-US/nope.js", &h).await;
    assert_eq!((s, hd["location"].to_str().unwrap()), (StatusCode::FOUND, "/rs/app/static/frontend/en-US/nope.js"));

    // RedirectViews
    let (s, hd, _) = send(addr, Method::GET, "/api/", &h).await;
    assert_eq!((s, hd["location"].to_str().unwrap()), (StatusCode::FOUND, "schema/view/"));
    let (s, hd, _) = send(addr, Method::GET, "/assets/i18n/a%20b.json", &h).await;
    assert_eq!((s, hd["location"].to_str().unwrap()), (StatusCode::FOUND, "/static/frontend/en-US/assets/i18n/a%20b.json"));
    let (s, hd, _) = send(addr, Method::GET, "/fetch/doc/12", &h).await;
    assert_eq!((s, hd["location"].to_str().unwrap()), (StatusCode::FOUND, "/api/documents/12/download/"));
    let (s, hd, _) = send(addr, Method::GET, "/fetch/thumb/abc", &h).await;
    assert_eq!((s, hd["location"].to_str().unwrap()), (StatusCode::FOUND, "/rs/app/fetch/thumb/abc"));

    // not ported + no Django → 404 (HTML / DRF JSON)
    let (s, hd, b) = send(addr, Method::GET, "/admin/", &h).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(hd["content-type"], "text/html; charset=utf-8");
    assert!(String::from_utf8_lossy(&b).contains("<h1>Not Found</h1>"));
    let (s, hd, b) = send(addr, Method::GET, "/api/documents/merge_as_versions/", &h).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(hd["content-type"], "application/json");
    assert_eq!(&b[..], br#"{"detail":"Not found."}"#);
    let (s, _, _) = send(addr, Method::POST, "/dashboard", &h).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // decisions Django takes before any view
    let (s, hd, _) = send(addr, Method::OPTIONS, "/api/tags/", &[("host", "127.0.0.1"), ("origin", "http://localhost:8000"), ("access-control-request-method", "GET")]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(hd["access-control-allow-origin"], "http://localhost:8000");
    assert!(hd.get("x-frame-options").is_none());
    let (s, _, b) = send(addr, Method::GET, "/", &[("host", "evil.example")]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&b).contains("Bad Request (400)"));
}

#[tokio::test]
async fn without_ui_routes_everything_is_404() {
    let dir = static_dir();
    let addr = start(dir.path(), &["ui.api_root"]).await;
    let h = [("host", "127.0.0.1")];
    let (s, _, _) = send(addr, Method::GET, "/dashboard", &h).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, _) = send(addr, Method::GET, "/static/frontend/en-US/main.js", &h).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, _) = send(addr, Method::GET, "/api/", &h).await;
    assert_eq!(s, StatusCode::FOUND);
}
