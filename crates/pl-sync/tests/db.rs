//! DB tests for change capture against an **own** test database (never `paperless`).
//!
//! Run (database name must start with `pl_sync_test`):
//! ```text
//! createdb -O paperless pl_sync_test        (empty, or a copy of paperless_snapshot)
//! PAPERRUST_SYNC_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_sync_test \
//!   cargo test -p pl-sync -j 4 -- --ignored
//! ```
//! If the Django schema is missing, the v3.4.2 schema from `crates/pl-dbmigrate/schema` is created.
//! The tests run one after another (global lock), because sequencer/enable/disable act DB-wide;
//! checks refer only to rows created by the test itself.

use std::time::Duration;

use pl_sync::{Kind, Op, SyncConfig, SyncError};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool, Row};
use tokio::sync::{Mutex, watch};

static LOCK: Mutex<()> = Mutex::const_new(());

const SCHEMA_PRE: &str = include_str!("../../pl-dbmigrate/schema/v3.4.2-postgres-pre.sql");
const SCHEMA_POST: &str = include_str!("../../pl-dbmigrate/schema/v3.4.2-postgres-post.sql");

fn url() -> String {
    let url = std::env::var("PAPERRUST_SYNC_TEST_DATABASE_URL").expect("PAPERRUST_SYNC_TEST_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_sync_test"), "refusing to run against a non-test database ({db})");
    url
}

async fn pool() -> PgPool {
    let pool = PgPoolOptions::new().max_connections(8).connect(&url()).await.expect("connect");
    let has: bool = sqlx::query_scalar("SELECT to_regclass('public.documents_document') IS NOT NULL").fetch_one(&pool).await.unwrap();
    if !has {
        let mut tx = pool.begin().await.unwrap();
        sqlx::raw_sql(SCHEMA_PRE).execute(&mut *tx).await.unwrap();
        sqlx::raw_sql(SCHEMA_POST).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    // clean up the previous test run (disable --purge): always start enabled
    pl_sync::migrations::apply(&pool).await.expect("migration");
    let st = pl_sync::status(&pool).await.unwrap();
    if !st.enabled || st.triggers != st.expected_triggers {
        pl_sync::enable(&pool).await.unwrap();
    }
    pool
}

async fn conn() -> PgConnection {
    PgConnection::connect(&url()).await.unwrap()
}

/// Inserts a row; required columns without a default get type-appropriate random values.
async fn insert(pool: &PgPool, table: &str, set: &[(&str, &str)]) -> i64 {
    let mut c = pool.acquire().await.unwrap();
    insert_on(&mut c, table, set).await
}

async fn insert_on(c: &mut PgConnection, table: &str, set: &[(&str, &str)]) -> i64 {
    let rows = sqlx::query(
        "SELECT column_name::text, udt_name::text, coalesce(character_maximum_length, 0)
         FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = $1 AND is_nullable = 'NO'
           AND column_default IS NULL AND is_identity = 'NO' AND is_generated = 'NEVER'
         ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(&mut *c)
    .await
    .unwrap();
    let mut cols: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    for (k, v) in set {
        cols.push(format!("\"{k}\""));
        vals.push(v.to_string());
    }
    for r in rows {
        let name: String = r.get(0);
        if set.iter().any(|(k, _)| *k == name) {
            continue;
        }
        let udt: String = r.get(1);
        let max: i32 = r.get(2);
        let v = match udt.as_str() {
            "varchar" | "text" | "bpchar" => {
                let n = if max > 0 { max.min(32) } else { 32 };
                format!("left(md5(random()::text || clock_timestamp()::text), {n})")
            }
            "int2" | "int4" | "int8" | "float8" | "float4" | "numeric" => "0".into(),
            "bool" => "false".into(),
            "timestamptz" | "timestamp" => "now()".into(),
            "date" => "current_date".into(),
            "jsonb" | "json" => "'[]'".into(),
            "uuid" => "gen_random_uuid()".into(),
            other => panic!("unsupported column type {other} ({table}.{name})"),
        };
        cols.push(format!("\"{name}\""));
        vals.push(v);
    }
    let sql = format!("INSERT INTO {table} ({}) VALUES ({}) RETURNING id::bigint", cols.join(", "), vals.join(", "));
    sqlx::query_scalar(&sql).fetch_one(&mut *c).await.unwrap()
}

async fn exec(pool: &PgPool, sql: &str) {
    sqlx::raw_sql(sql).execute(pool).await.unwrap();
}

async fn rev(pool: &PgPool, kind: Kind, id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT rev FROM paperrust_sync_object WHERE kind = $1 AND local_id = $2")
        .bind(kind.code())
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// pending-list rows (count, any with perm_change)
async fn pending(pool: &PgPool, kind: Kind, id: i64) -> (i64, bool) {
    let r =
        sqlx::query("SELECT count(*), coalesce(bool_or(perm_change), false) FROM paperrust_sync_pending WHERE kind = $1 AND local_id = $2")
            .bind(kind.code())
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    (r.get(0), r.get(1))
}

/// Log-Eintrag (seq, op, rev, perm_seq)
async fn log(pool: &PgPool, kind: Kind, id: i64) -> Option<(i64, String, i64, Option<i64>)> {
    sqlx::query("SELECT seq, op::text, rev, perm_seq FROM paperrust_sync_log WHERE kind = $1 AND local_id = $2")
        .bind(kind.code())
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
}

async fn perm_epoch(pool: &PgPool) -> i64 {
    pl_sync::read_meta(pool).await.unwrap().perm_epoch
}

/// Let the sequencer run until `done` is fulfilled (foreign long transactions in the
/// cluster hold xmin and thus the feed back briefly – intended).
async fn sequence_until<F, Fut>(pool: &PgPool, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..600 {
        pl_sync::sequence_once(pool, 50_000).await.unwrap();
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("sequencer did not reach the expected state within 30 s");
}

async fn doc(pool: &PgPool) -> i64 {
    insert(pool, "documents_document", &[]).await
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn migration_is_idempotent_and_installs_all_triggers() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let before = pl_sync::status(&pool).await.unwrap();
    pl_sync::migrations::apply(&pool).await.unwrap();
    pl_sync::migrations::apply(&pool).await.unwrap();
    let st = pl_sync::status(&pool).await.unwrap();
    assert!(st.installed && st.enabled);
    // 13 object tables × 3 + 2 guardian × 3 + 3 permission tables × 3 + auth_user permissions
    assert_eq!(st.expected_triggers, 55);
    assert_eq!(st.triggers, 55);
    // a repeated run does not rebuild the feed (initial fill only with an empty log)
    assert_eq!(st.server_uuid, before.server_uuid);
    assert!(st.last_seq >= before.last_seq);
    assert_eq!(st.log_entries, before.log_entries);
    // only new paperrust_ objects: no column on Django tables
    let foreign: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns WHERE table_schema = 'public'
         AND table_name NOT LIKE 'paperrust\\_%' AND column_name LIKE '%paperrust%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(foreign, 0);
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn triggers_capture_every_table() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let user = insert(&pool, "auth_user", &[]).await;
    assert_eq!(rev(&pool, Kind::User, user).await, Some(1));
    let group = insert(&pool, "auth_group", &[]).await;
    assert_eq!(rev(&pool, Kind::Group, group).await, Some(1));

    // Dokument: insert/update/delete-Pfade
    let d = doc(&pool).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(1));
    assert_eq!(pending(&pool, Kind::Document, d).await, (1, false));
    exec(&pool, &format!("UPDATE documents_document SET title = 'changed' WHERE id = {d}")).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(2));
    // owner change → perm_change
    exec(&pool, &format!("UPDATE documents_document SET owner_id = {user} WHERE id = {d}")).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(3));
    assert_eq!(pending(&pool, Kind::Document, d).await, (3, true));

    // M2M tag link (bulk_edit.add_tag: bulk_create without modified)
    let tag = insert(&pool, "documents_tag", &[]).await;
    assert_eq!(rev(&pool, Kind::Tag, tag).await, Some(1));
    let d2 = doc(&pool).await;
    exec(&pool, &format!("INSERT INTO documents_document_tags (document_id, tag_id) VALUES ({d}, {tag}), ({d2}, {tag})")).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(4));
    assert_eq!(rev(&pool, Kind::Document, d2).await, Some(2));
    exec(&pool, &format!("DELETE FROM documents_document_tags WHERE tag_id = {tag}")).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(5));

    // note (create, soft delete) and custom field instance count towards the document
    let note = insert(&pool, "documents_note", &[("document_id", &d.to_string())]).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(6));
    exec(&pool, &format!("UPDATE documents_note SET deleted_at = now() WHERE id = {note}")).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(7));
    let cf = insert(&pool, "documents_customfield", &[]).await;
    assert_eq!(rev(&pool, Kind::CustomField, cf).await, Some(1));
    insert(&pool, "documents_customfieldinstance", &[("document_id", &d.to_string()), ("field_id", &cf.to_string())]).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(8));

    // document version counts as a change of the root document
    let v = insert(&pool, "documents_document", &[("root_document_id", &d.to_string()), ("version_index", "1")]).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, Some(9));
    assert_eq!(rev(&pool, Kind::Document, v).await, None);

    // further object kinds including owner change
    for (table, kind) in [
        ("documents_correspondent", Kind::Correspondent),
        ("documents_documenttype", Kind::DocumentType),
        ("documents_storagepath", Kind::StoragePath),
    ] {
        let id = insert(&pool, table, &[]).await;
        assert_eq!(rev(&pool, kind, id).await, Some(1), "{table}");
        exec(&pool, &format!("UPDATE {table} SET owner_id = {user} WHERE id = {id}")).await;
        assert_eq!(rev(&pool, kind, id).await, Some(2), "{table}");
        assert!(pending(&pool, kind, id).await.1, "{table}: owner change sets perm_change");
        exec(&pool, &format!("UPDATE {table} SET owner_id = owner_id WHERE id = {id}")).await;
        assert_eq!(pending(&pool, kind, id).await.0, 3, "{table}");
    }
    let sv = insert(&pool, "documents_savedview", &[("owner_id", &user.to_string())]).await;
    assert_eq!(rev(&pool, Kind::SavedView, sv).await, Some(1));
    insert(&pool, "documents_savedviewfilterrule", &[("saved_view_id", &sv.to_string())]).await;
    assert_eq!(rev(&pool, Kind::SavedView, sv).await, Some(2));

    // guardian: object permission on the document → perm_change (empty DB: create ContentTypes/permissions)
    exec(
        &pool,
        "INSERT INTO django_content_type (app_label, model) VALUES ('documents', 'document'), ('documents', 'tag')
           ON CONFLICT DO NOTHING;
         INSERT INTO auth_permission (name, content_type_id, codename)
           SELECT 'x', ct.id, c FROM django_content_type ct,
                  (VALUES ('document', 'view_document'), ('tag', 'change_tag')) v(m, c)
           WHERE ct.app_label = 'documents' AND ct.model = v.m
           ON CONFLICT DO NOTHING",
    )
    .await;
    let d3 = doc(&pool).await;
    exec(
        &pool,
        &format!(
            "INSERT INTO guardian_userobjectpermission (object_pk, content_type_id, permission_id, user_id)
             SELECT '{d3}', ct.id, p.id, {user} FROM django_content_type ct
             JOIN auth_permission p ON p.content_type_id = ct.id AND p.codename = 'view_document'
             WHERE ct.app_label = 'documents' AND ct.model = 'document'"
        ),
    )
    .await;
    assert_eq!(rev(&pool, Kind::Document, d3).await, Some(2));
    assert_eq!(pending(&pool, Kind::Document, d3).await, (2, true));
    exec(
        &pool,
        &format!(
            "INSERT INTO guardian_groupobjectpermission (object_pk, content_type_id, permission_id, group_id)
             SELECT '{tag}', ct.id, p.id, {group} FROM django_content_type ct
             JOIN auth_permission p ON p.content_type_id = ct.id AND p.codename = 'change_tag'
             WHERE ct.app_label = 'documents' AND ct.model = 'tag'"
        ),
    )
    .await;
    assert_eq!(rev(&pool, Kind::Tag, tag).await, Some(2));
    assert!(pending(&pool, Kind::Tag, tag).await.1);

    // group/permission assignment: only perm_epoch
    let e0 = perm_epoch(&pool).await;
    exec(&pool, &format!("INSERT INTO auth_user_groups (user_id, group_id) VALUES ({user}, {group})")).await;
    assert_eq!(perm_epoch(&pool).await, e0 + 1);
    exec(&pool, &format!("DELETE FROM auth_user_groups WHERE user_id = {user} AND group_id = -1")).await;
    assert_eq!(perm_epoch(&pool).await, e0 + 1, "statement without rows does not bump");
    exec(&pool, &format!("UPDATE auth_user SET last_login = now() WHERE id = {user}")).await;
    assert_eq!(perm_epoch(&pool).await, e0 + 1, "login does not bump perm_epoch");
    exec(&pool, &format!("UPDATE auth_user SET is_superuser = true WHERE id = {user}")).await;
    assert_eq!(perm_epoch(&pool).await, e0 + 2);
    exec(
        &pool,
        &format!(
            "INSERT INTO auth_user_user_permissions (user_id, permission_id) SELECT {user}, min(id) FROM auth_permission;
             INSERT INTO auth_group_permissions (group_id, permission_id) SELECT {group}, min(id) FROM auth_permission"
        ),
    )
    .await;
    assert_eq!(perm_epoch(&pool).await, e0 + 4);

    // Delete
    exec(
        &pool,
        &format!("DELETE FROM documents_customfieldinstance WHERE document_id = {d}; DELETE FROM documents_customfield WHERE id = {cf}"),
    )
    .await;
    assert_eq!(rev(&pool, Kind::CustomField, cf).await, Some(2));

    // rollback leaves nothing behind
    let mut c = conn().await;
    let mut tx = c.begin().await.unwrap();
    let gone = insert_on(&mut tx, "documents_tag", &[]).await;
    tx.rollback().await.unwrap();
    assert_eq!(rev(&pool, Kind::Tag, gone).await, None);
    assert_eq!(pending(&pool, Kind::Tag, gone).await.0, 0);
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn sequencer_upserts_tombstones_and_restores() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let d = doc(&pool).await;
    sequence_until(&pool, || async { log(&pool, Kind::Document, d).await.is_some() }).await;
    let (s1, op, rev1, _) = log(&pool, Kind::Document, d).await.unwrap();
    assert_eq!((op.as_str(), rev1), ("u", 1));
    assert_eq!(pending(&pool, Kind::Document, d).await.0, 0);

    // Papierkorb → delete
    exec(&pool, &format!("UPDATE documents_document SET deleted_at = now() WHERE id = {d}")).await;
    sequence_until(&pool, || async { log(&pool, Kind::Document, d).await.is_some_and(|l| l.0 > s1) }).await;
    let (s2, op, rev2, _) = log(&pool, Kind::Document, d).await.unwrap();
    assert_eq!((op.as_str(), rev2), ("d", 2));
    let (oseq, deleted): (Option<i64>, bool) =
        sqlx::query_as("SELECT seq, deleted_at IS NOT NULL FROM paperrust_sync_object WHERE kind = 1 AND local_id = $1")
            .bind(d)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((oseq, deleted), (Some(s2), true));

    // Wiederherstellen → upsert, gleiche UUID
    let uuid_before: uuid::Uuid =
        sqlx::query_scalar("SELECT uuid FROM paperrust_sync_log WHERE kind = 1 AND local_id = $1").bind(d).fetch_one(&pool).await.unwrap();
    exec(&pool, &format!("UPDATE documents_document SET deleted_at = NULL WHERE id = {d}")).await;
    sequence_until(&pool, || async { log(&pool, Kind::Document, d).await.is_some_and(|l| l.0 > s2) }).await;
    let (s3, op, _, _) = log(&pool, Kind::Document, d).await.unwrap();
    assert_eq!(op, "u");
    let uuid_after: uuid::Uuid =
        sqlx::query_scalar("SELECT uuid FROM paperrust_sync_log WHERE kind = 1 AND local_id = $1").bind(d).fetch_one(&pool).await.unwrap();
    assert_eq!(uuid_before, uuid_after);

    // delete permanently → delete; exactly one log entry per object
    exec(&pool, &format!("DELETE FROM documents_document WHERE id = {d}")).await;
    sequence_until(&pool, || async { log(&pool, Kind::Document, d).await.is_some_and(|l| l.0 > s3) }).await;
    assert_eq!(log(&pool, Kind::Document, d).await.unwrap().1, "d");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_sync_log WHERE kind = 1 AND local_id = $1")
        .bind(d)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);

    // perm_seq is kept on later changes without a permission change
    let t = insert(&pool, "documents_tag", &[]).await;
    let u = insert(&pool, "auth_user", &[]).await;
    exec(&pool, &format!("UPDATE documents_tag SET owner_id = {u} WHERE id = {t}")).await;
    sequence_until(&pool, || async { log(&pool, Kind::Tag, t).await.is_some_and(|l| l.2 == 2) }).await;
    let (ps, _, _, perm) = log(&pool, Kind::Tag, t).await.unwrap();
    assert_eq!(perm, Some(ps));
    exec(&pool, &format!("UPDATE documents_tag SET name = name || 'x' WHERE id = {t}")).await;
    sequence_until(&pool, || async { log(&pool, Kind::Tag, t).await.is_some_and(|l| l.2 == 3) }).await;
    let (ps2, _, _, perm2) = log(&pool, Kind::Tag, t).await.unwrap();
    assert!(ps2 > ps);
    assert_eq!(perm2, Some(ps), "earlier permission change stays visible to older cursors");
}

/// SYNC.md §9: A starts before B (smaller xid), B commits first → both in the feed, A is not
/// skipped (A appears with a seq above every cursor read in the meantime).
#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn late_committing_transaction_is_not_skipped() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let mut ca = conn().await;
    let mut cb = conn().await;
    let mut ta = ca.begin().await.unwrap();
    let a = insert_on(&mut ta, "documents_document", &[]).await;
    let xa: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text").fetch_one(&mut *ta).await.unwrap();
    let mut tb = cb.begin().await.unwrap();
    let b = insert_on(&mut tb, "documents_document", &[]).await;
    let xb: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text").fetch_one(&mut *tb).await.unwrap();
    assert!(xa.parse::<u64>().unwrap() < xb.parse::<u64>().unwrap());
    tb.commit().await.unwrap();

    // B is committed, A is still running: the sequencer must not number B yet (xid_B ≥ xmin)
    for _ in 0..5 {
        pl_sync::sequence_once(&pool, 50_000).await.unwrap();
    }
    assert!(log(&pool, Kind::Document, a).await.is_none());
    assert!(log(&pool, Kind::Document, b).await.is_none(), "B must wait for the older open transaction A");
    assert_eq!(pending(&pool, Kind::Document, b).await.0, 1);

    // client reads the feed to the end
    let page = pl_sync::changes(&pool, None, 1000, Some(&[Kind::Tag])).await.unwrap();
    let cursor = page.next_cursor;

    ta.commit().await.unwrap();
    sequence_until(&pool, || async { log(&pool, Kind::Document, a).await.is_some() && log(&pool, Kind::Document, b).await.is_some() })
        .await;
    let sa = log(&pool, Kind::Document, a).await.unwrap().0;
    let sb = log(&pool, Kind::Document, b).await.unwrap().0;
    assert!(sa > cursor && sb > cursor, "cursor {cursor}, a {sa}, b {sb}");

    // feed from the cursor contains both
    let mut seen = Vec::new();
    let mut c = cursor;
    loop {
        let p = pl_sync::changes(&pool, Some(c), 500, Some(&[Kind::Document])).await.unwrap();
        seen.extend(p.changes.iter().map(|x| x.local_id));
        c = p.next_cursor;
        if !p.has_more {
            break;
        }
    }
    assert!(seen.contains(&a) && seen.contains(&b));
}

/// The sequencer never waits for writers (SKIP LOCKED) and does not block them.
#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn sequencer_does_not_wait_for_writers() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let d = doc(&pool).await;
    sequence_until(&pool, || async { log(&pool, Kind::Document, d).await.is_some() }).await;
    exec(&pool, &format!("UPDATE documents_document SET title = 'one' WHERE id = {d}")).await;
    // writer holds the paperrust_sync_object row locked
    let mut cw = conn().await;
    let mut tw = cw.begin().await.unwrap();
    sqlx::query(&format!("UPDATE documents_document SET title = 'two' WHERE id = {d}")).execute(&mut *tw).await.unwrap();
    let r = tokio::time::timeout(Duration::from_secs(10), pl_sync::sequence_once(&pool, 50_000)).await;
    assert!(r.is_ok(), "sequencer blocked by an open writer");
    tw.commit().await.unwrap();
    sequence_until(&pool, || async { log(&pool, Kind::Document, d).await.is_some_and(|l| l.2 == 3) }).await;
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn retention_removes_old_tombstones_and_expires_cursors() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let d = doc(&pool).await;
    exec(&pool, &format!("DELETE FROM documents_document WHERE id = {d}")).await;
    sequence_until(&pool, || async { log(&pool, Kind::Document, d).await.is_some_and(|l| l.1 == "d") }).await;
    let (seq, ..) = log(&pool, Kind::Document, d).await.unwrap();
    // frischer Tombstone bleibt
    pl_sync::retention(&pool, 180).await.unwrap();
    assert!(log(&pool, Kind::Document, d).await.is_some());
    exec(&pool, &format!("UPDATE paperrust_sync_log SET at = now() - interval '181 days' WHERE kind = 1 AND local_id = {d}")).await;
    assert!(pl_sync::retention(&pool, 180).await.unwrap() >= 1);
    assert!(log(&pool, Kind::Document, d).await.is_none());
    assert_eq!(rev(&pool, Kind::Document, d).await, None, "object identity removed with the tombstone");
    let meta = pl_sync::read_meta(&pool).await.unwrap();
    assert!(meta.min_cursor >= seq);
    match pl_sync::changes(&pool, Some(seq - 1), 10, None).await {
        Err(SyncError::CursorExpired { min_cursor }) => assert_eq!(min_cursor, meta.min_cursor),
        other => panic!("expected cursor_expired, got {other:?}"),
    }
    assert!(pl_sync::changes(&pool, Some(meta.min_cursor), 10, None).await.is_ok());
    assert!(pl_sync::changes(&pool, None, 10, None).await.is_ok(), "full sync always allowed");
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn feed_pages_in_seq_order() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let start = pl_sync::status(&pool).await.unwrap().last_seq;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(insert(&pool, "documents_correspondent", &[]).await);
    }
    let last = *ids.last().unwrap();
    sequence_until(&pool, || async { log(&pool, Kind::Correspondent, last).await.is_some() }).await;
    let mut got = Vec::new();
    let mut c = start;
    let mut pages = 0;
    loop {
        let p = pl_sync::changes(&pool, Some(c), 2, Some(&[Kind::Correspondent])).await.unwrap();
        pages += 1;
        let mut prev = c;
        for ch in &p.changes {
            assert!(ch.seq > prev);
            prev = ch.seq;
            assert_eq!(ch.kind, Kind::Correspondent);
            assert_eq!(ch.op, Op::Upsert);
            got.push(ch.local_id);
        }
        assert!(p.next_cursor >= c);
        c = p.next_cursor;
        if !p.has_more {
            break;
        }
    }
    assert!(pages >= 3);
    for id in &ids {
        assert!(got.contains(id));
    }
    // leere Folgeseite, Cursor bleibt stabil
    let p = pl_sync::changes(&pool, Some(c), 2, Some(&[Kind::Correspondent])).await.unwrap();
    assert!(p.changes.is_empty() && !p.has_more && p.next_cursor >= c);
    assert_eq!(pl_sync::parse_cursor(&pl_sync::format_cursor(c)).unwrap(), c);
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn disable_enable_rebuild_and_purge() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let removed = pl_sync::disable(&pool, false).await.unwrap();
    assert_eq!(removed, 55);
    let st = pl_sync::status(&pool).await.unwrap();
    assert!(!st.enabled && st.triggers == 0 && st.pending == 0);
    // schema stays usable for Django/paperless-ngx, nothing is captured
    let d = doc(&pool).await;
    exec(&pool, &format!("UPDATE documents_document SET title = 'while disabled' WHERE id = {d}")).await;
    assert_eq!(rev(&pool, Kind::Document, d).await, None);
    // repeated migration respects "disabled"
    pl_sync::migrations::apply(&pool).await.unwrap();
    assert_eq!(pl_sync::status(&pool).await.unwrap().triggers, 0);

    let e0 = perm_epoch(&pool).await;
    let r = pl_sync::enable(&pool).await.unwrap();
    assert!(!r.migrated);
    assert_eq!(r.triggers, 55);
    assert!(r.rebuilt >= 1);
    assert_eq!(log(&pool, Kind::Document, d).await.unwrap().1, "u", "rebuild captured the change made while disabled");
    assert!(perm_epoch(&pool).await > e0, "rebuild forces clients to re-check visibility");

    // rebuild is idempotent (no duplicate objects, one log entry per object)
    let objects = pl_sync::status(&pool).await.unwrap().objects;
    pl_sync::rebuild(&pool).await.unwrap();
    let st = pl_sync::status(&pool).await.unwrap();
    assert_eq!(st.objects, objects);
    let dup: i64 =
        sqlx::query_scalar("SELECT count(*) FROM (SELECT kind, local_id FROM paperrust_sync_log GROUP BY 1, 2 HAVING count(*) > 1) x")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(dup, 0);
    let missing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents_document d WHERE d.deleted_at IS NULL AND d.root_document_id IS NULL
         AND NOT EXISTS (SELECT 1 FROM paperrust_sync_log l WHERE l.kind = 1 AND l.local_id = d.id AND l.op = 'u')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(missing, 0);

    // complete rollback and reinstall
    let uuid_before = pl_sync::read_meta(&pool).await.unwrap().server_uuid;
    pl_sync::disable(&pool, true).await.unwrap();
    assert!(!pl_sync::installed(&pool).await.unwrap());
    let rest: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM pg_class WHERE relname LIKE 'paperrust\\_sync%')
              + (SELECT count(*) FROM pg_proc WHERE proname LIKE 'paperrust\\_sync%')
              + (SELECT count(*) FROM pg_trigger WHERE tgname LIKE 'paperrust\\_sync%')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rest, 0, "purge leaves no paperrust_sync objects");
    exec(&pool, "UPDATE documents_document SET title = title WHERE id IN (SELECT id FROM documents_document LIMIT 3)").await;
    let r = pl_sync::enable(&pool).await.unwrap();
    assert!(r.migrated && r.triggers == 55);
    assert_ne!(pl_sync::read_meta(&pool).await.unwrap().server_uuid, uuid_before);
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn reset_identity_sets_min_cursor() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let before = pl_sync::read_meta(&pool).await.unwrap();
    let last = pl_sync::status(&pool).await.unwrap().last_seq;
    let (id, min) = pl_sync::reset_identity(&pool).await.unwrap();
    assert_ne!(id.to_string(), before.server_uuid);
    assert_eq!(min, last);
    if last > 0 {
        assert!(matches!(pl_sync::changes(&pool, Some(last - 1), 10, None).await, Err(SyncError::CursorExpired { .. })));
    }
    let out = pl_sync::manage::command(&pool, &["status".to_string()]).await.unwrap();
    assert!(out.contains(&id.to_string()) && out.contains("enabled"), "{out}");
    assert!(matches!(pl_sync::manage::command(&pool, &["bogus".to_string()]).await, Err(pl_sync::manage::ManageError::Usage(_))));
}

#[tokio::test]
#[ignore = "needs PAPERRUST_SYNC_TEST_DATABASE_URL"]
async fn sequencer_loop_follows_notify_and_stops() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let cfg = SyncConfig { interval: Duration::from_millis(200), retention_interval: Duration::from_secs(3600), ..SyncConfig::default() };
    let (tx, rx) = watch::channel(false);
    let h = tokio::spawn(pl_sync::sequencer::run(pool.clone(), cfg, rx));
    let t = insert(&pool, "documents_tag", &[]).await;
    let mut ok = false;
    for _ in 0..300 {
        if log(&pool, Kind::Tag, t).await.is_some() {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ok, "sequencer loop did not pick up the change");
    // long-poll helper sees new entries
    let last = pl_sync::status(&pool).await.unwrap().last_seq;
    let waiter = tokio::spawn({
        let pool = pool.clone();
        async move { pl_sync::wait_for_changes(&pool, last, Duration::from_secs(20)).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    insert(&pool, "documents_tag", &[]).await;
    assert!(waiter.await.unwrap().unwrap());
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(10), h).await.expect("sequencer stops").unwrap();
}
