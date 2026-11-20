//! Concurrency of the bulk changes (tests/stress/BEFUNDE.md B3, B4, B5, B6) against
//! throwaway copies of a migrated test DB (never `paperless`, `paperless_snapshot`, `pl_scale_1m`).
//!
//! Preparation (once): template `pl_fix2_tpl` = `paperless_snapshot` + `paperrust manage migrate`
//! (data model stage 5 with sync triggers). Each test creates its own DB `pl_fix2_<test>` from it
//! and drops it at the end.
//!
//! ```sh
//! source reference/env.sh
//! PAPERRUST_FIX2_SERVER=postgres://paperless:paperless@127.0.0.1:54329 \
//!   cargo test -p pl-documents-write -j 2 --test concurrency_db -- --ignored --test-threads 1 --nocapture
//! ```

use std::time::Duration;

use pl_documents_write::bulk_edit::{self, ExecCtx, Params};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgConnection, PgPool};

const TEMPLATE: &str = "pl_fix2_tpl";

fn server() -> Option<String> {
    let s = std::env::var("PAPERRUST_FIX2_SERVER").ok()?;
    assert!(s.contains("127.0.0.1") || s.contains("localhost"), "nur lokale Server");
    Some(s)
}

async fn fresh_db(server: &str, name: &str, max_conn: u32) -> PgPool {
    assert!(name.starts_with("pl_fix2_") && name != TEMPLATE);
    // Template is migrated (data model stage 5): row table paperrust_document as in the server
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

fn ctx() -> ExecCtx {
    let env = pl_core::config::Env::from_pairs([("PAPERRUST_SECRET_KEY", "fix2-test-secret-key-local-only-0123456789")]);
    let cfg = pl_core::Config::from_env_map(env).expect("config");
    ExecCtx {
        log: pl_documents_write::auditlog::LogCtx::middleware(false, None, None, None, None),
        media: pl_documents_write::files::MediaCfg::from_config(&cfg),
    }
}

async fn live_doc_ids(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar::<_, i32>("SELECT id FROM documents_document WHERE deleted_at IS NULL AND root_document_id IS NULL ORDER BY id")
        .fetch_all(pool)
        .await
        .expect("ids")
        .into_iter()
        .map(i64::from)
        .collect()
}

/// Own tag without parents (no ancestor assignments).
async fn new_tag(pool: &PgPool, name: &str) -> i64 {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO documents_tag (name, match, matching_algorithm, is_insensitive, color, is_inbox_tag, owner_id) \
         VALUES ($1, '', 0, true, '#a6cee3', false, NULL) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("tag");
    i64::from(id)
}

async fn tag_count(pool: &PgPool, tag: i64) -> (i64, i64) {
    sqlx::query_as("SELECT count(*), count(DISTINCT document_id) FROM documents_document_tags WHERE tag_id = $1")
        .bind(tag as i32)
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn backend_pid(conn: &mut PgConnection) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *conn).await.expect("pid")
}

/// Waits until `pid` waits for a lock (at most 10 s).
async fn wait_until_blocked(pool: &PgPool, pid: i32) {
    for _ in 0..200 {
        let waiting: bool = sqlx::query_scalar("SELECT coalesce(bool_or(wait_event_type = 'Lock'), false) FROM pg_stat_activity WHERE pid = $1")
            .bind(pid)
            .fetch_one(pool)
            .await
            .expect("activity");
        if waiting {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("backend {pid} wartet nicht auf eine Sperre");
}

/// B3: the same tag-assignment INSERT from the consumer (ingestion) and from bulk_edit `add_tag`
/// on **one** connection (statement cache) – in both orders.
#[tokio::test]
#[ignore = "braucht PAPERRUST_FIX2_SERVER und die Vorlage pl_fix2_tpl (siehe Modulkopf)"]
async fn b3_tag_insert_same_connection() {
    let Some(server) = server() else { return };
    let name = "pl_fix2_b3";
    let pool = fresh_db(&server, name, 1).await;
    let ids = live_doc_ids(&pool).await;
    let ex = ctx();
    for (round, consumer_first) in [(0, true), (1, false)] {
        let tag = new_tag(&pool, &format!("fix2-b3-{round}")).await;
        let (a, b) = (ids[round * 2], ids[round * 2 + 1]);
        let mut conn = pool.acquire().await.expect("conn");
        if consumer_first {
            pl_consumer::db::add_tags(&mut conn, a, &[tag]).await.expect("consumer add_tags");
            bulk_edit::execute(&mut conn, &[b], &Params::Tag { id: tag, add: true }, &ex).await.expect("bulk_edit add_tag after consumer");
        } else {
            bulk_edit::execute(&mut conn, &[b], &Params::Tag { id: tag, add: true }, &ex).await.expect("bulk_edit add_tag");
            pl_consumer::db::add_tags(&mut conn, a, &[tag]).await.expect("consumer add_tags after bulk_edit");
        }
        drop(conn);
        assert_eq!(tag_count(&pool, tag).await, (2, 2));
    }

    // Control case (old state): the same text once with int4, then with int8 on the same
    // connection → PostgreSQL rejects the 8-byte values of the cached int4 statement.
    let tag = new_tag(&pool, "fix2-b3-control").await;
    let mut tx = pool.begin().await.expect("tx");
    let sql = r#"INSERT INTO "documents_document_tags" ("document_id", "tag_id") VALUES ($1, $2)"#;
    sqlx::query(sql).bind(ids[10] as i32).bind(tag as i32).execute(&mut *tx).await.expect("int4");
    let clash = sqlx::query(sql).bind(ids[11]).bind(tag).execute(&mut *tx).await;
    let msg = format!("{clash:?}");
    assert!(clash.is_err() && msg.contains("binary data format"), "Kontrollfall B3 nicht nachgestellt: {msg}");
    drop(tx);
    drop_db(&server, pool, name).await;
}

/// B4 control case: the earlier unsorted `UPDATE … WHERE id = ANY($1)` (table order)
/// deadlocks in the same situation with the sorting trigger.
#[tokio::test]
#[ignore = "braucht PAPERRUST_FIX2_SERVER und die Vorlage pl_fix2_tpl (siehe Modulkopf)"]
async fn b4_control_unsorted_update_deadlocks() {
    let Some(server) = server() else { return };
    let name = "pl_fix2_b4c";
    let pool = fresh_db(&server, name, 6).await;
    let ids = live_doc_ids(&pool).await;
    for id in ids.iter().rev() {
        sqlx::query("UPDATE documents_document SET modified = modified WHERE id = $1").bind(*id as i32).execute(&pool).await.expect("rewrite");
    }
    let min = ids[0];
    let tag = new_tag(&pool, "fix2-b4c").await;
    let mut b = pool.begin().await.expect("tx b");
    sqlx::query("SELECT id FROM paperrust_document WHERE id = $1 FOR NO KEY UPDATE").bind(min).execute(&mut *b).await.expect("lock min");
    let mut a_conn = pool.acquire().await.expect("conn a");
    let a_pid = backend_pid(&mut a_conn).await;
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let a = tokio::spawn(async move {
        let r = sqlx::query("UPDATE documents_document SET modified = now() WHERE id = ANY($1) AND deleted_at IS NULL")
            .bind(&ids32)
            .execute(&mut *a_conn)
            .await;
        drop(a_conn);
        r.map(|_| ()).map_err(|e| format!("{e}"))
    });
    wait_until_blocked(&pool, a_pid).await;
    let ex = ctx();
    let rb = bulk_edit::execute(&mut b, &ids, &Params::Tag { id: tag, add: true }, &ex).await;
    let rb = match rb {
        Ok(_) => b.commit().await.map_err(|e| format!("{e}")),
        Err(e) => Err(format!("{e:?}")),
    };
    let ra = tokio::time::timeout(Duration::from_secs(30), a).await.expect("A hängt").expect("join");
    let both = format!("A={ra:?} B={rb:?}");
    assert!(both.contains("deadlock"), "Kontrollfall B4 nicht nachgestellt: {both}");
    drop_db(&server, pool, name).await;
}

/// B4: multi-row `modified` update (follow-up work) concurrently with bulk_edit, whose trigger locks the
/// document rows sorted by id. The table order is deliberately reversed
/// (as after many changes). Expected: the update also locks by id – so it holds nothing
/// while it waits for the smallest id – and both sides run through without deadlock.
#[tokio::test]
#[ignore = "braucht PAPERRUST_FIX2_SERVER und die Vorlage pl_fix2_tpl (siehe Modulkopf)"]
async fn b4_touch_modified_vs_bulk_edit_no_deadlock() {
    let Some(server) = server() else { return };
    let name = "pl_fix2_b4";
    let pool = fresh_db(&server, name, 6).await;
    let ids = live_doc_ids(&pool).await;
    // Reverse the table order: rewrite descending, one by one (new row versions at the end)
    for id in ids.iter().rev() {
        sqlx::query("UPDATE documents_document SET modified = modified WHERE id = $1").bind(*id as i32).execute(&pool).await.expect("rewrite");
    }
    let (min, max) = (ids[0], ids[ids.len() - 1]);
    let tag = new_tag(&pool, "fix2-b4").await;

    // B = bulk_edit transaction that has already locked the smallest id (like the sorting trigger)
    let mut b = pool.begin().await.expect("tx b");
    sqlx::query("SELECT id FROM paperrust_document WHERE id = $1 FOR NO KEY UPDATE").bind(min).execute(&mut *b).await.expect("lock min");

    // A = follow-up work (bulk_update_documents / touch_modified), own connection, autocommit
    let mut a_conn = pool.acquire().await.expect("conn a");
    let a_pid = backend_pid(&mut a_conn).await;
    let a_ids = ids.clone();
    let a = tokio::spawn(async move {
        let r = pl_documents_write::docs::touch_modified(&mut a_conn, &a_ids).await;
        drop(a_conn);
        r
    });
    wait_until_blocked(&pool, a_pid).await;
    // A must not have locked anything besides the waiting smallest id
    let max_free = sqlx::query("SELECT id FROM paperrust_document WHERE id = $1 FOR NO KEY UPDATE NOWAIT").bind(max).execute(&pool).await;
    assert!(max_free.is_ok(), "Nacharbeit sperrt Zeilen nicht in id-Reihenfolge: {max_free:?}");

    // B adds the tag to all documents (trigger locks all documents sorted) and commits
    let ex = ctx();
    let rb = bulk_edit::execute(&mut b, &ids, &Params::Tag { id: tag, add: true }, &ex).await;
    assert!(rb.is_ok(), "bulk_edit: {rb:?}");
    b.commit().await.expect("commit b");
    let ra = tokio::time::timeout(Duration::from_secs(30), a).await.expect("A hängt").expect("join");
    assert!(ra.is_ok(), "touch_modified: {ra:?}");
    assert_eq!(tag_count(&pool, tag).await.1, ids.len() as i64);
    drop_db(&server, pool, name).await;
}

/// B5: two concurrent `add_tag` on the same documents – the second must not fail with
/// `duplicate key`; every document has the tag exactly once.
#[tokio::test]
#[ignore = "braucht PAPERRUST_FIX2_SERVER und die Vorlage pl_fix2_tpl (siehe Modulkopf)"]
async fn b5_parallel_add_tag() {
    let Some(server) = server() else { return };
    let name = "pl_fix2_b5";
    let pool = fresh_db(&server, name, 6).await;
    let ids: Vec<i64> = live_doc_ids(&pool).await.into_iter().take(40).collect();
    let tag = new_tag(&pool, "fix2-b5").await;
    let ex = ctx();

    let mut t1 = pool.begin().await.expect("tx1");
    bulk_edit::execute(&mut t1, &ids, &Params::Tag { id: tag, add: true }, &ex).await.expect("tx1 add_tag");

    let mut t2 = pool.begin().await.expect("tx2");
    let t2_pid = backend_pid(&mut t2).await;
    let ids2 = ids.clone();
    let second = tokio::spawn(async move {
        let ex = ctx();
        let r = bulk_edit::execute(&mut t2, &ids2, &Params::Tag { id: tag, add: true }, &ex).await;
        match r {
            Ok(_) => t2.commit().await.map_err(|e| format!("{e}")),
            Err(e) => Err(format!("{e:?}")),
        }
    });
    wait_until_blocked(&pool, t2_pid).await;
    t1.commit().await.expect("commit tx1");
    let r2 = tokio::time::timeout(Duration::from_secs(30), second).await.expect("tx2 hängt").expect("join");
    assert!(r2.is_ok(), "zweites add_tag: {r2:?}");
    assert_eq!(tag_count(&pool, tag).await, (ids.len() as i64, ids.len() as i64));
    drop_db(&server, pool, name).await;
}

/// B6: attach ids of a further bulk change to a waiting `bulk_update_documents` task
/// (with repetitions); running, already delivered, delayed or foreign tasks remain
/// untouched.
#[tokio::test]
#[ignore = "braucht PAPERRUST_FIX2_SERVER und die Vorlage pl_fix2_tpl (siehe Modulkopf)"]
async fn b6_coalesce_pending_bulk_update() {
    let Some(server) = server() else { return };
    let name = "pl_fix2_b6";
    let pool = fresh_db(&server, name, 2).await;
    let name_bu = pl_documents_write::enqueue::BULK_UPDATE_DOCUMENTS;
    let insert = |task_id: &'static str, task: &'static str, state: &'static str, attempts: i32, eta_s: i32| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO paperrust_task_queue (task_id, task_name, state, attempts, eta, kwargs) \
                 VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5), '{\"document_ids\": [1, 2]}'::jsonb)",
            )
            .bind(task_id)
            .bind(task)
            .bind(state)
            .bind(attempts)
            .bind(f64::from(eta_s))
            .execute(&pool)
            .await
            .expect("insert task");
        }
    };
    let kwargs = |task_id: &'static str| {
        let pool = pool.clone();
        async move {
            let v: serde_json::Value = sqlx::query_scalar("SELECT kwargs FROM paperrust_task_queue WHERE task_id = $1")
                .bind(task_id)
                .fetch_one(&pool)
                .await
                .expect("kwargs");
            v
        }
    };
    // nothing suitable is waiting → new task needed
    insert("t-running", name_bu, "running", 1, -1).await;
    insert("t-retried", name_bu, "queued", 1, -1).await;
    insert("t-later", name_bu, "queued", 0, 3600).await;
    insert("t-other", "documents.tasks.train_classifier", "queued", 0, -1).await;
    let mut tx = pool.begin().await.expect("tx");
    assert_eq!(pl_documents_write::coalesce::try_merge(&mut tx, &[2, 3]).await.expect("merge"), None);
    tx.commit().await.expect("commit");

    // waiting task → attached, with repetition
    insert("t-pending", name_bu, "queued", 0, -1).await;
    let mut tx = pool.begin().await.expect("tx");
    assert_eq!(pl_documents_write::coalesce::try_merge(&mut tx, &[2, 3]).await.expect("merge").as_deref(), Some("t-pending"));
    tx.commit().await.expect("commit");
    assert_eq!(kwargs("t-pending").await, serde_json::json!({"document_ids": [1, 2, 2, 3]}));
    for t in ["t-running", "t-retried", "t-later", "t-other"] {
        assert_eq!(kwargs(t).await, serde_json::json!({"document_ids": [1, 2]}), "{t}");
    }

    // currently locked by another transaction (worker is picking it up) → do not wait, new task
    let mut holder = pool.begin().await.expect("holder");
    sqlx::query("SELECT id FROM paperrust_task_queue WHERE task_id = 't-pending' FOR UPDATE").execute(&mut *holder).await.expect("hold");
    let mut tx = pool.begin().await.expect("tx");
    let r = tokio::time::timeout(Duration::from_secs(5), pl_documents_write::coalesce::try_merge(&mut tx, &[9])).await.expect("wartet nicht");
    assert_eq!(r.expect("merge"), None);
    drop(tx);
    drop(holder);
    drop_db(&server, pool, name).await;
}
