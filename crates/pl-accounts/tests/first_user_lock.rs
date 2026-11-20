//! upstream fix 22746aa20 (#14688, after v3.4.2): concurrent first signups create only one
//! superuser (`CustomAccountAdapter.save_user`: `@transaction.atomic` +
//! `ContentType.objects.select_for_update()`). Mirrors
//! `test_concurrent_bootstrap_signups_create_only_one_superuser`.
//!
//! Uses its own throw-away database `pl_upsync_accounts_test` (from `template0`, only the three
//! tables the check touches); never `paperless`. Dropped at the end.
//!
//! ```text
//! cargo test -p pl-accounts -j 2 --test first_user_lock -- --ignored
//! ```

use std::time::Duration;

use sqlx::PgPool;

const DB: &str = "pl_upsync_accounts_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn admin() -> PgPool {
    sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect")
}

async fn signup(db: PgPool, name: String) -> bool {
    let mut tx = db.begin().await.expect("begin");
    let first = pl_accounts::page::first_install_locked(&mut tx).await.expect("first install");
    // widen the gap between the bootstrap decision and the insert (like the upstream test)
    tokio::time::sleep(Duration::from_millis(100)).await;
    sqlx::query("INSERT INTO auth_user (username, is_superuser, is_staff) VALUES ($1, $2, $2)")
        .bind(name)
        .bind(first)
        .execute(&mut *tx)
        .await
        .expect("insert");
    tx.commit().await.expect("commit");
    first
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn concurrent_bootstrap_signups_create_only_one_superuser() {
    let a = admin().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&a).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE template0")).execute(&a).await.expect("create");
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}"))
        .await
        .expect("db");
    sqlx::raw_sql(
        "CREATE TABLE django_content_type (id serial PRIMARY KEY, app_label text NOT NULL, model text NOT NULL);
         INSERT INTO django_content_type (app_label, model) VALUES ('auth', 'user');
         CREATE TABLE auth_user (id serial PRIMARY KEY, username text NOT NULL UNIQUE, is_superuser boolean NOT NULL, is_staff boolean NOT NULL);
         INSERT INTO auth_user (username, is_superuser, is_staff) VALUES ('AnonymousUser', false, false), ('consumer', false, false);
         CREATE TABLE documents_document (id serial PRIMARY KEY);",
    )
    .execute(&db)
    .await
    .expect("schema");

    let (r1, r2) = tokio::join!(signup(db.clone(), "signup0".into()), signup(db.clone(), "signup1".into()));
    assert!(r1 ^ r2, "exactly one signup must become superuser ({r1}, {r2})");
    let (n, su, staff): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE username LIKE 'signup%'), count(*) FILTER (WHERE is_superuser), count(*) FILTER (WHERE is_staff) FROM auth_user",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!((n, su, staff), (2, 1, 1));

    // a later signup is no longer a first install
    assert!(!signup(db.clone(), "signup2".into()).await);

    db.close().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&a).await.expect("drop");
}
