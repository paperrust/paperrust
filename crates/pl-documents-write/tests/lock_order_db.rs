//! Lock order MEDIA_LOCK ↔ document row (tests/stress/BEFUNDE.md B1) and recognizing an
//! already stored ingestion task (B7) against throwaway copies of a migrated test DB (never
//! `paperless`, `paperless_snapshot`, `pl_scale_1m`).
//!
//! Preparation (once): template `pl_fix1_tpl` = migrated copy of `paperless_snapshot`
//! (data model stage 5 with sync triggers, queue table). Each test creates its own
//! DB `pl_fix1_<test>` from it and drops it at the end.
//!
//! ```sh
//! source reference/env.sh
//! PAPERRUST_FIX1_SERVER=postgres://paperless:paperless@127.0.0.1:54329 \
//!   cargo test -p pl-documents-write -j 2 --test lock_order_db -- --ignored --test-threads 1 --nocapture
//! ```

use std::time::Duration;

use pl_documents_write::files::{MediaCfg, MediaLock};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

const TEMPLATE: &str = "pl_fix1_tpl";

fn server() -> Option<String> {
    let s = std::env::var("PAPERRUST_FIX1_SERVER").ok()?;
    assert!(s.contains("127.0.0.1") || s.contains("localhost"), "nur lokale Server");
    Some(s)
}

async fn fresh_db(server: &str, name: &str, max_conn: u32) -> PgPool {
    assert!(name.starts_with("pl_fix1_") && name != TEMPLATE);
    pl_authz::grants::set_native(true);
    let admin = PgPool::connect(&format!("{server}/postgres")).await.expect("admin");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE {TEMPLATE}")).execute(&admin).await.expect("create");
    admin.close().await;
    PgPoolOptions::new().max_connections(max_conn).connect(&format!("{server}/{name}")).await.expect("connect")
}

async fn drop_db(server: &str, pool: PgPool, name: &str) {
    pool.close().await;
    let admin = PgPool::connect(&format!("{server}/postgres")).await.expect("admin");
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&admin).await;
    admin.close().await;
}

fn media(root: &std::path::Path) -> MediaCfg {
    MediaCfg {
        originals_dir: root.join("documents/originals"),
        archive_dir: root.join("documents/archive"),
        thumbnail_dir: root.join("documents/thumbnails"),
        media_lock: root.join("media.lock"),
        empty_trash_dir: None,
        opts: Default::default(),
    }
}

async fn doc_with_filename(pool: &PgPool) -> i64 {
    let id: i32 = sqlx::query_scalar(
        "SELECT id FROM documents_document WHERE deleted_at IS NULL AND root_document_id IS NULL \
         AND coalesce(filename, '') <> '' ORDER BY id LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .expect("doc");
    i64::from(id)
}

/// B1: transaction A holds the document row (like ingesting a new version:
/// `create_version` → `SELECT … FOR UPDATE`) and then wants MEDIA_LOCK. At the same time B
/// (PATCH → `post_save` → `update_filename_and_move_files`) updates the file names. Previously B
/// first took MEDIA_LOCK and then waited for the row → A waited forever for MEDIA_LOCK (PostgreSQL sees
/// only "idle in transaction", no deadlock). Now B locks the row first (waits for A, without
/// holding MEDIA_LOCK), A gets MEDIA_LOCK, commits, then B runs through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "braucht PAPERRUST_FIX1_SERVER und die Vorlage pl_fix1_tpl (siehe Modulkopf)"]
async fn b1_row_then_media_lock_no_deadlock() {
    let Some(server) = server() else { return };
    let name = "pl_fix1_b1";
    let pool = fresh_db(&server, name, 4).await;
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = media(tmp.path());
    let id = doc_with_filename(&pool).await;

    for caller_in_tx in [false, true] {
        // A: lock the row (like create_version)
        let mut a = pool.begin().await.expect("begin a");
        sqlx::query("SELECT id FROM paperrust_document WHERE id = $1 FOR UPDATE").bind(id as i32).fetch_one(&mut *a).await.expect("lock");

        // B: update file names – without a transaction (PATCH after the commit) or in a
        // transaction of the caller (batch deletion → docs::save with media)
        let (p, c) = (pool.clone(), cfg.clone());
        let b = tokio::spawn(async move {
            let mut conn = p.acquire().await.expect("conn b");
            if caller_in_tx {
                let mut tx = sqlx::Connection::begin(&mut *conn).await.expect("begin b");
                pl_documents_write::files::update_filename_and_move_files(&mut tx, &c, id).await.expect("move b");
                tx.commit().await.expect("commit b");
            } else {
                pl_documents_write::files::update_filename_and_move_files(&mut conn, &c, id).await.expect("move b");
            }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;

        // A wants MEDIA_LOCK while it holds the row
        let got = MediaLock::acquire_with(&cfg.media_lock, Some(Duration::from_secs(5))).await;
        assert!(
            got.is_ok(),
            "Verklemmung (caller_in_tx={caller_in_tx}): A hält die Zeile und wartet auf MEDIA_LOCK, B hält MEDIA_LOCK und wartet auf die Zeile: {:?}",
            got.err()
        );
        drop(got);
        a.commit().await.expect("commit a");
        tokio::time::timeout(Duration::from_secs(10), b).await.expect("B endet").expect("B ohne Panik");
    }
    drop_db(&server, pool, name).await;
}

/// B7: The ingestion transaction records the created document on the task; a redelivery
/// finds it (even after the working file has been deleted). Without a record, or if the document is
/// permanently deleted: nothing.
#[tokio::test]
#[ignore = "braucht PAPERRUST_FIX1_SERVER und die Vorlage pl_fix1_tpl (siehe Modulkopf)"]
async fn b7_task_document_marker() {
    let Some(server) = server() else { return };
    let name = "pl_fix1_b7";
    let pool = fresh_db(&server, name, 2).await;
    let id = doc_with_filename(&pool).await;
    let task = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO paperrust_task_queue (task_id, task_name, state) VALUES ($1, 'documents.tasks.consume_file', 'running')")
        .bind(&task)
        .execute(&pool)
        .await
        .expect("task");
    assert_eq!(pl_consumer::consumer::task_document(&pool, &task).await.expect("lookup"), None, "ohne Vermerk");
    let mut tx = pool.begin().await.expect("begin");
    pl_consumer::consumer::record_task_document(&mut tx, &task, id).await.expect("record");
    // invisible before the commit
    assert_eq!(pl_consumer::consumer::task_document(&pool, &task).await.expect("lookup"), None, "vor dem Commit");
    tx.commit().await.expect("commit");
    assert_eq!(pl_consumer::consumer::task_document(&pool, &task).await.expect("lookup"), Some(id));
    // existing header rows are preserved
    let h: serde_json::Value =
        sqlx::query_scalar("SELECT headers FROM paperrust_task_queue WHERE task_id = $1").bind(&task).fetch_one(&pool).await.expect("h");
    assert_eq!(h[pl_consumer::consumer::TASK_DOCUMENT_HEADER], serde_json::json!(id));
    // unknown task
    assert_eq!(pl_consumer::consumer::task_document(&pool, "no-such-task").await.expect("lookup"), None);
    drop_db(&server, pool, name).await;
}
