//! Integration tests: routes.toml (switching routes off), mounting, other methods on mounted
//! paths, Django-like middleware on Rust responses, health endpoint. There is no Django backend
//! any more. Only 127.0.0.1, no external services, no database.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, put};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use pl_core::ApiRoute;
use pl_server::config::ServerConfig;
use pl_server::routing::{self, Backend, RoutesError};
use tokio::net::TcpListener;

const T: Duration = Duration::from_secs(10);

mod support;
use support::test_state;

// ---------------------------------------------------------------- helpers

async fn start(vars: &[(&str, &str)], routes: Vec<ApiRoute>, toml: &str) -> SocketAddr {
    let m: HashMap<String, String> = vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    let cfg = ServerConfig::from_lookup(|k| m.get(k).cloned()).unwrap();
    let parsed = routing::parse_routes_toml(toml).unwrap();
    let plan = routing::plan(routes, &parsed).unwrap();
    let app = pl_server::build_app(&cfg, plan, test_state()).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(pl_server::serve::serve(l, app, std::future::pending()));
    addr
}

fn client() -> Client<HttpConnector, Body> {
    Client::builder(TokioExecutor::new()).build(HttpConnector::new())
}

async fn send(addr: SocketAddr, method: Method, path: &str, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Bytes) {
    let mut b = hyper::Request::builder().method(method).uri(format!("http://{addr}{path}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let resp = tokio::time::timeout(T, client().request(b.body(Body::empty()).unwrap())).await.unwrap().unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = tokio::time::timeout(T, body.collect()).await.unwrap().unwrap().to_bytes();
    (parts.status, parts.headers, bytes)
}

#[tokio::test]
async fn health_is_internal() {
    let app = start(&[], vec![], "").await;
    let (status, _, body) = send(app, Method::GET, "/__rs/health", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["service"], "paperrust");
    assert_eq!(v["version"], pl_core::brand::PAPERRUST_VERSION);
    // anything else under the prefix: 404
    let (s, _, _) = send(app, Method::GET, "/__rs/other", &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- routes.toml / mounting

fn demo_routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute { id: "x.get", method: Method::GET, path: "/api/x/", upstream: "test", handler: get(|| async { "rust-get" }) },
        // "another crate" adds PUT on the same path
        ApiRoute { id: "x.put", method: Method::PUT, path: "/api/x/", upstream: "test", handler: put(|| async { "rust-put" }) },
        ApiRoute {
            id: "x.post",
            method: Method::POST,
            path: "/api/x/",
            upstream: "test",
            handler: axum::routing::post(|| async { "rust-post" }),
        },
        ApiRoute { id: "x.detail", method: Method::GET, path: "/api/x/{id}/", upstream: "test", handler: get(|| async { "rust-detail" }) },
        ApiRoute {
            id: "boom",
            method: Method::GET,
            path: "/api/boom/",
            upstream: "test",
            handler: get(|| async {
                let fail = std::hint::black_box(true);
                if fail {
                    panic!("handler bug");
                }
                "unreachable"
            }),
        },
        ApiRoute {
            id: "big",
            method: Method::GET,
            path: "/api/big/",
            upstream: "test",
            handler: get(|| async {
                ([("content-type", "application/json")], format!("[{}0]", "1234567890,".repeat(500))).into_response()
            }),
        },
    ]
}

const DEMO_TOML: &str = r#"
[routes]
"x.get" = "rust"
"x.put" = "rust"
"x.post" = "off"
"x.detail" = "rust"
"big" = "rust"
"boom" = "rust"
"#;

/// Other methods on a mounted path: DRF's 401/403 (authentication first) below `/api/`, never a
/// proxy; switched-off routes are not mounted (docs/STANDALONE-GAPS.md §5).
#[tokio::test]
async fn partial_methods_and_switched_off_routes() {
    let app = start(&[], demo_routes(), DEMO_TOML).await;

    let (s, _, b) = send(app, Method::GET, "/api/x/", &[]).await;
    assert_eq!((s, b.as_ref()), (StatusCode::OK, b"rust-get".as_ref()));
    let (_, _, b) = send(app, Method::PUT, "/api/x/", &[]).await;
    assert_eq!(b, "rust-put");
    // x.post is "off": like any method without a route
    for m in [Method::POST, Method::DELETE] {
        let (s, _, b) = send(app, m.clone(), "/api/x/?a=1", &[]).await;
        assert!(matches!(s, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN), "{m}: {s}");
        assert!(!String::from_utf8_lossy(&b).contains("rust-post"));
    }
    let (s, _, b) = send(app, Method::HEAD, "/api/x/", &[]).await;
    assert_eq!((s, b.len()), (StatusCode::OK, 0));
    // no trailing slash: no route → 404
    let (s, _, _) = send(app, Method::GET, "/api/x", &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // CORS preflight: answered like CorsMiddleware
    let (s, h, b) =
        send(app, Method::OPTIONS, "/api/x/", &[("origin", "http://localhost:8000"), ("access-control-request-method", "GET")]).await;
    assert_eq!((s, b.len()), (StatusCode::OK, 0));
    assert_eq!(h["access-control-allow-origin"], "http://localhost:8000");
    let (_, _, b) = send(app, Method::GET, "/api/x/5/", &[]).await;
    assert_eq!(b, "rust-detail");
}

#[tokio::test]
async fn rust_responses_get_django_middleware_headers() {
    let app = start(&[], demo_routes(), DEMO_TOML).await;
    let (_, h, _) = send(app, Method::GET, "/api/x/", &[("origin", "http://localhost:8000"), ("accept-language", "de-CH,de;q=0.9")]).await;
    assert_eq!(h["x-frame-options"], "SAMEORIGIN");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["referrer-policy"], "same-origin");
    assert_eq!(h["cross-origin-opener-policy"], "same-origin");
    assert_eq!(h["content-language"], "de-de");
    assert_eq!(h["vary"], "Accept-Language, origin, Cookie");
    assert_eq!(h["access-control-allow-origin"], "http://localhost:8000");
    assert_eq!(h["access-control-allow-credentials"], "true");
    assert_eq!(h["access-control-expose-headers"], "Content-Disposition");

    // token auth: Django still sends Vary: Cookie (checked against the reference); foreign origin
    let (_, h, _) = send(app, Method::GET, "/api/x/", &[("origin", "http://evil.example"), ("authorization", "Token abc")]).await;
    assert_eq!(h["vary"], "Accept-Language, origin, Cookie");
    assert!(h.get("access-control-allow-origin").is_none());

    // compression like django-compression-middleware (zstd > br > gzip)
    for (ae, enc) in [("gzip", "gzip"), ("gzip, br", "br"), ("gzip, br, zstd", "zstd")] {
        let (_, h, b) = send(app, Method::GET, "/api/big/", &[("accept-encoding", ae)]).await;
        assert_eq!(h["content-encoding"], enc);
        assert!(b.len() < 1000);
        assert_eq!(h["vary"], "Accept-Language, origin, Cookie, Accept-Encoding");
    }
    let (_, h, b) = send(app, Method::GET, "/api/big/", &[]).await;
    assert!(h.get("content-encoding").is_none());
    assert_eq!(b.len(), 5503);
}

#[tokio::test]
async fn panicking_handler_gives_django_500_page() {
    let app = start(&[], demo_routes(), DEMO_TOML).await;
    let (s, h, b) = send(app, Method::GET, "/api/boom/", &[]).await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(String::from_utf8_lossy(&b).contains("<h1>Server Error (500)</h1>"));
    assert!(!String::from_utf8_lossy(&b).contains("handler bug"));
    assert_eq!(h["x-frame-options"], "SAMEORIGIN");
    // the server keeps working
    let (_, _, b) = send(app, Method::GET, "/api/x/", &[]).await;
    assert_eq!(b, "rust-get");
}

#[tokio::test]
async fn disallowed_host_gets_django_400() {
    let app = start(&[("PAPERLESS_ALLOWED_HOSTS", "only.example")], demo_routes(), DEMO_TOML).await;
    let (s, _, b) = send(app, Method::GET, "/api/x/", &[("host", "evil.example")]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&b).contains("Bad Request (400)"), "{}", String::from_utf8_lossy(&b));
    let (_, _, b) = send(app, Method::GET, "/api/x/", &[("host", "only.example:8000")]).await;
    assert_eq!(b, "rust-get");
}

#[test]
fn routes_toml_validation() {
    let cfg = |s: &str| routing::parse_routes_toml(s);
    assert!(cfg("").unwrap().is_empty());
    assert!(cfg("[routes]\n").unwrap().is_empty());
    assert_eq!(cfg("[routes]\n\"a.b\" = \"rust\"\n").unwrap()["a.b"], Backend::Rust);
    assert!(matches!(cfg("[routes]\na = \"maybe\"\n"), Err(RoutesError::Parse(_))));
    assert!(matches!(cfg("[other]\na = \"rust\"\n"), Err(RoutesError::Parse(_))));
    assert!(matches!(cfg("a = \"rust\"\n"), Err(RoutesError::Parse(_))));

    assert_eq!(cfg("[routes]\n\"a.b\" = \"off\"\n").unwrap()["a.b"], Backend::Off);
    assert_eq!(cfg("[routes]\n\"a.b\" = \"on\"\n").unwrap()["a.b"], Backend::Rust);
    // obsolete value: still accepted (warning), the route is served by Rust
    assert_eq!(cfg("[routes]\n\"a.b\" = \"django\"\n").unwrap()["a.b"], Backend::Django);

    let mut unknown = BTreeMap::new();
    unknown.insert("nope.list".to_owned(), Backend::Off);
    match routing::plan(demo_routes(), &unknown) {
        Err(RoutesError::UnknownIds(ids)) => assert_eq!(ids, ["nope.list"]),
        _ => panic!("unknown id must fail"),
    }

    let mut dup = demo_routes();
    dup.push(ApiRoute {
        id: "x.get",
        method: Method::DELETE,
        path: "/api/y/",
        upstream: "test",
        handler: axum::routing::delete(|| async { "" }),
    });
    assert!(matches!(routing::plan(dup, &BTreeMap::new()), Err(RoutesError::DuplicateId(_))));

    let mut dup = demo_routes();
    dup.push(ApiRoute { id: "x.get2", method: Method::GET, path: "/api/x/{pk}/", upstream: "test", handler: get(|| async { "" }) });
    assert!(matches!(routing::plan(dup, &BTreeMap::new()), Err(RoutesError::DuplicateRoute { .. })));

    let mut conflict = demo_routes();
    conflict.push(ApiRoute {
        id: "x.delete",
        method: Method::DELETE,
        path: "/api/x/{pk}/",
        upstream: "test",
        handler: axum::routing::delete(|| async { "" }),
    });
    assert!(matches!(routing::plan(conflict, &BTreeMap::new()), Err(RoutesError::ParamNameConflict { .. })));

    let plan = routing::plan(demo_routes(), &routing::parse_routes_toml(DEMO_TOML).unwrap()).unwrap();
    assert_eq!(plan.total, 6);
    let ids: Vec<_> = plan.active_info().iter().map(|r| r.id).collect();
    assert_eq!(ids, ["x.get", "x.put", "x.detail", "boom", "big"]);
    // ids not listed are on; "django" counts as on
    let plan = routing::plan(demo_routes(), &routing::parse_routes_toml("[routes]\n\"x.post\" = \"django\"\n").unwrap()).unwrap();
    assert_eq!(plan.active.len(), 6);
    let plan = routing::plan(demo_routes(), &BTreeMap::new()).unwrap();
    assert_eq!(plan.active.len(), 6);
}

#[test]
fn shipped_routes_toml_matches_registered_routes() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../routes.toml");
    let cfg = routing::load_routes_file(std::path::Path::new(path), true).unwrap();
    routing::plan(pl_server::all_routes(), &cfg).unwrap();
}
