//! User documentation under `/rs/docs/` (`rs.docs.index`, `rs.docs.page`): rendered pages,
//! language negotiation (UI language cookie / Accept-Language), online redirect
//! (`PAPERRUST_DOCS_URL`). No database: anonymous requests only.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, StatusCode};
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

async fn start(extra: &[(&str, &str)]) -> SocketAddr {
    let mut env = vec![("PAPERLESS_SECRET_KEY", "test-only-secret-key-0123456789")];
    env.extend_from_slice(extra);
    let config = pl_core::Config::from_env_map(Env::from_pairs(env)).unwrap();
    let db = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://localhost/unused")
        .unwrap();
    let state = AppState::with_pool(config, db).unwrap();
    let m: HashMap<String, String> = [
        ("PAPERLESS_ALLOWED_HOSTS".into(), "127.0.0.1".into()),
    ]
    .into();
    let cfg = ServerConfig::from_lookup(|k| m.get(k).cloned()).unwrap();
    let on = ["rs.docs.index", "rs.docs.page", "ui.index"];
    let toml: BTreeMap<String, Backend> = pl_server::all_routes()
        .iter()
        .map(|r| (r.id.to_owned(), if on.contains(&r.id) { Backend::Rust } else { Backend::Off }))
        .collect();
    let plan = routing::plan(pl_server::all_routes(), &toml).unwrap();
    let app = pl_server::build_app(&cfg, plan, state).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(pl_server::serve::serve(l, app, std::future::pending()));
    addr
}

async fn get(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Bytes) {
    let mut b = hyper::Request::builder()
        .uri(format!("http://{addr}{path}"))
        .header("host", "127.0.0.1");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let client: Client<HttpConnector, Body> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let resp = tokio::time::timeout(T, client.request(b.body(Body::empty()).unwrap()))
        .await
        .unwrap()
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = tokio::time::timeout(T, body.collect())
        .await
        .unwrap()
        .unwrap()
        .to_bytes();
    (parts.status, parts.headers, bytes)
}

fn location(h: &HeaderMap) -> &str {
    h.get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

#[tokio::test]
async fn docs_pages_and_language() {
    let addr = start(&[]).await;

    let (st, h, body) = get(addr, "/rs/docs/de/usage.md", &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h["content-type"], "text/html; charset=utf-8");
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("<html lang=\"de\">") && html.contains("<h1"),
        "{}",
        &html[..300.min(html.len())]
    );
    assert!(
        html.contains("id=\"papierkorb\""),
        "anchor of doc-links.json 'usage.md#papierkorb'"
    );

    let (st, _, body) = get(addr, "/rs/docs/en/configuration.md", &[]).await;
    assert_eq!(st, StatusCode::OK);
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("<html lang=\"en\">") && html.contains("<table>"));
    assert!(
        html.contains("id=\"paperless_ocr_language\""),
        "config anchors (configuration/#PAPERLESS_X)"
    );

    // without language: the UI's language cookie, then Accept-Language, default English
    let (st, h, _) = get(
        addr,
        "/rs/docs/usage.md",
        &[("cookie", "django_language=de-de")],
    )
    .await;
    assert_eq!((st, location(&h)), (StatusCode::FOUND, "de/usage.md"));
    let (_, h, _) = get(
        addr,
        "/rs/docs/usage.md",
        &[("accept-language", "de-CH,de;q=0.9")],
    )
    .await;
    assert_eq!(location(&h), "de/usage.md");
    let (_, h, _) = get(
        addr,
        "/rs/docs/usage.md",
        &[("cookie", "django_language=fr-fr")],
    )
    .await;
    assert_eq!(location(&h), "en/usage.md");
    let (_, h, _) = get(addr, "/rs/docs/", &[]).await;
    assert_eq!(location(&h), "en/README.md");
    let (_, h, _) = get(
        addr,
        "/rs/docs/README.md",
        &[("cookie", "django_language=de")],
    )
    .await;
    assert_eq!(location(&h), "de/README.md");
    let (_, h, _) = get(addr, "/rs/docs/en/", &[]).await;
    assert_eq!(location(&h), "README.md");

    let (st, _, _) = get(addr, "/rs/docs/de/does-not-exist.md", &[]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = get(addr, "/rs/docs/de/..%2F..%2FCargo.toml", &[]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn docs_online_base_redirects() {
    let addr = start(&[("PAPERRUST_DOCS_URL", "https://docs.example.org/user/")]).await;
    let (st, h, _) = get(addr, "/rs/docs/en/usage.md", &[]).await;
    assert_eq!(
        (st, location(&h)),
        (
            StatusCode::FOUND,
            "https://docs.example.org/user/en/usage.md"
        )
    );
    let (_, h, _) = get(
        addr,
        "/rs/docs/workflows.md",
        &[("cookie", "django_language=de")],
    )
    .await;
    assert_eq!(location(&h), "https://docs.example.org/user/workflows.md");
}
