//! Parity comparison `GET /api/trash/` Django ↔ Rust (in-process with `pl_authz::Authz`), read-only.
//!
//! ```sh
//! source reference/env.sh
//! cargo test -p pl-documents-write -j 1 --test trash_parity -- --ignored --nocapture
//! ```

use axum::body::Body;
use axum::http::Request;
use pl_authz::{AuthConfig, Authz};
use serde_json::Value;
use tower::ServiceExt;

const DJANGO: &str = "http://127.0.0.1:8000";

fn normalize(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => serde_json::to_string(&v).unwrap_or_default(),
        Err(_) => body.to_string(),
    }
}

#[tokio::test]
#[ignore = "braucht Referenz-DB und Django (reference/README.md)"]
async fn trash_list_matches_django() {
    let state = pl_core::AppState::from_env().await.expect("state");
    let authz = Authz::new(state.db().clone(), AuthConfig::from_core(state.config()));
    let mut router: axum::Router<pl_core::AppState> = axum::Router::new();
    for r in pl_documents_write::routes().into_iter().filter(|r| r.id == "trash.list") {
        router = router.route(r.path, r.handler);
    }
    let app: axum::Router = router.with_state(state).layer(axum::Extension(authz));
    let client = reqwest::Client::new();
    let users = [
        Some("Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"),
        Some("Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002"),
        Some("Token b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003"),
        Some("Token c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004"),
        Some("Token d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005"),
        Some("Token nope"),
        None,
    ];
    let paths = [
        "/api/trash/",
        "/api/trash/?full_perms=true",
        "/api/trash/?page_size=2",
        "/api/trash/?page=2&page_size=2",
        "/api/trash/?page=last&page_size=2",
        "/api/trash/?page=9",
        "/api/trash/?page=x",
        "/api/trash/?fields=id&truncate_content=true",
    ];
    let (mut same, mut diff) = (0, 0);
    for auth in users {
        for accept in ["application/json", "application/json; version=9"] {
            for path in paths {
                let mut rb = client.get(format!("{DJANGO}{path}")).header("Accept", accept);
                if let Some(a) = auth {
                    rb = rb.header("Authorization", a);
                }
                let d = rb.send().await.expect("django");
                let d_status = d.status().as_u16();
                let d_ct = d.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let d_body = d.text().await.unwrap_or_default();

                let mut req = Request::builder().uri(path).header("host", "127.0.0.1:8000").header("accept", accept);
                if let Some(a) = auth {
                    req = req.header("authorization", a);
                }
                let r = app.clone().oneshot(req.body(Body::empty()).expect("req")).await.expect("rust");
                let r_status = r.status().as_u16();
                let r_ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let bytes = axum::body::to_bytes(r.into_body(), 64 << 20).await.unwrap_or_default();
                let r_body = String::from_utf8_lossy(&bytes).into_owned();
                if d_status == r_status && d_ct.split(';').next() == r_ct.split(';').next() && normalize(&d_body) == normalize(&r_body) {
                    same += 1;
                } else {
                    diff += 1;
                    println!("DIFF {auth:?} [{accept}] {path}");
                    println!("  django {d_status} {d_ct}: {}", d_body.chars().take(800).collect::<String>());
                    println!("  rust   {r_status} {r_ct}: {}", r_body.chars().take(800).collect::<String>());
                }
            }
        }
    }
    println!("PARITY trash.list same={same} different={diff}");
    assert_eq!(diff, 0);
}
