//! Document row locks on a database **after data model stage 1** (`documents_document` is a view with an
//! outer join, `paperrust_document` the base table) while the grants refresher has not (yet) seen stage 4
//! (`pl_authz::grants::native() == false`, e.g. right after start). Needs a disposable copy of such a DB
//! whose name starts with `pl_consumer_test`:
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_consumer_test_dm TEMPLATE pl_light_f0"
//! PAPERRUST_CONSUMER_DM_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_consumer_test_dm \
//!   cargo test -p pl-consumer -j 2 --test lock_table_db -- --ignored
//! ```
//! Only locks are taken, every transaction is rolled back.
//!
//! Found 2026-11-28 while checking the folder TLA+ assumption "document permission writers lock the same
//! rows as `move_documents`": `lock_documents_sorted` used `document_row_table()`, which names the view
//! until stage 4 is detected, and PostgreSQL refuses row locks there ("FOR NO KEY UPDATE cannot be
//! applied to the nullable side of an outer join") - bulk edit failed instead of locking.

use sqlx::{Connection, PgConnection};

async fn conn() -> Option<PgConnection> {
    let url = std::env::var("PAPERRUST_CONSUMER_DM_TEST_DATABASE_URL").ok()?;
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_consumer_test"), "refusing to run against database {name}");
    Some(PgConnection::connect(&url).await.expect("connect"))
}

#[tokio::test]
#[ignore = "needs PAPERRUST_CONSUMER_DM_TEST_DATABASE_URL (copy of a stage-1 DB named pl_consumer_test*)"]
async fn document_locks_reach_the_base_table_before_stage4_is_detected() {
    let (Some(mut a), Some(mut b)) = (conn().await, conn().await) else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_DM_TEST_DATABASE_URL not set");
        return;
    };
    pl_authz::grants::set_native(false);
    let kind: String = sqlx::query_scalar("SELECT relkind::text FROM pg_class WHERE oid = 'documents_document'::regclass")
        .fetch_one(&mut a)
        .await
        .expect("relkind");
    assert_eq!(kind, "v", "test DB must be after data model stage 1");
    assert_eq!(pl_authz::grants::document_row_table(), "documents_document", "precondition: stage 4 not detected");

    let id: i64 = sqlx::query_scalar("SELECT id::int8 FROM paperrust_document ORDER BY id LIMIT 1").fetch_one(&mut a).await.expect("doc");
    let mut ta = a.begin().await.expect("begin a");
    assert_eq!(pl_authz::grants::document_lock_table(&mut ta).await.expect("lock table"), "paperrust_document");
    // bulk edit / set_permissions path
    pl_consumer::db::lock_documents_sorted(&mut ta, &[id]).await.expect("lock through the base table, not the view");

    // the folder move's FOR SHARE on the same row must conflict (it would wait; NOWAIT -> 55P03)
    let mut tb = b.begin().await.expect("begin b");
    let err = sqlx::query("SELECT id FROM paperrust_document WHERE id = $1::int4 FOR SHARE NOWAIT")
        .bind(id as i32)
        .execute(&mut *tb)
        .await
        .expect_err("row is locked by the bulk edit");
    assert_eq!(err.as_database_error().and_then(|d| d.code()).as_deref(), Some("55P03"), "{err}");
    tb.rollback().await.expect("rollback b");
    ta.rollback().await.expect("rollback a");

    // media lock path (versions, file moves)
    let mut ta = a.begin().await.expect("begin a");
    pl_consumer::medialock::lock_document_rows(&mut ta, &[id]).await.expect("FOR UPDATE through the base table");
    ta.rollback().await.expect("rollback a");
}
