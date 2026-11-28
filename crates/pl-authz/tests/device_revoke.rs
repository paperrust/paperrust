//! Security review 2026-11-19 F13: `device::revoke_all_for_user` revokes all active
//! device tokens of exactly one user (called after password reset/change in pl-accounts and
//! pl-auth-api).
//!
//! Own throwaway database `pl_authz_device_revoke_test` on the reference PostgreSQL (only a
//! minimal table `auth_user` + migration `pl_authz.0001`), never `paperless`; deleted at the end.
//!
//! ```text
//! cargo test -p pl-authz -j 2 --test device_revoke -- --ignored
//! ```

use pl_authz::device;
use sqlx::PgPool;

const DB: &str = "pl_authz_device_revoke_test";
const DB_TX: &str = "pl_authz_device_revoke_tx_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn admin() -> PgPool {
    sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect")
}

async fn drop_db_named(name: &str) {
    let a = admin().await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&a).await;
}

async fn drop_db() {
    drop_db_named(DB).await;
}

async fn fresh_db() -> PgPool {
    fresh_db_named(DB).await
}

async fn fresh_db_named(name: &str) -> PgPool {
    drop_db_named(name).await;
    let a = admin().await;
    sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE template0")).execute(&a).await.expect("create");
    let url = format!("postgres://paperless:paperless@127.0.0.1:54329/{name}");
    sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.expect("db")
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn password_change_revokes_device_tokens_of_that_user_only() {
    let db = fresh_db().await;
    // without the table: nothing to revoke, no error
    assert_eq!(device::revoke_all_for_user(&db, 1).await.expect("no table"), 0);

    sqlx::raw_sql(
        "CREATE TABLE auth_user (id serial PRIMARY KEY); INSERT INTO auth_user DEFAULT VALUES; INSERT INTO auth_user DEFAULT VALUES;",
    )
    .execute(&db)
    .await
    .expect("auth_user");
    device::migrations::apply(&db).await.expect("migration");

    let (_, a1) = device::create(&db, 1, "phone").await.expect("a1");
    let (_, a2) = device::create(&db, 1, "laptop").await.expect("a2");
    let (_, b1) = device::create(&db, 2, "other").await.expect("b1");
    assert_eq!(device::lookup_user_id(&db, &a1).await.expect("lookup"), Some(1));

    assert_eq!(device::revoke_all_for_user(&db, 1).await.expect("revoke"), 2);
    assert_eq!(device::lookup_user_id(&db, &a1).await.expect("lookup"), None);
    assert_eq!(device::lookup_user_id(&db, &a2).await.expect("lookup"), None);
    assert!(device::list(&db, 1).await.expect("list").is_empty());
    assert_eq!(device::lookup_user_id(&db, &b1).await.expect("lookup"), Some(2), "other users keep their tokens");
    // second call: nothing active any more
    assert_eq!(device::revoke_all_for_user(&db, 1).await.expect("revoke"), 0);

    db.close().await;
    drop_db().await;
}

/// `revoke_all_for_user_tx` runs inside the caller's transaction: a rollback undoes the revocation,
/// and a missing table neither errors nor aborts the outer transaction (savepoint).
#[tokio::test]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn tx_variant_follows_the_callers_transaction() {
    let db = fresh_db_named(DB_TX).await;
    sqlx::raw_sql("CREATE TABLE auth_user (id serial PRIMARY KEY); INSERT INTO auth_user DEFAULT VALUES;")
        .execute(&db)
        .await
        .expect("auth_user");

    // missing table: Ok(0), the outer transaction stays usable
    let mut tx = db.begin().await.expect("begin");
    assert_eq!(device::revoke_all_for_user_tx(&mut tx, 1).await.expect("no table"), 0);
    let one: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&mut *tx).await.expect("tx still usable");
    assert_eq!(one, 1);
    tx.commit().await.expect("commit");

    device::migrations::apply(&db).await.expect("migration");
    let (_, a1) = device::create(&db, 1, "phone").await.expect("a1");

    // rollback: the token stays valid
    let mut tx = db.begin().await.expect("begin");
    assert_eq!(device::revoke_all_for_user_tx(&mut tx, 1).await.expect("revoke"), 1);
    tx.rollback().await.expect("rollback");
    assert_eq!(device::lookup_user_id(&db, &a1).await.expect("lookup"), Some(1));

    // commit: revoked
    let mut tx = db.begin().await.expect("begin");
    assert_eq!(device::revoke_all_for_user_tx(&mut tx, 1).await.expect("revoke"), 1);
    tx.commit().await.expect("commit");
    assert_eq!(device::lookup_user_id(&db, &a1).await.expect("lookup"), None);

    db.close().await;
    drop_db_named(DB_TX).await;
}
