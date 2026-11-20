//! HTTP comparison Rust handler ↔ Django (reference environment) for read routes and
//! requests that fail before any write. No own server: the routes run
//! in-process via `tower::ServiceExt::oneshot`.
//!
//! Run: `cargo test -p pl-workflows -j 2 --test http -- --ignored`

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use pl_core::config::Env;
use pl_core::{AppState, Config};
use serde_json::{Value, json};
use tower::ServiceExt as _;

const DB: &str = "postgres://paperless:paperless@127.0.0.1:54329/paperless";
const DJANGO: &str = "http://127.0.0.1:8000";
const ADMIN: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const ALICE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";
const BOB: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003";
const CAROL: &str = "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004";
const DAVE: &str = "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005";

async fn app() -> Router {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(DB).await.expect("db");
    let env = Env::from_pairs([
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERLESS_ALLOWED_HOSTS", "127.0.0.1,localhost"),
        ("PAPERLESS_URL", "http://127.0.0.1:8000"),
        ("PAPERLESS_SECRET_KEY", "paperless-rs-reference-secret-key-local-only-0123456789"),
    ]);
    let config = Config::from_env_map(env).expect("config");
    let state = AppState::with_pool(config, pool.clone()).expect("state");
    let authz = pl_authz::Authz::new(
        pool,
        pl_authz::AuthConfig::for_tests("paperless-rs-reference-secret-key-local-only-0123456789"),
    );
    let mut router: Router<AppState> = Router::new();
    for r in pl_workflows::routes() {
        router = router.route(r.path, r.handler);
    }
    router.layer(axum::Extension(authz)).with_state(state)
}

struct Resp {
    status: u16,
    content_type: String,
    allow: String,
    body: Value,
}

fn parse(status: u16, ct: String, allow: String, text: String) -> Resp {
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
    Resp { status, content_type: ct, allow, body }
}

async fn rust(app: &Router, method: &str, path: &str, token: Option<&str>, body: Option<&str>, ct: &str, accept: &str) -> Resp {
    let mut b = Request::builder().method(method).uri(path).header("Host", "127.0.0.1:8000").header("Accept", accept);
    if let Some(t) = token {
        b = b.header("Authorization", format!("Token {t}"));
    }
    if body.is_some() {
        b = b.header("Content-Type", ct);
    }
    let req = b.body(Body::from(body.unwrap_or("").to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let h = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (ct, allow) = (h("content-type"), h("allow"));
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    parse(status, ct, allow, String::from_utf8_lossy(&bytes).to_string())
}

async fn django(method: &str, path: &str, token: Option<&str>, body: Option<&str>, ct: &str, accept: &str) -> Resp {
    let client = reqwest::Client::new();
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut b = client.request(m, format!("{DJANGO}{path}")).header("Accept", accept);
    if let Some(t) = token {
        b = b.header("Authorization", format!("Token {t}"));
    }
    if let Some(x) = body {
        b = b.header("Content-Type", ct).body(x.to_string());
    }
    let resp = b.send().await.unwrap();
    let status = resp.status().as_u16();
    let h = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (ct, allow) = (h("content-type"), h("allow"));
    parse(status, ct, allow, resp.text().await.unwrap())
}

#[tokio::test]
#[ignore = "braucht Referenz-DB und Django (reference/README.md)"]
async fn http_matches_django() {
    let app = app().await;
    let v10 = "application/json; version=10";
    let v9 = "application/json; version=9";
    type Case<'a> = (&'a str, String, Option<&'a str>, Option<String>, &'a str, &'a str);
    let mut cases: Vec<Case> = Vec::new();
    for base in ["/api/workflows/", "/api/workflow_triggers/", "/api/workflow_actions/"] {
        for tok in [Some(ADMIN), Some(ALICE), Some(BOB), Some(CAROL), Some(DAVE), None] {
            cases.push(("GET", base.to_string(), tok, None, "", v10));
        }
        for q in ["?page=2&page_size=1", "?page_size=1&page=last", "?page=9", "?page=0", "?page=x", "?page_size=0", "?page_size=abc&b=%C3%A4&a=1"] {
            cases.push(("GET", format!("{base}{q}"), Some(ADMIN), None, "", v10));
        }
        cases.push(("GET", base.to_string(), Some(ADMIN), None, "", v9));
        cases.push(("GET", format!("{base}?page_size=2"), Some(ADMIN), None, "", v9));
        cases.push(("GET", base.to_string(), Some(ADMIN), None, "", "application/json; version=8"));
        for id in ["1", "2", "999999", "99999999999", "abc", "-1"] {
            cases.push(("GET", format!("{base}{id}/"), Some(ADMIN), None, "", v10));
        }
        cases.push(("GET", format!("{base}1/"), Some(CAROL), None, "", v10));
        cases.push(("GET", format!("{base}999999/"), Some(CAROL), None, "", v10));
        cases.push(("DELETE", format!("{base}1/"), Some(CAROL), None, "", v10));
        cases.push(("DELETE", format!("{base}999999/"), Some(ADMIN), None, "", v10));
        cases.push(("POST", base.to_string(), Some(ALICE), Some("{}".into()), "application/json", v10));
        cases.push(("POST", base.to_string(), Some(ADMIN), Some("[1, 2]".into()), "application/json", v10));
        cases.push(("POST", base.to_string(), Some(ADMIN), Some("null".into()), "application/json", v10));
        cases.push(("POST", base.to_string(), Some(ADMIN), Some("x".into()), "text/plain", v10));
        cases.push(("PUT", format!("{base}999999/"), Some(ADMIN), Some("{}".into()), "application/json", v10));
        cases.push(("PUT", format!("{base}1/"), Some(ADMIN), Some("null".into()), "application/json", v10));
        cases.push(("PATCH", format!("{base}1/"), Some(CAROL), Some("{}".into()), "application/json", v10));
    }
    // Trigger/action: ID check on PATCH (before get_object)
    for base in ["/api/workflow_triggers/", "/api/workflow_actions/"] {
        cases.push(("PATCH", format!("{base}1/"), Some(ADMIN), Some(r#"{"id": 2}"#.into()), "application/json", v10));
        cases.push(("PATCH", format!("{base}1/"), Some(ADMIN), Some(r#"{"id": 1.0}"#.into()), "application/json", v10));
        cases.push(("PATCH", format!("{base}999999/"), Some(ADMIN), Some(r#"{"id": 5}"#.into()), "application/json", v10));
        cases.push(("PATCH", format!("{base}999999/"), Some(ADMIN), Some(r#"{"id": 999999}"#.into()), "application/json", v10));
    }
    cases.push(("PATCH", "/api/workflow_triggers/1/".into(), Some(ADMIN), Some(r#"{"type": 1, "filter_filename": null}"#.into()), "application/json", v10));
    cases.push(("PATCH", "/api/workflows/1/".into(), Some(ADMIN), Some(r#"{"name": "Invoice intake", "triggers": [{"type": 1}]}"#.into()), "application/json", v10));
    cases.push(("PATCH", "/api/workflows/1/".into(), Some(ADMIN), Some(r#"{"name": "Invoice intake 9", "actions": [{"type": 8, "ai_suggestion_fields": ["nope"]}]}"#.into()), "application/json", v10));

    let mut failures = Vec::new();
    for (m, path, tok, body, ct, accept) in &cases {
        let a = django(m, path, *tok, body.as_deref(), ct, accept).await;
        let b = rust(&app, m, path, *tok, body.as_deref(), ct, accept).await;
        assert!(
            !(matches!(*m, "POST" | "PUT" | "PATCH" | "DELETE") && (200..300).contains(&a.status)),
            "Django hat einen Schreibzugriff ausgeführt: {m} {path} {body:?}"
        );
        // Django 500s: compare only the status
        let same = a.status == b.status
            && (a.status >= 500 || (a.body == b.body && a.content_type == b.content_type && a.allow == b.allow));
        if !same {
            failures.push(format!(
                "{m} {path} tok={tok:?} body={body:?} accept={accept}\n  django {} {} [{}] {}\n  rust   {} {} [{}] {}",
                a.status, a.content_type, a.allow, a.body, b.status, b.content_type, b.allow, b.body
            ));
        }
    }
    assert!(failures.is_empty(), "{} von {} unterschiedlich:\n{}", failures.len(), cases.len(), failures.join("\n"));
    let _ = (StatusCode::OK, json!(null));
}
