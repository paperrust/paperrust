//! Parity against the reference environment (`reference/README.md`): every request is sent to
//! Django (127.0.0.1:8000) and, in-process, to the Rust handlers of this crate (same DB, same
//! user tokens). Read-only GET requests only – the reference data is never changed.
//!
//! Run: `cargo test -p pl-system -j 2 --test reference_parity -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use pl_core::config::Env;
use pl_core::{AppState, Config};
use serde_json::Value;
use tower::ServiceExt as _;

const DJANGO: &str = "http://127.0.0.1:8000";
const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn reference_env() -> Env {
    let r = repo().join("reference");
    let w = |p: &str| r.join(p).to_string_lossy().replace('/', "\\");
    Env::from_pairs([
        ("PAPERLESS_DBENGINE", "postgresql".to_string()),
        ("PAPERLESS_DBHOST", "127.0.0.1".into()),
        ("PAPERLESS_DBPORT", "54329".into()),
        ("PAPERLESS_DBNAME", "paperless".into()),
        ("PAPERLESS_DBUSER", "paperless".into()),
        ("PAPERLESS_DBPASS", "paperless".into()),
        ("PAPERLESS_REDIS", "redis://127.0.0.1:63799".into()),
        ("PAPERLESS_SECRET_KEY", SECRET.into()),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich".into()),
        ("PAPERLESS_DATA_DIR", w("data/data")),
        ("PAPERLESS_MEDIA_ROOT", w("data/media")),
        ("PAPERLESS_CONSUMPTION_DIR", w("data/consume")),
        ("PAPERLESS_STATICDIR", w("data/static")),
        ("PAPERLESS_LOGGING_DIR", w("logs")),
        ("PAPERLESS_URL", DJANGO.into()),
        ("PAPERLESS_CSRF_TRUSTED_ORIGINS", "http://127.0.0.1:8000,http://localhost:8000".into()),
        ("PAPERLESS_ALLOWED_HOSTS", "127.0.0.1,localhost".into()),
        ("PAPERLESS_DEBUG", "false".into()),
        ("PAPERLESS_OCR_LANGUAGE", "eng".into()),
    ])
}

fn app() -> Router {
    let config = Config::from_env_map(reference_env()).expect("config");
    let state = AppState::new(config).expect("state");
    let authz = pl_authz::Authz::new(state.db().clone(), pl_authz::AuthConfig::for_tests(SECRET));
    let mut by_path: BTreeMap<&str, axum::routing::MethodRouter<AppState>> = BTreeMap::new();
    for r in pl_system::routes() {
        let mr = match by_path.remove(r.path) {
            Some(prev) => prev.merge(r.handler),
            None => r.handler,
        };
        by_path.insert(r.path, mr);
    }
    let mut router: Router<AppState> = Router::new();
    for (p, mr) in by_path {
        router = router.route(p, mr);
    }
    router.layer(axum::Extension(authz)).with_state(state)
}

async fn rust(app: &Router, path: &str, token: &str, accept: &str, lang: Option<&str>) -> (u16, Value) {
    let mut b = Request::get(path)
        .header("host", "127.0.0.1:8000")
        .header("authorization", format!("Token {token}"))
        .header("accept", accept);
    if let Some(l) = lang {
        b = b.header("accept-language", l);
    }
    let resp = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), 50_000_000).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into())))
}

async fn django(c: &reqwest::Client, path: &str, token: &str, accept: &str, lang: Option<&str>) -> (u16, Value) {
    let mut r = c.get(format!("{DJANGO}{path}")).header("authorization", format!("Token {token}")).header("accept", accept);
    if let Some(l) = lang {
        r = r.header("accept-language", l);
    }
    let resp = r.send().await.expect("django reachable");
    let status = resp.status().as_u16();
    let body = resp.bytes().await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into())))
}

const TOKENS: &[(&str, &str)] = &[
    ("admin", "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"),
    ("alice", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002"),
    ("bob", "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003"),
    ("carol", "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004"),
];

const PATHS: &[&str] = &[
    "/api/logs/",
    "/api/logs/bogus/",
    "/api/logs/paperless/?limit=abc",
    "/api/logs/paperless/?limit=0",
    "/api/statistics/",
    "/api/status/",
    "/api/config/",
    "/api/config/1/",
    "/api/config/999/",
    "/api/config/abc/",
    "/api/tasks/",
    "/api/tasks/?page=2&page_size=5",
    "/api/tasks/?page=99",
    "/api/tasks/?ordering=status,-date_done",
    "/api/tasks/?ordering=task_type",
    "/api/tasks/?status=success&status=failure",
    "/api/tasks/?status=bogus",
    "/api/tasks/?status=",
    "/api/tasks/?is_complete=true",
    "/api/tasks/?is_complete=0",
    "/api/tasks/?acknowledged=false",
    "/api/tasks/?name=consume",
    "/api/tasks/?name=pdf",
    "/api/tasks/?name=Verarbeitung",
    "/api/tasks/?result=duplicate",
    "/api/tasks/?result=1",
    "/api/tasks/?owner=abc",
    "/api/tasks/?owner=3",
    "/api/tasks/?owner=999",
    "/api/tasks/?task_type=consume_file&trigger_source=web_ui",
    "/api/tasks/?date_created_after=2020-01-01",
    "/api/tasks/?date_created_before=nope",
    "/api/tasks/?task_id=does-not-exist",
    "/api/tasks/summary/?days=abc",
    "/api/tasks/summary/?days=100000",
    "/api/tasks/status_counts/",
    "/api/tasks/status_counts/?status=failure&task_type=consume_file",
    "/api/tasks/active/",
    "/api/tasks/1/",
    "/api/tasks/2/",
    "/api/tasks/999999/",
    "/api/tasks/99999999999/",
    "/api/tasks/abc/",
    "/api/tasks/1/?status=bogus",
];

const GERMAN_OK: &[&str] = &["/api/tasks/", "/api/tasks/?name=consume", "/api/tasks/?name=Verarbeitung", "/api/tasks/1/", "/api/tasks/active/"];

/// Requests whose answer depends on "now" or the host and are compared loosely.
fn normalise(path: &str, mut v: Value) -> Value {
    if path.starts_with("/api/logs/") {
        // log files grow while Django runs (django.*.log is written by the request itself)
        return v;
    }
    if path.starts_with("/api/tasks/summary/") {
        // date_created >= now - days: identical within the same second
        return v;
    }
    if let Value::Object(m) = &mut v {
        // platform.platform() of the Windows reference is not reproduced (see contract)
        m.remove("server_os");
        // R4: PaperRust extra object (does not exist in Django)
        m.remove("paperrust");
        if let Some(Value::Object(st)) = m.get_mut("storage") {
            st.remove("available");
        }
    }
    v
}

#[tokio::test]
#[ignore = "needs the reference environment (Django :8000, PostgreSQL :54329)"]
async fn parity_with_django() {
    let app = app();
    let client = reqwest::Client::new();
    // Django translates model labels only with compiled catalogs (Dockerfile runs
    // `compilemessages`); the reference has none → compare German only when they exist.
    let german = repo().join("upstream/src/locale/de_DE/LC_MESSAGES/django.mo").is_file();
    if !german {
        println!("note: no compiled .mo catalogs in upstream/src/locale – German cases skipped");
    }
    let mut different = 0;
    let mut total = 0;
    for (who, tok) in TOKENS {
        for accept in ["application/json", "application/json; version=9"] {
            // German only where no DRF/Django error text is involved (those are translated by
            // Django; pl-core renders English – see contract, Known differences)
            for lang in [None, Some("de-DE,de;q=0.9")] {
                for p in PATHS {
                    if lang.is_some() && (!german || *who == "carol" || !GERMAN_OK.contains(p)) {
                        continue;
                    }
                    total += 1;
                    let (ds, dj) = django(&client, p, tok, accept, lang).await;
                    let (rs, ru) = rust(&app, p, tok, accept, lang).await;
                    let (dj, ru) = (normalise(p, dj), normalise(p, ru));
                    if ds != rs || dj != ru {
                        different += 1;
                        let d = dj.to_string();
                        let r = ru.to_string();
                        println!("DIFF [{who} {accept} {lang:?}] {p}: django={ds} rust={rs}");
                        println!("  django: {}", &d[..d.len().min(600)]);
                        println!("  rust:   {}", &r[..r.len().min(600)]);
                    }
                }
            }
        }
    }
    println!("total={total} different={different}");
    assert_eq!(different, 0);
}

async fn rust_post(app: &Router, path: &str, token: &str, body: &str) -> (u16, Value) {
    let req = Request::post(path)
        .header("host", "127.0.0.1:8000")
        .header("authorization", format!("Token {token}"))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), 10_000_000).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into())))
}

async fn django_post(c: &reqwest::Client, path: &str, token: &str, body: &str) -> (u16, Value) {
    let resp = c
        .post(format!("{DJANGO}{path}"))
        .header("authorization", format!("Token {token}"))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("django reachable");
    let status = resp.status().as_u16();
    let body = resp.bytes().await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into())))
}

/// POST cases that are rejected before any write (validation/permission errors only).
#[tokio::test]
#[ignore = "needs the reference environment (Django :8000, PostgreSQL :54329)"]
async fn parity_rejected_posts() {
    let app = app();
    let client = reqwest::Client::new();
    let cases: &[(&str, &str)] = &[
        ("/api/tasks/acknowledge/", "{}"),
        ("/api/tasks/acknowledge/", r#"{"tasks": "1"}"#),
        ("/api/tasks/acknowledge/", r#"{"tasks": [1, "x"]}"#),
        ("/api/tasks/acknowledge/", r#"{"tasks": [999999]}"#),
        ("/api/tasks/acknowledge/", r#"{"tasks": [1, 1]}"#),
        ("/api/tasks/acknowledge/", r#"{"tasks": [1], "all": true}"#),
        ("/api/tasks/acknowledge/", r#"{"all": "maybe"}"#),
        ("/api/tasks/acknowledge/", r#"{"all": false}"#),
        ("/api/tasks/acknowledge/", r#"[1]"#),
        ("/api/tasks/run/", "{}"),
        ("/api/tasks/run/", r#"{"task_type": "bogus"}"#),
        ("/api/tasks/run/", r#"{"task_type": "consume_file"}"#),
        ("/api/tasks/run/", r#"{"task_type": null}"#),
        ("/api/config/", "{}"),
    ];
    let mut different = 0;
    for (who, tok) in TOKENS {
        for (path, body) in cases {
            // a superuser with a valid runnable task would enqueue it – not part of these cases
            let (ds, dj) = django_post(&client, path, tok, body).await;
            let (rs, ru) = rust_post(&app, path, tok, body).await;
            if ds != rs || dj != ru {
                different += 1;
                println!("DIFF [{who}] POST {path} {body}: django={ds} {dj} rust={rs} {ru}");
            }
        }
    }
    println!("different={different}");
    assert_eq!(different, 0);
}
