//! Read-only tests against the reference DB (`reference/README.md`, 127.0.0.1:54329).
//!
//! `#[ignore]`: need the running reference PostgreSQL. Run with
//! `PL_MAIL_TEST_DB=postgres://paperless:paperless@127.0.0.1:54329/paperless cargo test -p pl-mail -j 2 -- --ignored`.
//! GET requests only – the reference data is not modified.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use pl_mail::MailUser;
use serde_json::Value;
use tower::ServiceExt;

async fn setup() -> Option<(Router, sqlx::PgPool)> {
    let url = std::env::var("PL_MAIL_TEST_DB").ok()?;
    let pool = match sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await {
        Ok(p) => p,
        Err(e) => panic!("db connect: {e}"),
    };
    let cfg = Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "pl-mail-tests-only-0123456789abcdefghijklmnopqrstuvwxyz"),
        ("PAPERLESS_REDIS", "redis://127.0.0.1:63799"),
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
    Some((router.with_state(state), pool))
}

async fn superuser(pool: &sqlx::PgPool) -> MailUser {
    let (id,): (i32,) = sqlx::query_as("SELECT id FROM auth_user WHERE is_superuser AND is_active ORDER BY id LIMIT 1")
        .fetch_one(pool)
        .await
        .expect("superuser in reference db");
    MailUser { id, is_active: true, is_superuser: true, perms: None }
}

async fn get(router: &Router, user: &MailUser, uri: &str) -> (StatusCode, Value) {
    let mut req = Request::builder().uri(uri).header("host", "localhost:8000").body(Body::empty()).unwrap();
    req.extensions_mut().insert(user.clone());
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PL_MAIL_TEST_DB)"]
async fn lists_have_drf_shape_and_masked_passwords() {
    let Some((router, pool)) = setup().await else { panic!("PL_MAIL_TEST_DB not reachable") };
    let su = superuser(&pool).await;
    let (s, v) = get(&router, &su, "/api/mail_accounts/").await;
    assert_eq!(s, StatusCode::OK);
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["count", "next", "previous", "results"]);
    for acc in v["results"].as_array().unwrap() {
        let pw = acc["password"].as_str().unwrap();
        assert!(pw.len() >= 10 && pw.chars().all(|c| c == '*'));
        let k: Vec<&String> = acc.as_object().unwrap().keys().collect();
        assert_eq!(
            k,
            [
                "id",
                "name",
                "imap_server",
                "imap_port",
                "imap_security",
                "username",
                "password",
                "character_set",
                "is_token",
                "owner",
                "user_can_change",
                "account_type",
                "expiration"
            ]
        );
    }
    let (s, _) = get(&router, &su, "/api/mail_rules/?full_perms=true").await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = get(&router, &su, "/api/processed_mail/?rule=abc").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["rule"][0], "Select a valid choice. That choice is not one of the available choices.");
    let (s, v) = get(&router, &su, "/api/mail_accounts/abc/").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["detail"], "Not found.");
    let (s, v) = get(&router, &su, "/api/mail_rules/999999/").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["detail"], "No MailRule matches the given query.");
}

/// Review 2 M3: deleting a rule/account that a workflow trigger points to does not violate an FK
/// (Django: `WorkflowTrigger.filter_mailrule` `on_delete=SET_NULL`). Runs entirely in one transaction
/// that is rolled back at the end – the reference data stays unchanged.
#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PL_MAIL_TEST_DB)"]
async fn destroy_sets_workflow_trigger_filter_null() {
    let Some((_router, pool)) = setup().await else { panic!("PL_MAIL_TEST_DB not reachable") };
    for account in [false, true] {
        let mut tx = pool.begin().await.unwrap();
        let (rule_id, account_id): (i32, i32) = sqlx::query_as("SELECT id, account_id FROM paperless_mail_mailrule ORDER BY id LIMIT 1")
            .fetch_one(&mut *tx)
            .await
            .expect("mail rule in reference db");
        let (trigger_id,): (i32,) = sqlx::query_as("SELECT id FROM documents_workflowtrigger ORDER BY id LIMIT 1")
            .fetch_one(&mut *tx)
            .await
            .expect("workflow trigger in reference db");
        sqlx::query("UPDATE documents_workflowtrigger SET filter_mailrule_id = $1 WHERE id = $2")
            .bind(rule_id)
            .bind(trigger_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        if account {
            pl_mail::accounts::delete_account_cascade(&mut tx, account_id).await.unwrap();
        } else {
            pl_mail::rules::delete_rule_cascade(&mut tx, rule_id).await.unwrap();
        }
        // FKs are DEFERRABLE INITIALLY DEFERRED → check immediately here instead of at commit.
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE").execute(&mut *tx).await.expect("no FK violation");
        let (filter,): (Option<i32>,) = sqlx::query_as("SELECT filter_mailrule_id FROM documents_workflowtrigger WHERE id = $1")
            .bind(trigger_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(filter, None);
        tx.rollback().await.unwrap();
    }
}
