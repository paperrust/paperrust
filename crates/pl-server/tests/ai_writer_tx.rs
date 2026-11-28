//! `ConsumerDocWriter` transactional path (AI review/automation/undo): the `*_tx` methods write
//! on the caller's connection and never commit themselves, so a rollback leaves neither document
//! changes nor auditlog entries behind; the file move only runs in `after_commit`.
//!
//! The DB test runs against a disposable DB copy whose name starts with `pl_consumer_test`
//! (`PAPERRUST_CONSUMER_TEST_DATABASE_URL`, see crates/pl-consumer/tests/e2e.rs). It only writes
//! inside a transaction that it rolls back. The copy must have the PaperRust migrations applied
//! (tag adjacency list: `get_or_create` does not fill the dropped django-treenode columns), e.g.
//! `CREATE DATABASE pl_consumer_test_migr TEMPLATE pl_fix2_tpl`; a plain `paperless_snapshot` copy
//! fails with `null value in column "tn_ancestors_pks"`.
//!
//! `cargo test -p pl-server -j 2 --test ai_writer_tx -- --ignored`
#![cfg(feature = "ai")]

use std::sync::Arc;

use pl_ai::server::workflow::{DocChanges, DocWriter};
use pl_server::worker::ConsumerDocWriter;
use sqlx::{PgPool, Row};

fn writer(root: &std::path::Path) -> ConsumerDocWriter {
    let s =
        pl_consumer::settings::ConsumerSettings::with_root(root.join("media"), root.join("data"), root.join("tmp"), root.join("consume"));
    ConsumerDocWriter { audit_enabled: true, settings: Arc::new(s) }
}

#[test]
fn consumer_writer_is_transactional() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(writer(dir.path()).supports_transaction());
}

async fn pool() -> Option<PgPool> {
    let url = std::env::var("PAPERRUST_CONSUMER_TEST_DATABASE_URL").ok()?;
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_consumer_test"), "refusing to run against database {name}");
    Some(PgPool::connect(&url).await.expect("connect"))
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await.expect("count")
}

#[tokio::test]
#[ignore = "needs PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB copy pl_consumer_test*)"]
async fn tx_writes_roll_back_completely() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let w = writer(dir.path());
    let doc = sqlx::query("SELECT id, title, filename FROM documents_document WHERE deleted_at IS NULL ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("a document");
    let doc_id = i64::from(doc.get::<i32, _>("id"));
    let title: String = doc.get("title");
    let filename: Option<String> = doc.get("filename");
    let tag: Option<i32> = sqlx::query_scalar("SELECT id FROM documents_tag ORDER BY id LIMIT 1").fetch_optional(&pool).await.expect("tag");
    let field: Option<i32> = sqlx::query_scalar("SELECT id FROM documents_customfield WHERE data_type = 'string' ORDER BY id LIMIT 1")
        .fetch_optional(&pool)
        .await
        .expect("field");
    let log_before = count(&pool, "SELECT count(*)::bigint FROM auditlog_logentry").await;
    let cfi_before = count(&pool, "SELECT count(*)::bigint FROM documents_customfieldinstance").await;
    let tags_before = count(&pool, "SELECT count(*)::bigint FROM documents_document_tags").await;

    let mut tx = pool.begin().await.expect("begin");
    let ch = DocChanges { title: Some(format!("{title} (ai tx test)")), ..Default::default() };
    w.save_tx(&mut tx, doc_id, &ch).await.expect("save_tx");
    if let Some(t) = tag {
        w.add_nested_tags_tx(&mut tx, doc_id, &[i64::from(t)]).await.expect("add_nested_tags_tx");
    }
    if let Some(f) = field {
        w.set_custom_field_tx(&mut tx, doc_id, i64::from(f), "string", &serde_json::json!("ai tx value"))
            .await
            .expect("set_custom_field_tx");
        // wrong type is rejected without touching anything
        assert!(w.set_custom_field_tx(&mut tx, doc_id, i64::from(f), "integer", &serde_json::json!(1)).await.is_err());
    }
    let (_, created) = w.get_or_create_tx(&mut tx, pl_ai::server::data::Cat::Tags, "ai tx test tag", None).await.expect("get_or_create_tx");
    assert!(created);
    let inside: String = sqlx::query_scalar("SELECT title FROM documents_document WHERE id = $1")
        .bind(doc_id as i32)
        .fetch_one(&mut *tx)
        .await
        .expect("title in tx");
    assert_eq!(inside, format!("{title} (ai tx test)"));
    let log_inside: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM auditlog_logentry").fetch_one(&mut *tx).await.expect("count");
    assert!(log_inside > log_before, "auditlog entries are written in the caller's transaction");
    if let Some(f) = field {
        w.remove_custom_field_tx(&mut tx, doc_id, i64::from(f)).await.expect("remove_custom_field_tx");
    }
    tx.rollback().await.expect("rollback");

    let after = sqlx::query("SELECT title, filename FROM documents_document WHERE id = $1")
        .bind(doc_id as i32)
        .fetch_one(&pool)
        .await
        .expect("document");
    assert_eq!(after.get::<String, _>("title"), title);
    assert_eq!(after.get::<Option<String>, _>("filename"), filename);
    assert_eq!(count(&pool, "SELECT count(*)::bigint FROM auditlog_logentry").await, log_before);
    assert_eq!(count(&pool, "SELECT count(*)::bigint FROM documents_customfieldinstance").await, cfi_before);
    assert_eq!(count(&pool, "SELECT count(*)::bigint FROM documents_document_tags").await, tags_before);
    assert_eq!(count(&pool, "SELECT count(*)::bigint FROM documents_tag WHERE name = 'ai tx test tag'").await, 0);
}
