//! Integration tests for the cross-cutting pieces: "not supported" answers (404), form/multipart
//! bodies on JSON-only routes, body limits, Authz extension, request notes (X-Api-Version,
//! session-cookie deletion), and route-table consistency over all crates. There is no Django
//! backend any more. Only 127.0.0.1, no external services, no database.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::Extension;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::routing::{get, post};
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use pl_core::ApiRoute;
use pl_server::body::{Gate, gate};
use pl_server::config::ServerConfig;
use pl_server::routing::{self, Backend};
use tokio::net::TcpListener;

mod support;
use support::test_state;

const T: Duration = Duration::from_secs(20);

async fn spawn_router(app: Router) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(l, app).await.unwrap();
    });
    addr
}

fn server_config() -> ServerConfig {
    let m: HashMap<String, String> = HashMap::new();
    ServerConfig::from_lookup(|k| m.get(k).cloned()).unwrap()
}

async fn start(routes: Vec<ApiRoute>) -> SocketAddr {
    start_with(routes, test_state()).await
}

async fn start_with(routes: Vec<ApiRoute>, state: pl_core::AppState) -> SocketAddr {
    let cfg = server_config();
    let mut toml: BTreeMap<String, Backend> = BTreeMap::new();
    for r in &routes {
        toml.insert(r.id.to_owned(), Backend::Rust);
    }
    let plan = routing::plan(routes, &toml).unwrap();
    let app = pl_server::build_app(&cfg, plan, state).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(pl_server::serve::serve(l, app, std::future::pending()));
    addr
}

async fn send(
    addr: SocketAddr,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: impl Into<Body>,
) -> (StatusCode, HeaderMap, Bytes) {
    let mut b = hyper::Request::builder().method(method).uri(format!("http://{addr}{path}"));
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let client: Client<HttpConnector, Body> = Client::builder(TokioExecutor::new()).build(HttpConnector::new());
    let resp = tokio::time::timeout(T, client.request(b.body(body.into()).unwrap())).await.unwrap().unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = tokio::time::timeout(T, body.collect()).await.unwrap().unwrap().to_bytes();
    (parts.status, parts.headers, bytes)
}

fn text(b: &Bytes) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Handler that reads the body, then delegates if the body says so.
async fn maybe_delegate(body: Bytes) -> axum::response::Response {
    if body.starts_with(b"{\"delegate\"") {
        pl_core::delegate_to_django()
    } else {
        axum::response::IntoResponse::into_response(format!("rust saw {} bytes", body.len()))
    }
}

fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new("it.delegate", Method::POST, "/api/it/delegate/", "test", post(maybe_delegate)).delegatable(),
        ApiRoute::new("it.json", Method::POST, "/api/it/json/", "test", post(|b: Bytes| async move { format!("rust json {}", b.len()) })),
        ApiRoute::new("it.form", Method::POST, "/api/it/form/", "test", post(|b: Bytes| async move { format!("rust form {}", b.len()) }))
            .accepts_form(),
        ApiRoute::new(
            "it.coreerr",
            Method::POST,
            "/api/it/coreerr/",
            "test",
            post(|| async { Err::<String, _>(pl_core::ApiError::delegate_to_django("test")) }),
        ),
        ApiRoute::new(
            "it.notes",
            Method::GET,
            "/api/it/notes/",
            "test",
            get(|Extension(n): Extension<pl_core::RequestNotes>| async move {
                n.mark_authenticated();
                n.add_set_cookie(HeaderValue::from_static("sessionid=\"\"; Max-Age=0; Path=/"));
                "noted"
            }),
        ),
        ApiRoute::new(
            "it.maybe_user",
            Method::GET,
            "/api/it/user/",
            "test",
            get(|u: pl_authz::MaybeUser| async move { format!("anonymous={}", u.user().is_none()) }),
        ),
    ]
}

const NOT_FOUND_JSON: &str = "{\"detail\":\"Not found.\"}";

#[tokio::test]
async fn not_supported_answers_404() {
    let app = start(routes()).await;

    let (s, _, b) = send(app, Method::POST, "/api/it/delegate/?q=1", &[("content-type", "application/json")], "{\"x\":1}").await;
    assert_eq!((s, text(&b)), (StatusCode::OK, "rust saw 7 bytes".to_owned()));

    // A handler answering "not supported" (pl_core::delegate_to_django) → 404 like a missing route.
    let (s, _, b) = send(app, Method::POST, "/api/it/delegate/?q=1", &[("content-type", "application/json")], "{\"delegate\": true}").await;
    assert_eq!((s, text(&b)), (StatusCode::NOT_FOUND, NOT_FOUND_JSON.to_owned()));

    // pl_core::ApiError::delegate_to_django works the same way.
    let (s, _, b) = send(app, Method::POST, "/api/it/coreerr/", &[("content-type", "application/json")], "{}").await;
    assert_eq!((s, text(&b)), (StatusCode::NOT_FOUND, NOT_FOUND_JSON.to_owned()));
}

#[tokio::test]
async fn form_and_multipart_on_json_only_routes_are_not_supported() {
    let app = start(routes()).await;

    let (_, _, b) = send(app, Method::POST, "/api/it/json/", &[("content-type", "application/json")], "{}").await;
    assert_eq!(text(&b), "rust json 2");
    // below /api/: DRF's 415 for a parser the view lacks (parity run 2026-11-20), with the view's Allow
    let (s, h, b) = send(app, Method::POST, "/api/it/json/", &[("content-type", "application/x-www-form-urlencoded")], "a=1").await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(text(&b), r#"{"detail":"Unsupported media type \"application/x-www-form-urlencoded\" in request."}"#);
    assert_eq!(h["allow"], "POST, OPTIONS");
    let (s, _, b) = send(app, Method::POST, "/api/it/json/", &[("content-type", "multipart/form-data; boundary=x")], "--x--").await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(text(&b), r#"{"detail":"Unsupported media type \"multipart/form-data; boundary=x\" in request."}"#);

    // route that parses forms keeps them, but not multipart
    let (_, _, b) = send(app, Method::POST, "/api/it/form/", &[("content-type", "application/x-www-form-urlencoded")], "a=1").await;
    assert_eq!(text(&b), "rust form 3");
    let (s, _, _) = send(app, Method::POST, "/api/it/form/", &[("content-type", "multipart/form-data; boundary=x")], "--x--").await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn notes_give_api_version_and_cookie_deletion() {
    let app = start(routes()).await;
    let (s, h, b) = send(app, Method::GET, "/api/it/notes/", &[], Body::empty()).await;
    assert_eq!((s, text(&b)), (StatusCode::OK, "noted".to_owned()));
    assert_eq!(h["x-api-version"], "10");
    assert_eq!(h["x-version"], "3.4.2");
    assert_eq!(h["set-cookie"], "sessionid=\"\"; Max-Age=0; Path=/");
    assert_eq!(h["vary"], "Accept-Language, origin, Cookie");

    // Authz extension is present: anonymous MaybeUser works without a database.
    let (s, h, b) = send(app, Method::GET, "/api/it/user/", &[], Body::empty()).await;
    assert_eq!((s, text(&b)), (StatusCode::OK, "anonymous=true".to_owned()));
    assert!(h.get("x-api-version").is_none());
}

/// Other methods on a mounted `/api/` path: DRF's answer (authentication first, also for OPTIONS);
/// on other paths the 404 page without axum's injected `Allow` (docs/STANDALONE-GAPS.md §5).
#[tokio::test]
async fn unmounted_methods() {
    let app = start(routes()).await;
    let (s, h, _) = send(app, Method::OPTIONS, "/api/it/json/", &[], Body::empty()).await;
    assert!(matches!(s, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN), "OPTIONS: {s}");
    assert_eq!(h["allow"], "POST, OPTIONS");
    for m in [Method::PUT, Method::DELETE, Method::PATCH] {
        let (s, _, _) = send(app, m.clone(), "/api/it/json/", &[], Body::empty()).await;
        assert!(matches!(s, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN), "{m}: {s}");
    }
    let (s, h, _) = send(app, Method::GET, "/nope/", &[], Body::empty()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(h.get("allow").is_none());
}

/// The gate with a tiny limit (the real one is 2.5 MiB like Django): larger bodies are never
/// handled in Rust: Django's 400 page (`RequestDataTooBig`).
#[tokio::test]
async fn large_bodies() {
    let mk = |flags: pl_core::RouteFlags| Gate { flags, limit: 8, streaming: false, allow: None };
    let app = Router::new()
        .route(
            "/d/",
            post(maybe_delegate)
                .layer(axum::middleware::from_fn_with_state(mk(pl_core::RouteFlags { delegatable: true, ..Default::default() }), gate)),
        )
        .route("/s/", post(maybe_delegate).layer(axum::middleware::from_fn_with_state(mk(pl_core::RouteFlags::default()), gate)));
    let addr = spawn_router(app).await;

    let (_, _, b) = send(addr, Method::POST, "/d/", &[], "small").await;
    assert_eq!(text(&b), "rust saw 5 bytes");
    let big = "0123456789abcdef";
    let (s, _, _) = send(addr, Method::POST, "/d/", &[], big).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // streamed (chunked, no Content-Length) too
    let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from("01234")), Ok(Bytes::from("56789abc"))]);
    let (s, _, _) = send(addr, Method::POST, "/d/", &[], Body::from_stream(stream)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = send(addr, Method::POST, "/s/", &[], big).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, _, b) = send(addr, Method::POST, "/s/", &[], "12345678").await;
    assert_eq!(text(&b), "rust saw 8 bytes");
}

#[test]
fn route_ids_are_unique_across_all_crates() {
    let all = pl_server::all_routes();
    let mut seen = HashSet::new();
    let dups: Vec<&str> = all.iter().filter(|r| !seen.insert(r.id)).map(|r| r.id).collect();
    assert!(dups.is_empty(), "duplicate route ids: {dups:?}");
    // pl-ai is mounted (feature `ai`, default)
    if cfg!(feature = "ai") {
        assert!(all.iter().any(|r| r.id.starts_with("ai.")));
    }
    // upload route of the worker (W1)
    assert!(all.iter().any(|r| r.id == pl_server::upload::ROUTE_ID));
}

#[tokio::test]
async fn all_routes_can_be_mounted_together() {
    let all = pl_server::all_routes();
    let toml: BTreeMap<String, Backend> = all.iter().map(|r| (r.id.to_owned(), Backend::Rust)).collect();
    let plan = routing::plan(all, &toml).unwrap();
    let cfg = server_config();
    let _app = pl_server::build_app(&cfg, plan, test_state()).unwrap();
}

/// Real limit: a body just over DATA_UPLOAD_MAX_MEMORY_SIZE on a mounted route gets Django's
/// 400 page, one at the limit is handled by Rust.
#[tokio::test]
async fn body_limit_is_django_data_upload_max_memory_size() {
    let app = start(routes()).await;
    let limit = pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE;
    let ok = vec![b'{'; limit];
    let (_, _, b) = send(app, Method::POST, "/api/it/json/", &[("content-type", "application/json")], ok).await;
    assert_eq!(text(&b), format!("rust json {limit}"));
    let big = vec![b'{'; limit + 1];
    let (s, _, _) = send(app, Method::POST, "/api/it/json/", &[("content-type", "application/json")], big).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// A body far over the limit (Content-Length known up front) must still get the 400 answer, not a
/// connection reset: the server answers before it read the body, and closing a socket with unread
/// data makes Windows send RST, which loses the response (flaky `body_limit_…` under load).
#[tokio::test]
async fn body_far_over_limit_gets_400_not_reset() {
    let app = start(routes()).await;
    let big = vec![b'{'; 16 * 1024 * 1024];
    for _ in 0..3 {
        let (s, _, b) = send(app, Method::POST, "/api/it/json/", &[("content-type", "application/json")], big.clone()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{}", text(&b));
    }
}

/// Parity run 2026-11-20 F2: `GET /api/token/` is DRF's 405 (`Allow: POST, OPTIONS`, no
/// `Vary: Accept`: single renderer) and counts in the login throttle like every request to the
/// view (`APIView.initial` throttles before the handler is looked up); OPTIONS gives the metadata.
#[tokio::test]
async fn token_view_other_methods_and_throttle() {
    let state = test_state();
    // throttle keys in process memory (no database in these tests)
    state.keep_guarded_keys_local();
    let app = start_with(pl_auth_api::routes(), state).await;
    // own throttle bucket (DRF get_ident: X-Forwarded-For without NUM_PROXIES)
    let h = [("x-forwarded-for", "10.77.0.1")];
    let (s, hd, b) = send(app, Method::OPTIONS, "/api/token/", &h, Body::empty()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(hd["allow"], "POST, OPTIONS");
    assert!(
        text(&b).starts_with(r#"{"name":"Paperless Obtain Auth Token","description":"","renders":["application/json"],"#),
        "{}",
        text(&b)
    );
    for _ in 0..4 {
        let (s, hd, b) = send(app, Method::GET, "/api/token/", &h, Body::empty()).await;
        assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(hd["allow"], "POST, OPTIONS");
        assert!(!hd.get("vary").is_some_and(|v| v.to_str().unwrap_or("").contains("Accept,") || v == "Accept"));
        assert_eq!(text(&b), r#"{"detail":"Method \"GET\" not allowed."}"#);
    }
    // 5/min: the sixth request (after OPTIONS + 4 GETs) is throttled, whatever the method
    let (s, _, b) =
        send(app, Method::POST, "/api/token/", &[("x-forwarded-for", "10.77.0.1"), ("content-type", "application/json")], "{}").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(text(&b).starts_with(r#"{"detail":"Request was throttled. Expected available in "#), "{}", text(&b));
    let (s, _, _) = send(app, Method::DELETE, "/api/token/", &h, Body::empty()).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
}

/// Parity run 2026-11-20 F5: DRF format suffixes on router routes. `/api/tags/1.5/` (format "5")
/// is DRF's JSON 404 before authentication; `.json` reaches the view (anonymous: 401/403).
#[tokio::test]
async fn format_suffix_on_router_routes() {
    let app = start(pl_server::all_routes()).await;
    let (s, hd, b) = send(app, Method::GET, "/api/tags/1.5/", &[], Body::empty()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(hd["content-type"], "application/json");
    assert_eq!(text(&b), NOT_FOUND_JSON);
    assert_eq!(hd["allow"], "GET, PUT, PATCH, DELETE, HEAD, OPTIONS");
    assert!(hd.get("x-api-version").is_none());
    let (s, _, _) = send(app, Method::GET, "/api/tags/1.json/", &[], Body::empty()).await;
    assert!(matches!(s, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN), "{s}");
    // unknown API path: JSON 404 (not the web UI)
    let (s, hd, b) = send(app, Method::GET, "/api/no_such_thing/", &[], Body::empty()).await;
    assert_eq!((s, text(&b)), (StatusCode::NOT_FOUND, NOT_FOUND_JSON.to_owned()));
    assert_eq!(hd["content-type"], "application/json");
    // OPTIONS on a DRF view: authentication first (anonymous → 401/403 with the view's Allow)
    let (s, hd, _) = send(app, Method::OPTIONS, "/api/documents/", &[], Body::empty()).await;
    assert!(matches!(s, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN), "{s}");
    assert_eq!(hd["allow"], "GET, HEAD, OPTIONS");
}
