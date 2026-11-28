//! Truncation system note (`system_note::set_truncation` / `clear_truncation`) against a disposable
//! DB copy whose name starts with `pl_consumer_test` (`PAPERRUST_CONSUMER_TEST_DATABASE_URL`, see
//! `tests/e2e.rs`). Everything runs inside one transaction that is rolled back.
//!
//! `cargo test -p pl-consumer -j 2 --test system_note_db -- --ignored`

use pl_consumer::system_note::{self, TRUNCATION_MARKER};
use sqlx::{Connection, PgConnection};

async fn conn() -> Option<PgConnection> {
    let url = std::env::var("PAPERRUST_CONSUMER_TEST_DATABASE_URL").ok()?;
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_consumer_test"), "refusing to run against database {name}");
    Some(PgConnection::connect(&url).await.expect("connect"))
}

/// (id, note, deleted) of the system truncation notes of `doc`, oldest first.
async fn notes(c: &mut PgConnection, doc: i32) -> Vec<(i32, String, bool)> {
    sqlx::query_as(
        "SELECT id, note, deleted_at IS NOT NULL FROM documents_note \
         WHERE document_id = $1 AND user_id IS NULL AND note LIKE $2 ORDER BY id",
    )
    .bind(doc)
    .bind(format!("{TRUNCATION_MARKER}:%"))
    .fetch_all(&mut *c)
    .await
    .expect("notes")
}

#[tokio::test]
#[ignore = "needs PAPERRUST_CONSUMER_TEST_DATABASE_URL (DB copy pl_consumer_test*)"]
async fn truncation_note_set_update_clear() {
    let Some(mut c) = conn().await else {
        eprintln!("SKIP: PAPERRUST_CONSUMER_TEST_DATABASE_URL not set");
        return;
    };
    let mut tx = c.begin().await.expect("begin");
    let doc: i32 = sqlx::query_scalar("SELECT id FROM documents_document WHERE deleted_at IS NULL ORDER BY id LIMIT 1")
        .fetch_one(&mut *tx)
        .await
        .expect("a document");
    let user_notes_before: i64 = sqlx::query_scalar("SELECT count(*) FROM documents_note WHERE document_id = $1 AND user_id IS NOT NULL")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await
        .expect("count");
    assert!(notes(&mut tx, doc).await.is_empty(), "no truncation note at start");

    // first set inserts, second set updates the same row (no duplicate)
    system_note::set_truncation(&mut tx, i64::from(doc), "page limit (2)").await.expect("set 1");
    let n1 = notes(&mut tx, doc).await;
    assert_eq!(n1.len(), 1, "{n1:?}");
    assert_eq!(n1[0].1, system_note::truncation_note("page limit (2)"));
    system_note::set_truncation(&mut tx, i64::from(doc), "time limit").await.expect("set 2");
    let n2 = notes(&mut tx, doc).await;
    assert_eq!(n2.len(), 1, "updated, not duplicated: {n2:?}");
    assert_eq!(n2[0].0, n1[0].0);
    assert_eq!(n2[0].1, system_note::truncation_note("time limit"));
    assert!(!n2[0].2);

    // clear soft-deletes it; a later set creates a fresh note
    system_note::clear_truncation(&mut tx, i64::from(doc)).await.expect("clear");
    let n3 = notes(&mut tx, doc).await;
    assert_eq!(n3.len(), 1);
    assert!(n3[0].2, "soft-deleted: {n3:?}");
    system_note::clear_truncation(&mut tx, i64::from(doc)).await.expect("clear is idempotent");

    // with the PaperRust `format` column present the insert fills it (deferred FK checks first,
    // ALTER TABLE refuses to run with pending trigger events)
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE").execute(&mut *tx).await.expect("immediate");
    sqlx::query("ALTER TABLE documents_note ADD COLUMN IF NOT EXISTS format varchar(16) NOT NULL DEFAULT 'plain'")
        .execute(&mut *tx)
        .await
        .expect("format column");
    system_note::set_truncation(&mut tx, i64::from(doc), "text size").await.expect("set 3");
    let n4 = notes(&mut tx, doc).await;
    assert_eq!(n4.len(), 2, "{n4:?}");
    assert!(!n4[1].2 && n4[1].0 != n3[0].0);
    let fmt: String =
        sqlx::query_scalar("SELECT format FROM documents_note WHERE id = $1").bind(n4[1].0).fetch_one(&mut *tx).await.expect("format");
    assert_eq!(fmt, "plain");

    // user notes are never touched
    let user_notes_after: i64 = sqlx::query_scalar("SELECT count(*) FROM documents_note WHERE document_id = $1 AND user_id IS NOT NULL")
        .bind(doc)
        .fetch_one(&mut *tx)
        .await
        .expect("count");
    assert_eq!(user_notes_after, user_notes_before);
    tx.rollback().await.expect("rollback");
}
