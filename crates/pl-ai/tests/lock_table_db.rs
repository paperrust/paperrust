#![cfg(feature = "server")]
//! `workflow::load_state_conn(.., lock = true)` (AI review apply in one transaction) on a database
//! **after data model stage 1**, where `documents_document` is a view with an outer join. Needs a
//! disposable copy of such a DB whose name starts with `pl_ai_test`:
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_ai_test_dm TEMPLATE pl_light_f0"
//! PAPERRUST_AI_DM_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_ai_test_dm \
//!   cargo test -p pl-ai --features server -j 2 --test lock_table_db -- --ignored
//! ```
//! Found 2026-11-28: the state was read with `… FROM documents_document d … FOR UPDATE OF d`, which
//! PostgreSQL refuses on that view ("cannot be applied to the nullable side of an outer join"), so
//! applying a reviewed AI suggestion failed. Now the base row is locked first.

use sqlx::{Connection, PgConnection};

#[tokio::test]
#[ignore = "needs PAPERRUST_AI_DM_TEST_DATABASE_URL (copy of a stage-1 DB named pl_ai_test*)"]
async fn locked_state_read_works_on_the_stage1_view() {
    let Ok(url) = std::env::var("PAPERRUST_AI_DM_TEST_DATABASE_URL") else {
        eprintln!("SKIP: PAPERRUST_AI_DM_TEST_DATABASE_URL not set");
        return;
    };
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_ai_test"), "refusing to run against database {name}");
    let mut a = PgConnection::connect(&url).await.expect("connect a");
    let mut b = PgConnection::connect(&url).await.expect("connect b");
    pl_authz::grants::set_native(false);
    let kind: String = sqlx::query_scalar("SELECT relkind::text FROM pg_class WHERE oid = 'documents_document'::regclass")
        .fetch_one(&mut a)
        .await
        .expect("relkind");
    assert_eq!(kind, "v", "test DB must be after data model stage 1");
    let id: i64 = sqlx::query_scalar("SELECT id::int8 FROM paperrust_document WHERE deleted_at IS NULL ORDER BY id LIMIT 1")
        .fetch_one(&mut a)
        .await
        .expect("doc");

    let mut ta = a.begin().await.expect("begin a");
    let state = pl_ai::server::workflow::load_state_conn(&mut ta, id, true).await.expect("locked read on the stage-1 view");
    assert!(state.is_some(), "document {id} found");
    // the row really is locked: another writer cannot take it
    let mut tb = b.begin().await.expect("begin b");
    let err = sqlx::query("SELECT id FROM paperrust_document WHERE id = $1::int4 FOR UPDATE NOWAIT")
        .bind(id as i32)
        .execute(&mut *tb)
        .await
        .expect_err("row is locked");
    assert_eq!(err.as_database_error().and_then(|d| d.code()).as_deref(), Some("55P03"), "{err}");
    tb.rollback().await.expect("rollback b");
    ta.rollback().await.expect("rollback a");
}
