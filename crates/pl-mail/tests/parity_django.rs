//! Read-only parity comparison Django (reference, 127.0.0.1:8000) ↔ pl-mail (in-process) – GET only.
//!
//! `#[ignore]`: needs reference PostgreSQL + Django + `reference/users.json`.
//! `PL_MAIL_TEST_DB=postgres://paperless:paperless@127.0.0.1:54329/paperless \
//!  cargo test -p pl-mail -j 2 --test parity_django -- --ignored --nocapture`

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use pl_core::AppState;
use pl_core::config::{Config, Env};
use pl_mail::MailUser;
use serde_json::Value;
use tower::ServiceExt;

const DJANGO: &str = "http://127.0.0.1:8000";
const HOST: &str = "127.0.0.1:8000";

async fn setup() -> (Router, sqlx::PgPool) {
    let url = std::env::var("PL_MAIL_TEST_DB").expect("PL_MAIL_TEST_DB");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.expect("db");
    let cfg = Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "pl-mail-tests-only-0123456789abcdefghijklmnopqrstuvwxyz"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
    ]))
    .map_err(|e| format!("{e:?}"))
    .expect("config");
    let state = AppState::with_pool(cfg, pool.clone()).map_err(|e| format!("{e:?}")).expect("state");
    let mut router: Router<AppState> = Router::new();
    for r in pl_mail::routes() {
        router = router.route(r.path, r.handler);
    }
    (router.with_state(state), pool)
}

fn users() -> Vec<(String, String, String)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference/users.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("users.json")).expect("json");
    v["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| {
            (
                u["name"].as_str().unwrap().to_string(),
                u["username"].as_str().unwrap().to_string(),
                u["token"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "braucht Referenz-DB + Django (127.0.0.1:8000)"]
async fn read_routes_match_django() {
    let (router, pool) = setup().await;
    let http = reqwest::Client::new();
    let urls = [
        "/api/mail_accounts/",
        "/api/mail_accounts/?full_perms=true",
        "/api/mail_accounts/?page=2&page_size=1",
        "/api/mail_accounts/?page=9",
        "/api/mail_accounts/1/",
        "/api/mail_accounts/2/",
        "/api/mail_accounts/3/",
        "/api/mail_accounts/abc/",
        "/api/mail_accounts/999999/",
        "/api/mail_rules/",
        "/api/mail_rules/?full_perms=1",
        "/api/mail_rules/1/",
        "/api/mail_rules/2/",
        "/api/mail_rules/3/",
        "/api/processed_mail/",
        "/api/processed_mail/?ordering=-received,id",
        "/api/processed_mail/?ordering=owner",
        "/api/processed_mail/?rule=1",
        "/api/processed_mail/?rule=abc",
        "/api/processed_mail/?rule=999999",
        "/api/processed_mail/?status=SUCCESS",
        "/api/processed_mail/1/",
        "/api/processed_mail/2/?status=NOPE",
    ];
    let mut diffs = Vec::new();
    let mut compared = 0;
    for (name, username, token) in users() {
        if token.is_empty() || name == "inactive" {
            continue;
        }
        let Some(u) = pl_authz::user::load_user_by_username(&pool, &username).await.expect("load user") else {
            continue;
        };
        let mu = MailUser::from_authz(&u).expect("user");
        for version in ["10", "9"] {
            for url in urls {
                let accept = format!("application/json; version={version}");
                let d = http
                    .get(format!("{DJANGO}{url}"))
                    .header("Authorization", format!("Token {token}"))
                    .header("Accept", &accept)
                    .send()
                    .await
                    .expect("django");
                let ds = d.status().as_u16();
                let dct = d.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let db = d.text().await.unwrap_or_default();
                let mut req = Request::builder()
                    .uri(url)
                    .header("host", HOST)
                    .header("accept", &accept)
                    .body(Body::empty())
                    .unwrap();
                req.extensions_mut().insert(mu.clone());
                let r = router.clone().oneshot(req).await.unwrap();
                let rs = r.status().as_u16();
                let rct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let rb = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
                let rb = String::from_utf8_lossy(&rb).into_owned();
                compared += 1;
                if ds != rs || db != rb || dct != rct {
                    diffs.push(format!(
                        "{name} v{version} {url}\n  django {ds} {dct} {db}\n  rust   {rs} {rct} {rb}"
                    ));
                }
            }
        }
    }
    println!("compared {compared}, different {}", diffs.len());
    for d in &diffs {
        println!("{d}");
    }
    assert!(diffs.is_empty(), "{} differences", diffs.len());
}
