//! upstream fix a139bdff4 (#14722, after v3.4.2): `clear_expired_sessions` removes only expired
//! sessions, in batches of 500. Mirrors
//! `documents/tests/test_tasks.py::TestClearExpiredSessions::test_clears_only_expired_sessions_in_batches`
//! (501 expired + 1 valid → only "valid" left, exactly two DELETEs).
//!
//! Uses its own throw-away database `pl_upsync_sessions_test` (from `template0`, only
//! `django_session`); never `paperless`. Dropped at the end.
//!
//! ```text
//! cargo test -p pl-accounts -j 2 --test clear_sessions -- --ignored
//! ```

use chrono::{Duration, Utc};
use sqlx::PgPool;

const DB: &str = "pl_upsync_sessions_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn admin() -> PgPool {
    sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect")
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn clears_only_expired_sessions_in_batches() {
    let a = admin().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&a).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE template0")).execute(&a).await.expect("create");
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}"))
        .await
        .expect("db");
    // same definition as Django's sessions.0001_initial
    sqlx::raw_sql(
        "CREATE TABLE django_session (session_key varchar(40) PRIMARY KEY, session_data text NOT NULL, \
         expire_date timestamptz NOT NULL);
         CREATE INDEX django_session_expire_date_a5c62663 ON django_session (expire_date);",
    )
    .execute(&db)
    .await
    .expect("schema");

    let now = Utc::now();
    sqlx::query(
        "INSERT INTO django_session (session_key, session_data, expire_date) \
         SELECT 'expired-' || i, '', $1 FROM generate_series(0, 500) AS i",
    )
    .bind(now - Duration::days(1))
    .execute(&db)
    .await
    .expect("insert expired");
    sqlx::query("INSERT INTO django_session (session_key, session_data, expire_date) VALUES ('valid', '', $1)")
        .bind(now + Duration::days(1))
        .execute(&db)
        .await
        .expect("insert valid");

    let r = pl_accounts::session::clear_expired_sessions(&db, Utc::now()).await.expect("clear");
    assert_eq!(r.deleted, 501);
    assert_eq!(r.batches, 2, "two DELETE statements like upstream");
    let left: Vec<(String,)> = sqlx::query_as("SELECT session_key FROM django_session").fetch_all(&db).await.unwrap();
    assert_eq!(left, vec![("valid".to_string(),)]);

    // nothing expired → one empty pass
    let r = pl_accounts::session::clear_expired_sessions(&db, Utc::now()).await.expect("clear again");
    assert_eq!((r.deleted, r.batches), (0, 1));

    db.close().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&a).await.expect("drop");
}
