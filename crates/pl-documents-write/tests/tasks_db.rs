//! Worker tasks against the W13 copy `pl_w13_ref` (never against the reference DB `paperless`):
//! `restore_archive_serial_numbers_task`, `bulk_edit.delete`, `empty_trash`.
//!
//! ```sh
//! source tests/parity/pl-documents-write/w13/w13env.sh
//! cargo test -p pl-documents-write -j 1 --test tasks_db -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use serde_json::{Map, Value, json};

fn reset() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity/pl-documents-write/w13/reset.sh");
    let st = std::process::Command::new("bash").arg(script).stdout(std::process::Stdio::null()).status().expect("reset.sh");
    assert!(st.success(), "reset failed");
}

async fn row(state: &pl_core::AppState, id: i32) -> Option<(Option<i32>, bool)> {
    sqlx::query_as("SELECT archive_serial_number, deleted_at IS NOT NULL FROM documents_document WHERE id = $1")
        .bind(id)
        .fetch_optional(state.db())
        .await
        .expect("query")
}

#[tokio::test]
#[ignore = "braucht die W13-Kopie (source tests/parity/pl-documents-write/w13/w13env.sh)"]
async fn worker_tasks_on_reference_copy() {
    assert_eq!(std::env::var("PAPERLESS_DBNAME").as_deref(), Ok("pl_w13_ref"), "source w13env.sh first");
    reset();
    let state = pl_core::AppState::from_env().await.expect("state");

    // restore: direct call with backup, errback form (request first) fails like upstream
    sqlx::query("UPDATE documents_document SET archive_serial_number = NULL WHERE id IN (8, 10)").execute(state.db()).await.unwrap();
    pl_documents_write::tasks::restore_asn_task(state.db(), &[json!({"8": 1008, "10": null})], &Map::new()).await.expect("restore");
    assert_eq!(row(&state, 8).await, Some((Some(1008), false)));
    assert_eq!(row(&state, 10).await, Some((None, false)));
    let req = json!({"id": "x", "task": "documents.tasks.consume_file", "args": [], "kwargs": {}, "group": null, "retries": 0});
    let e = pl_documents_write::tasks::restore_asn_task(
        state.db(),
        &[req, json!({"exc_type": "X"}), Value::Null, json!({"10": 1010})],
        &Map::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.0, "AttributeError");
    assert_eq!(row(&state, 10).await, Some((None, false)));

    // bulk_edit.delete: root 1 with versions 201, 202 into the trash
    let deleted = pl_documents_write::tasks::bulk_delete(&state, &[1, 8]).await;
    assert_eq!(deleted, vec![1, 8, 201, 202]);
    for id in [1, 8, 201, 202] {
        assert_eq!(row(&state, id).await.map(|r| r.1), Some(true), "doc {id} in trash");
    }

    // empty_trash(doc_ids=[8]) → only 8 hard-deleted; non-deleted ids are ignored
    let n = pl_documents_write::tasks::empty_trash(&state, Some(&[8, 9])).await;
    assert_eq!(n, 1);
    assert_eq!(row(&state, 8).await, None);
    assert!(row(&state, 9).await.is_some());

    // empty_trash() → only expired ones (EMPTY_TRASH_DELAY days); freshly deleted ones remain
    sqlx::query("UPDATE documents_document SET deleted_at = now() - interval '40 days' WHERE id = 196").execute(state.db()).await.unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM documents_document WHERE deleted_at IS NOT NULL").fetch_one(state.db()).await.unwrap();
    let expired: i64 = sqlx::query_scalar("SELECT count(*) FROM documents_document WHERE deleted_at < now() - interval '30 days'")
        .fetch_one(state.db())
        .await
        .unwrap();
    let n = pl_documents_write::tasks::empty_trash(&state, None).await;
    assert_eq!(n as i64, expired);
    assert!(expired >= 1);
    assert_eq!(row(&state, 196).await, None);
    assert_eq!(row(&state, 1).await.map(|r| r.1), Some(true), "fresh trash stays");
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM documents_document WHERE deleted_at IS NOT NULL").fetch_one(state.db()).await.unwrap();
    assert_eq!(after, before - expired);
    reset();
}
