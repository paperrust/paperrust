//! Read-only live comparison Django reference ↔ Rust handler (in-process) for the GET routes.
//!
//! Prerequisite: reference environment is running (reference/README.md), environment set:
//! `source reference/env.sh && cargo test -p pl-fields-views -j 2 --test live_parity -- --ignored --nocapture`
//! Only GET requests are made; the reference data stays unchanged.

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request};
use tower::ServiceExt;

const DJANGO: &str = "http://127.0.0.1:8000";
const HOST: &str = "127.0.0.1:8000";

const USERS: &[(&str, Option<&str>)] = &[
    ("admin", Some("a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001")),
    ("alice", Some("a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002")),
    ("bob", Some("b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003")),
    ("carol", Some("c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004")),
    ("dave", Some("d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005")),
    ("anon", None),
];

async fn django(path: &str, token: Option<&str>, version: &str) -> (u16, String) {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    let mut req = client
        .get(format!("{DJANGO}{path}"))
        .header("Accept", format!("application/json; version={version}"));
    if let Some(t) = token {
        req = req.header("Authorization", format!("Token {t}"));
    }
    let resp = req.send().await.expect("django reachable");
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap_or_default())
}

async fn rust(app: &Router, path: &str, token: Option<&str>, version: &str) -> (u16, String) {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("Host", HOST)
        .header("Accept", format!("application/json; version={version}"));
    if let Some(t) = token {
        b = b.header("Authorization", format!("Token {t}"));
    }
    let req = b.body(Body::empty()).expect("request");
    let resp = app.clone().oneshot(req).await.expect("infallible");
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn first_id(list: &str) -> Option<i64> {
    let (_, body) = django(
        &format!("{list}?ordering=id&page_size=1"),
        USERS[0].1,
        "10",
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    v["results"][0]["id"].as_i64()
}

#[tokio::test]
#[ignore = "braucht die laufende Referenzumgebung (Django 127.0.0.1:8000, PostgreSQL) und reference/env.sh"]
async fn live_read_parity() {
    let state = pl_core::AppState::from_env().await.expect("AppState from env");
    let authz = pl_authz::Authz::new(
        state.inner.db.clone(),
        pl_authz::AuthConfig::from_env().expect("AuthConfig"),
    );
    let mut router: Router<pl_core::AppState> = Router::new();
    for r in pl_fields_views::routes() {
        if r.method == Method::GET {
            router = router.route(r.path, r.handler);
        }
    }
    let app = router
        .layer(axum::Extension(authz))
        .with_state(state);

    let mut paths: Vec<String> = vec![
        "/api/custom_fields/".into(),
        "/api/custom_fields/?ordering=-document_count".into(),
        "/api/custom_fields/?ordering=data_type,-name".into(),
        "/api/custom_fields/?name__icontains=a".into(),
        "/api/custom_fields/?name__iexact=x&id=1".into(),
        "/api/custom_fields/?id__in=1,2,3".into(),
        "/api/custom_fields/?id=abc".into(),
        "/api/custom_fields/?page=2&page_size=1".into(),
        "/api/custom_fields/?page=999".into(),
        "/api/custom_fields/abc/".into(),
        "/api/custom_fields/999999/".into(),
        "/api/saved_views/".into(),
        "/api/saved_views/?full_perms=true".into(),
        "/api/saved_views/?ordering=-name&page_size=2".into(),
        "/api/saved_views/999999/".into(),
        "/api/share_links/".into(),
        "/api/share_links/?ordering=-document__title".into(),
        "/api/share_links/?created__date__gt=2000-01-01".into(),
        "/api/share_links/?created__gt=x".into(),
        "/api/share_links/?expiration__year=2099".into(),
        "/api/share_links/999999/".into(),
        "/api/share_link_bundles/".into(),
        "/api/share_link_bundles/?status=ready".into(),
        "/api/share_link_bundles/?status=bogus".into(),
        "/api/share_link_bundles/?documents=1,2".into(),
        "/api/share_link_bundles/?documents=x".into(),
        "/api/share_link_bundles/999999/".into(),
    ];
    for list in [
        "/api/custom_fields/",
        "/api/saved_views/",
        "/api/share_links/",
        "/api/share_link_bundles/",
    ] {
        if let Some(id) = first_id(list).await {
            paths.push(format!("{list}{id}/"));
        }
    }

    let mut total = 0;
    let mut diffs = Vec::new();
    for path in &paths {
        for (name, token) in USERS {
            for version in ["10", "9"] {
                total += 1;
                let a = django(path, *token, version).await;
                let b = rust(&app, path, *token, version).await;
                if a != b {
                    diffs.push(format!(
                        "{path} @{name}/v{version}\n  django {}: {}\n  rust   {}: {}",
                        a.0,
                        a.1.chars().take(400).collect::<String>(),
                        b.0,
                        b.1.chars().take(400).collect::<String>()
                    ));
                }
            }
        }
    }
    println!("verglichen: {total}, verschieden: {}", diffs.len());
    for d in &diffs {
        println!("{d}");
    }
    assert!(diffs.is_empty(), "{} Abweichungen", diffs.len());
}
