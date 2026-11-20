//! Read-only parity check of the GET routes of pl-objects against the running Django reference
//! (`reference/README.md`). Ignored by default; run with the reference environment loaded:
//!
//! ```bash
//! source reference/env.sh
//! CARGO_TARGET_DIR=... cargo test -p pl-objects -j 2 --test reference_parity -- --ignored --nocapture
//! ```
//!
//! Only GET requests are sent (nothing is written to the reference database).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_authz::{AuthConfig, Authz};
use pl_core::AppState;
use serde_json::Value;
use tower::ServiceExt;

const DJANGO: &str = "http://127.0.0.1:8000";
const HOST: &str = "127.0.0.1:8000";
const TOKENS: &[(&str, &str)] = &[
    ("admin", "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"),
    ("alice", "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002"),
    ("bob", "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003"),
    ("carol", "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004"),
];

fn paths() -> Vec<String> {
    let mut v: Vec<String> = vec![];
    for k in ["tags", "correspondents", "document_types", "storage_paths"] {
        for q in [
            "",
            "?page_size=3",
            "?page_size=3&page=2",
            "?page=999",
            "?page=abc",
            "?ordering=-document_count",
            "?ordering=match,-name",
            "?ordering=bogus",
            "?full_perms=true&page_size=100",
            "?name__icontains=a",
            "?name__istartswith=ta",
            "?name__iexact=finance",
            "?name__iendswith=s",
            "?id__in=1,2,3",
            "?id__in=a,b",
            "?id=abc",
            "?id=2",
            "?id=2.0",
        ] {
            v.push(format!("/api/{k}/{q}"));
        }
        for id in ["1", "2", "3", "5", "13", "15", "19", "999", "abc"] {
            v.push(format!("/api/{k}/{id}/"));
            v.push(format!("/api/{k}/{id}/?full_perms=1"));
        }
    }
    for q in [
        "?is_root=true",
        "?is_root=false",
        "?is_root=1",
        "?is_root=x",
        "?page_size=1&ordering=-name",
        "?page_size=2&ordering=color",
        "?name__icontains=tax",
        "?full_perms=true&is_root=true",
    ] {
        v.push(format!("/api/tags/{q}"));
    }
    for q in ["?last_correspondence=true", "?last_correspondence=false&page_size=100", "?last_correspondence=1&ordering=-last_correspondence"] {
        v.push(format!("/api/correspondents/{q}"));
    }
    v.push("/api/storage_paths/?path__icontains=bob".into());
    v.push("/api/tags/2/?ordering=-name".into());
    v
}

async fn django(client: &reqwest::Client, path: &str, token: &str, version: &str) -> (u16, Option<Value>, String) {
    let r = client
        .get(format!("{DJANGO}{path}"))
        .header("Authorization", format!("Token {token}"))
        .header("Accept", format!("application/json; version={version}"))
        .send()
        .await
        .expect("django reachable");
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    (status, serde_json::from_str(&text).ok(), text)
}

async fn rust(app: &Router, path: &str, token: &str, version: &str) -> (u16, Option<Value>, String) {
    let req = Request::get(path)
        .header("Host", HOST)
        .header("Authorization", format!("Token {token}"))
        .header("Accept", format!("application/json; version={version}"))
        .body(Body::empty())
        .unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    let status: StatusCode = r.status();
    let bytes = axum::body::to_bytes(r.into_body(), 50_000_000).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (status.as_u16(), serde_json::from_str(&text).ok(), text)
}

#[tokio::test]
#[ignore = "needs the running Django reference + PostgreSQL (reference/README.md) and its env"]
async fn get_routes_match_django() {
    let state = AppState::from_env().await.expect("PAPERLESS_* env (source reference/env.sh)");
    let authz = Authz::new(state.db().clone(), AuthConfig::from_env().expect("auth config"));
    let mut app: Router<AppState> = Router::new();
    for r in pl_objects::routes().into_iter().filter(|r| r.method == axum::http::Method::GET) {
        app = app.route(r.path, r.handler);
    }
    let app: Router = app.with_state(state).layer(axum::Extension(authz));
    let client = reqwest::Client::new();
    let mut diffs = 0;
    let mut total = 0;
    for (user, token) in TOKENS {
        for version in ["10", "9"] {
            for p in paths() {
                total += 1;
                let a = django(&client, &p, token, version).await;
                let b = rust(&app, &p, token, version).await;
                let same = a.0 == b.0 && match (&a.1, &b.1) {
                    (Some(x), Some(y)) => x == y,
                    (None, None) => a.2.trim() == b.2.trim(),
                    _ => false,
                };
                if !same {
                    diffs += 1;
                    println!("DIFF {user} v{version} {p}\n  django {} {}\n  rust   {} {}", a.0, trunc(&a.2), b.0, trunc(&b.2));
                }
            }
        }
    }
    println!("{diffs} differences in {total} requests");
    assert_eq!(diffs, 0);
}

fn trunc(s: &str) -> String {
    if s.len() > 600 { format!("{}…", &s[..s.char_indices().nth(600).map(|x| x.0).unwrap_or(s.len())]) } else { s.to_string() }
}
