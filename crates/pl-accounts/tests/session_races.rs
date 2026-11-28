//! Regression tests for the external review (2026-11-27), `Session::finish`:
//!
//! 1. Logout race: request A loaded a session, request B logged out (row deleted), A then
//!    modifies and saves its copy → `SessionError::Interrupted` (Django `UpdateError` →
//!    `SessionInterrupted`, 400), the old key is **not** re-created with the old login data.
//! 3. "Until the browser closes" (`set_expiry(0)`): session cookie without `Max-Age`, but the DB
//!    row keeps `SESSION_COOKIE_AGE` (Django `get_expiry_age`), so the next request still finds it.
//!
//! Uses its own throw-away database `pl_accounts_session_race_test` (from `template0`, only
//! `django_session`); never `paperless`. Dropped at the end.
//!
//! ```text
//! cargo test -p pl-accounts -j 2 --test session_races -- --ignored
//! ```

use chrono::{Duration, Utc};
use pl_accounts::cache::Cache;
use pl_accounts::session::{Session, SessionError};
use pl_accounts::settings::Settings;
use pl_core::config::Env;
use pl_core::{AppState, Config};
use serde_json::Value;
use sqlx::PgPool;

const DB: &str = "pl_accounts_session_race_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

#[tokio::test]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn logout_race_and_browser_close_expiry() {
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE template0")).execute(&admin).await.expect("create");
    let cfg = Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", DB),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_SECRET_KEY", "unit-test-only-secret-key-0123456789abcdef"),
    ]))
    .expect("config");
    let state = AppState::new(cfg).expect("state");
    let db: PgPool = state.db().clone();
    sqlx::raw_sql(
        "CREATE TABLE django_session (session_key varchar(40) PRIMARY KEY, session_data text NOT NULL, \
         expire_date timestamptz NOT NULL);",
    )
    .execute(&db)
    .await
    .expect("schema");
    let cache = Cache::backend_only(&state);
    let mut s = Settings::from_core(state.config());
    s.session_cookie_age = 1_209_600;
    s.session_expire_at_browser_close = false;

    // Login with "remember me" off: set_expiry(0).
    let mut login = Session::load(&db, &s, None).await.expect("load");
    login.set("_auth_user_id", Value::from("1"));
    login.set_expiry(0);
    let cookie = login.finish(&db, &cache, &s, 302).await.expect("finish").expect("cookie");
    assert!(!cookie.contains("Max-Age"), "browser-session cookie: {cookie}");
    let key = login.session_key().expect("key").to_string();
    let expire: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT expire_date FROM django_session WHERE session_key = $1").bind(&key).fetch_one(&db).await.expect("row");
    assert!(expire > Utc::now() + Duration::days(13), "server-side expiry must be SESSION_COOKIE_AGE, got {expire}");
    // the next request still finds the session
    let next = Session::load(&db, &s, Some(&key)).await.expect("load");
    assert_eq!(next.user_id(), Some(1));

    // Logout race: A loads, B logs out, A saves.
    let mut a = Session::load(&db, &s, Some(&key)).await.expect("load a");
    let mut b = Session::load(&db, &s, Some(&key)).await.expect("load b");
    b.flush();
    b.finish(&db, &cache, &s, 302).await.expect("logout");
    a.set("some_flag", Value::Bool(true));
    let r = a.finish(&db, &cache, &s, 200).await;
    assert!(matches!(r, Err(SessionError::Interrupted)), "{r:?}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM django_session").fetch_one(&db).await.expect("count");
    assert_eq!(n, 0, "the logged-out session must not be resurrected");

    db.close().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
}
