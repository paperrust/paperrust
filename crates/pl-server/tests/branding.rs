//! Branding (PAPERRUST_BRANDING): style injection only into HTML navigations, exactly once,
//! with/without compression; off = untouched; own assets under /__rs/brand/.
//! Only 127.0.0.1, test routes in-process.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use async_compression::tokio::bufread::{GzipDecoder, ZstdDecoder};
use axum::body::Body;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::routing::{get, post};
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use pl_core::ApiRoute;
use pl_server::config::ServerConfig;
use pl_server::routing;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

mod support;
use support::test_state;

const T: Duration = Duration::from_secs(10);

const INDEX: &str = concat!(
    "<!doctype html>\n<html lang=\"en\" data-bs-theme=\"auto\">\n<head>\n",
    "\t<meta charset=\"utf-8\">\n\t<title>Paperless-ngx</title>\n\t<base href=\"/\">\n",
    "\t<link rel=\"icon\" type=\"image/x-icon\" href=\"favicon.ico\">\n",
    "\t<link rel=\"stylesheet\" href=\"/static/frontend/en-US/styles.css\">\n",
    "</head>\n<body>\n\t<pngx-root></pngx-root>\n"
);

fn index_body() -> String {
    // > 500 bytes so the compression middleware applies
    format!("{INDEX}{}</body>\n</html>\n", "<!-- padding -->\n".repeat(80))
}

fn html() -> ([(&'static str, &'static str); 1], String) {
    ([("content-type", "text/html; charset=utf-8")], index_body())
}

/// HTML pages at the paths the tests use (GET), a POST form target and a non-HTML page.
fn routes() -> Vec<ApiRoute> {
    let page = |id: &'static str, path: &'static str| ApiRoute::new(id, Method::GET, path, "test", get(|| async { html() }));
    vec![
        page("rs.page", "/rs-page/"),
        page("rs.dashboard", "/dashboard"),
        page("rs.api_html", "/api/x/"),
        page("rs.share", "/share/{slug}"),
        ApiRoute::new("rs.login", Method::POST, "/accounts/x-login/", "test", post(|| async { html() })),
        ApiRoute::new("rs.plain", Method::GET, "/plain", "test", get(|| async { "x".repeat(1000) })),
    ]
}

async fn start(vars: &[(&str, &str)]) -> SocketAddr {
    let m: HashMap<String, String> = vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    let cfg = ServerConfig::from_lookup(|k| m.get(k).cloned()).unwrap();
    let plan = routing::plan(routes(), &routing::parse_routes_toml("").unwrap()).unwrap();
    let app = pl_server::build_app(&cfg, plan, test_state()).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(pl_server::serve::serve(l, app, std::future::pending()));
    a
}

async fn send(addr: SocketAddr, method: Method, path: &str, headers: &[(&str, &str)]) -> (StatusCode, HeaderMap, Bytes) {
    let mut b = hyper::Request::builder().method(method).uri(format!("http://{addr}{path}")).header("host", "127.0.0.1");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let client = Client::builder(TokioExecutor::new()).build_http::<Body>();
    let resp = tokio::time::timeout(T, client.request(b.body(Body::empty()).unwrap())).await.unwrap().unwrap();
    let (p, body) = resp.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (p.status, p.headers, bytes)
}

const NAV: (&str, &str) = ("accept", "text/html,application/xhtml+xml,*/*;q=0.8");

fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

#[tokio::test]
async fn injects_once_into_html_navigation_uncompressed() {
    let app = start(&[]).await;
    let (st, h, body) = send(app, Method::GET, "/dashboard", &[NAV]).await;
    assert_eq!(st, StatusCode::OK);
    let s = std::str::from_utf8(&body).unwrap();
    assert_eq!(count(s, "<style id=\"paperrust-brand\">"), 1);
    assert!(s.contains("--pngx-primary:18,86%;--pngx-primary-lightness:39%"));
    assert!(s.contains("href=\"/__rs/brand/favicon.ico\""));
    assert!(!s.contains("href=\"favicon.ico\""));
    assert_eq!(h["content-length"], body.len().to_string().as_str());
    assert!(h.get("content-encoding").is_none());
}

#[tokio::test]
async fn injects_and_compresses_like_django() {
    let app = start(&[]).await;
    for (ae, enc) in [("gzip, deflate, br, zstd", "zstd"), ("gzip", "gzip")] {
        let (st, h, body) = send(app, Method::GET, "/rs-page/", &[NAV, ("accept-encoding", ae)]).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h["content-encoding"], enc);
        assert_eq!(h["content-length"], body.len().to_string().as_str());
        assert!(h["vary"].to_str().unwrap().contains("Accept-Encoding"));
        let mut out = Vec::new();
        if enc == "zstd" {
            ZstdDecoder::new(&body[..]).read_to_end(&mut out).await.unwrap();
        } else {
            GzipDecoder::new(&body[..]).read_to_end(&mut out).await.unwrap();
        }
        let s = String::from_utf8(out).unwrap();
        assert_eq!(count(&s, "id=\"paperrust-brand\""), 1);
    }
}

#[tokio::test]
async fn non_navigation_and_api_untouched() {
    let app = start(&[]).await;
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("/rs-page/", &[("accept", "application/json")]),
        ("/rs-page/", &[NAV, ("sec-fetch-dest", "empty")]),
        ("/api/x/", &[NAV]),
        ("/share/abc", &[NAV]),
    ];
    for (path, hdrs) in cases {
        let (st, _, body) = send(app, Method::GET, path, hdrs).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(&body[..], index_body().as_bytes(), "{path} untouched");
    }
    // POST navigation (form submit) untouched as well
    let (_, _, body) = send(app, Method::POST, "/accounts/x-login/", &[NAV]).await;
    assert!(!std::str::from_utf8(&body).unwrap().contains("paperrust-brand"));
    // non-HTML on a candidate path: compressed by Django's rules, content unchanged
    let (_, h, body) = send(app, Method::GET, "/plain", &[NAV, ("accept-encoding", "gzip")]).await;
    assert_eq!(h["content-encoding"], "gzip");
    let mut out = Vec::new();
    GzipDecoder::new(&body[..]).read_to_end(&mut out).await.unwrap();
    assert_eq!(out, "x".repeat(1000).as_bytes());
}

#[tokio::test]
async fn off_leaves_html_untouched() {
    let app = start(&[("PAPERRUST_BRANDING", "off")]).await;
    let (st, _, body) = send(app, Method::GET, "/dashboard", &[NAV]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(&body[..], index_body().as_bytes());
    // no own assets either: 404 like any unknown path
    let (st, _, _) = send(app, Method::GET, "/__rs/brand/logo.svg", &[NAV]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn assets_served() {
    let app = start(&[]).await;
    let (st, h, body) = send(app, Method::GET, "/__rs/brand/logo.svg", &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h["content-type"], "image/svg+xml");
    assert_eq!(&body[..], pl_server::branding::LOGO_SVG);
    assert_eq!(h["x-content-type-options"], "nosniff");
    let (st, h, body) = send(app, Method::GET, "/__rs/brand/favicon.ico", &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h["content-type"], "image/x-icon");
    assert_eq!(&body[..4], &[0, 0, 1, 0]);
    let (st, h, body) = send(app, Method::HEAD, "/__rs/brand/favicon.svg", &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(h["content-length"], pl_server::branding::FAVICON_SVG.len().to_string().as_str());
    // unknown brand path -> 404 like every unknown /__rs/ path
    let (st, _, body) = send(app, Method::GET, "/__rs/brand/nope.svg", &[]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(std::str::from_utf8(&body).unwrap().contains("Not Found"));
}
