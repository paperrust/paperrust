//! Own data model (docs/DATAMODEL-V2.md §6): conversion stages on a copy of the reference data.
//!
//! Per stage checks: view = original (row hash over all columns in v3.4.2 order),
//! `datamodel check`, writing through the view (INSERT … RETURNING, UPDATE, UPDATE … FROM, DELETE,
//! computed columns, sync capture, search index marking, foreign keys), rollback and renewed
//! conversion.
//!
//! Runs against a **private copy** (`pl_dm_cli_test`, TEMPLATE `paperless_snapshot`), never against
//! `paperless`; the copy is dropped at the end.
//!
//! ```text
//! cargo test -p pl-cli -j 2 --test datamodel_db -- --ignored --test-threads=1
//! ```

use sqlx::PgPool;

const DB: &str = "pl_dm_cli_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

/// Hash over all rows of the relation (columns in table/view order, text representation).
async fn rows_hash(pool: &PgPool, rel: &str) -> String {
    sqlx::query_scalar(&format!("SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t.id)), '') FROM {rel} t"))
        .fetch_one(pool)
        .await
        .expect("hash")
}

async fn scalar_i64(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await.unwrap_or_else(|e| panic!("{sql}: {e}"))
}

async fn manage(args: &[&str]) -> i32 {
    pl_cli::run_async(args.iter().map(|s| s.to_string()).collect()).await
}

/// `document_count` per object: previous way (live) versus counters (stage 2), for all
/// reference users (2 admin, 3 alice, 4 bob, 5 carol, 6 dave inactive) and all four kinds.
async fn counters_match_live(pool: &PgPool, when: &str) {
    use pl_objects::model::Kind;
    use sea_query::PostgresQueryBuilder;
    async fn run(pool: &PgPool, q: sea_query::SelectStatement) -> std::collections::BTreeMap<i64, i64> {
        let (sql, values) = pl_core::sqlbind::SqlxBinder::build_sqlx(&q, PostgresQueryBuilder);
        let rows: Vec<(i64, i64)> = sqlx::query_as_with(&format!("SELECT k::int8, n::int8 FROM ({sql}) x"), values)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|e| panic!("{e}: {sql}"));
        rows.into_iter().filter(|(_, n)| *n != 0).collect()
    }
    let mut compared = 0;
    for uid in [2i64, 3, 4, 5, 6] {
        let user = pl_authz::user::load_user_by_id(pool, uid).await.expect("user").expect("user exists");
        for kind in [Kind::Tag, Kind::Correspondent, Kind::DocumentType, Kind::StoragePath] {
            let live = run(pool, pl_objects::perms::document_count_source_ext(&user, kind, None, false, false)).await;
            let cnt = run(pool, pl_objects::perms::document_count_source_ext(&user, kind, None, false, true)).await;
            // without an id list the previous path of a restricted user pre-filters to visible
            // objects (the list shows only those) → compare on its keys …
            let cnt_visible: std::collections::BTreeMap<i64, i64> =
                cnt.iter().filter(|(k, _)| live.contains_key(k)).map(|(k, v)| (*k, *v)).collect();
            assert_eq!(live, cnt_visible, "{when}: user {uid} {kind:?}");
            // … and with an explicit id list (no pre-filter) exactly over all keys
            let ids: Vec<i64> = cnt.keys().copied().collect();
            if !ids.is_empty() {
                let live_ids = run(pool, pl_objects::perms::document_count_source_ext(&user, kind, Some(&ids), false, false)).await;
                assert_eq!(live_ids, cnt, "{when}: user {uid} {kind:?} alle Schlüssel");
            }
            let ids: Vec<i64> = live.keys().copied().take(3).collect();
            let live_ids = run(pool, pl_objects::perms::document_count_source_ext(&user, kind, Some(&ids), false, false)).await;
            let cnt_ids = run(pool, pl_objects::perms::document_count_source_ext(&user, kind, Some(&ids), false, true)).await;
            assert_eq!(live_ids, cnt_ids, "{when}: user {uid} {kind:?} ids {ids:?}");
            compared += live.len();
        }
    }
    // Custom fields (kind 6, from stage 3): pl-fields-views previous way versus counters
    for uid in [2i64, 3, 4, 5, 6] {
        let user = pl_authz::user::load_user_by_id(pool, uid).await.expect("user").expect("user exists");
        let wrap = |q: sea_query::SelectStatement| {
            let mut o = sea_query::Query::select();
            o.expr_as(sea_query::Expr::cust("x.field_id"), sea_query::Alias::new("k"))
                .expr_as(sea_query::Expr::cust("x.c"), sea_query::Alias::new("n"))
                .from_subquery(q, sea_query::Alias::new("x"));
            o
        };
        let live = run(pool, wrap(pl_fields_views::custom_fields::count_select_for(&user, None, false))).await;
        let cnt = run(pool, wrap(pl_fields_views::custom_fields::count_select_for(&user, None, true))).await;
        assert_eq!(live, cnt, "{when}: user {uid} custom fields");
        compared += live.len();
    }
    assert!(compared > 50, "{when}: zu wenig verglichen ({compared})");
}

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (127.0.0.1:54329, paperless_snapshot)"]
async fn stages_convert_write_and_roll_back() {
    // SAFETY: only test in this file, runs before any other access to the environment.
    unsafe {
        for (k, v) in [
            ("PAPERLESS_DBENGINE", "postgresql"),
            ("PAPERLESS_DBHOST", "127.0.0.1"),
            ("PAPERLESS_DBPORT", "54329"),
            ("PAPERLESS_DBNAME", DB),
            ("PAPERLESS_DBUSER", "paperless"),
            ("PAPERLESS_DBPASS", "paperless"),
            ("PAPERRUST_DJANGO_URL", "off"),
            ("PAPERRUST_DATAMODEL_BACKUP", "off"),
            ("PAPERLESS_SECRET_KEY", "datamodel-test-local-only"),
        ] {
            std::env::set_var(k, v);
        }
    }
    let admin = PgPool::connect(ADMIN_URL).await.expect("admin");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE paperless_snapshot")).execute(&admin).await.expect("create");
    // paperless_snapshot is a template (no connections) → benchmark from the fresh copy
    let copy = PgPool::connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}")).await.expect("copy");
    let want_doc = rows_hash(&copy, "documents_document").await;
    let want_cfi = rows_hash(&copy, "documents_customfieldinstance").await;
    let want_tags = rows_hash(&copy, "documents_document_tags").await;
    let want_uop = rows_hash(&copy, "guardian_userobjectpermission").await;
    let want_gop = rows_hash(&copy, "guardian_groupobjectpermission").await;
    let mut want_labels = Vec::new();
    for t in ["documents_correspondent", "documents_documenttype", "documents_storagepath", "documents_customfield"] {
        want_labels.push((t, rows_hash(&copy, t).await));
    }
    copy.close().await;

    assert_eq!(manage(&["migrate", "--no-input"]).await, 0, "migrate");
    let pool = PgPool::connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}")).await.expect("db");
    assert_eq!(manage(&["datamodel", "check"]).await, 0, "datamodel check");

    // ---- Stage 1: view = original, structure
    let kind: String = sqlx::query_scalar("SELECT relkind::text FROM pg_class WHERE oid = 'documents_document'::regclass")
        .fetch_one(&pool)
        .await
        .expect("relkind");
    assert_eq!(kind, "v");
    assert_eq!(rows_hash(&pool, "documents_document").await, want_doc, "Sicht liefert dieselben Zeilen wie die Django-Tabelle");
    assert_eq!(rows_hash(&pool, "documents_document_tags").await, want_tags, "Stufe 2: Tag-Zuordnung unverändert");
    assert_eq!(rows_hash(&pool, "documents_customfieldinstance").await, want_cfi, "Stufe 3: Feldwerte unverändert");
    assert_eq!(rows_hash(&pool, "guardian_userobjectpermission").await, want_uop, "Stufe 4: Nutzer-Freigaben unverändert");
    assert_eq!(rows_hash(&pool, "guardian_groupobjectpermission").await, want_gop, "Stufe 4: Gruppen-Freigaben unverändert");
    for (t, want) in &want_labels {
        assert_eq!(&rows_hash(&pool, t).await, want, "Stufe 5: {t} über die Sicht unverändert");
    }
    // Stage 5: identity in the row, side table resolved
    for (rel, want) in [("documents_tag", "v"), ("paperrust_tag", "r"), ("paperrust_sync_object", "v"), ("paperrust_sync_ident", "r")] {
        let k: String = sqlx::query_scalar(&format!("SELECT relkind::text FROM pg_class WHERE oid = '{rel}'::regclass"))
            .fetch_one(&pool)
            .await
            .expect("relkind");
        assert_eq!(k, want, "{rel}");
    }
    assert_eq!(
        scalar_i64(&pool, "SELECT count(*) FROM paperrust_sync_ident WHERE kind = 8").await,
        scalar_i64(&pool, "SELECT count(*) FROM auth_user").await,
        "Nutzer haben eine Identität"
    );
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM paperrust_document WHERE uuid IS NULL").await, 0);
    assert_eq!(
        scalar_i64(&pool, "SELECT count(*) FROM paperrust_sync_object WHERE kind = 1").await,
        scalar_i64(&pool, "SELECT count(*) FROM paperrust_document WHERE root_document_id IS NULL").await
    );
    let st = pl_authz::grants::status(&pool).await.expect("grants status");
    assert!(st.native && st.ready(), "Stufe 4: Freigaben nativ und bereit");
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM ngx_legacy.documents_document").await, 0, "Vorlage leer");
    assert_eq!(
        scalar_i64(&pool, "SELECT count(*) FROM pg_constraint WHERE contype = 'f' AND confrelid = 'paperrust_document'::regclass").await,
        // content, file, root_document (own) + note, tags, customfieldinstance, sharelink, workflowrun,
        // sharelinkbundle_documents, documentbarcode, paperrust_ai_review
        11,
        "eingehende Fremdschlüssel umgehängt"
    );
    let triggers: Vec<String> = sqlx::query_scalar(
        "SELECT tgname::text FROM pg_trigger WHERE tgrelid = 'paperrust_document'::regclass AND NOT tgisinternal ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("trigger");
    assert_eq!(
        triggers,
        [
            "paperrust_count_del", "paperrust_count_ins", "paperrust_count_trunc", "paperrust_count_upd", // Stage 2
            "paperrust_search_doc_del", "paperrust_search_doc_upd", "paperrust_sync_del", "paperrust_sync_ins",
            "paperrust_sync_rev", // Stage 5
            "paperrust_sync_upd"
        ]
    );
    // Join removal: whoever does not read content does not read paperrust_document_content
    let plan: Vec<String> = sqlx::query_scalar("EXPLAIN SELECT id, title FROM documents_document WHERE id = 1")
        .fetch_all(&pool)
        .await
        .expect("explain");
    assert!(!plan.join("\n").contains("paperrust_document_content"), "{plan:?}");

    // ---- Writing through the view
    let next_expected = scalar_i64(&pool, "SELECT last_value + 1 FROM paperrust_document_id_seq").await;
    let (id, len): (i32, Option<i32>) = sqlx::query_as(
        "INSERT INTO documents_document (title, content, mime_type, checksum, created, modified, added, filename, owner_id)
         VALUES ('neu', 'Grüße aus Zürich', 'application/pdf', 'abc', '2026-01-01', now(), now(), 'neu.pdf', 3)
         RETURNING id, content_length",
    )
    .fetch_one(&pool)
    .await
    .expect("insert über Sicht");
    assert_eq!(i64::from(id), next_expected, "id aus der übernommenen Sequenz");
    assert_eq!(len, Some(16), "content_length = length(content)");
    let rev0 = scalar_i64(&pool, &format!("SELECT rev FROM paperrust_sync_object WHERE kind = 1 AND local_id = {id}")).await;
    // only text changed → content_length and sync rev follow
    sqlx::query("UPDATE documents_document SET content = 'kurz' WHERE id = $1").bind(id).execute(&pool).await.expect("update content");
    assert_eq!(scalar_i64(&pool, &format!("SELECT content_length::int8 FROM paperrust_document WHERE id = {id}")).await, 4);
    assert_eq!(
        scalar_i64(&pool, &format!("SELECT rev FROM paperrust_sync_object WHERE kind = 1 AND local_id = {id}")).await,
        rev0 + 1,
        "eine Anweisung = rev + 1"
    );
    // owner change → search index marking
    sqlx::query("DELETE FROM paperrust_search_dirty").execute(&pool).await.expect("clear dirty");
    sqlx::query("UPDATE documents_document SET owner_id = NULL, filename = 'neu2.pdf' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("update owner");
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM paperrust_search_dirty WHERE doc_id = {id}")).await, 1);
    let f: String = sqlx::query_scalar("SELECT filename FROM paperrust_document_file WHERE document_id = $1").bind(id).fetch_one(&pool).await.expect("file");
    assert_eq!(f, "neu2.pdf");
    // UPDATE … FROM over several rows (bulk change)
    let n = sqlx::query(
        "UPDATE documents_document d SET correspondent_id = v.c FROM (VALUES (1, NULL::int), (2, NULL::int)) v(id, c) WHERE d.id = v.id",
    )
    .execute(&pool)
    .await
    .expect("update from")
    .rows_affected();
    assert_eq!(n, 2);
    // unique file names still apply (same error class as Django: unique_violation)
    let dup = sqlx::query("UPDATE documents_document SET filename = (SELECT filename FROM documents_document WHERE id = 2) WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(dup.as_ref().err().and_then(|e| e.as_database_error()).and_then(|d| d.code()).as_deref() == Some("23505"), "{dup:?}");
    // Delete with note: foreign key (deferred) like Django – first the note, then the document
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query("INSERT INTO documents_note (note, created, document_id, user_id) VALUES ('n', now(), $1, NULL)")
        .bind(id)
        .execute(&mut *tx)
        .await
        .expect("note");
    sqlx::query("DELETE FROM documents_document WHERE id = $1").bind(id).execute(&mut *tx).await.expect("delete");
    assert!(tx.commit().await.is_err(), "verwaiste Notiz verletzt den Fremdschlüssel");
    let mut tx = pool.begin().await.expect("tx");
    let doc_uuid: uuid::Uuid = sqlx::query_scalar("SELECT uuid FROM paperrust_document WHERE id = $1").bind(id).fetch_one(&pool).await.expect("uuid");
    let doc_rev = scalar_i64(&pool, &format!("SELECT rev FROM paperrust_document WHERE id = {id}")).await;
    sqlx::query("DELETE FROM documents_document WHERE id = $1").bind(id).execute(&mut *tx).await.expect("delete");
    tx.commit().await.expect("commit delete");
    // Stage 5: tombstone with uuid and rev + 1, sequencer reports 'd'
    let (t_uuid, t_rev): (uuid::Uuid, i64) =
        sqlx::query_as("SELECT uuid, rev FROM paperrust_sync_tombstone WHERE kind = 1 AND local_id = $1").bind(id).fetch_one(&pool).await.expect("tombstone");
    assert_eq!((t_uuid, t_rev), (doc_uuid, doc_rev + 1));
    while scalar_i64(&pool, "SELECT paperrust_sync_sequence(100000)::int8").await > 0 {}
    let (op, l_uuid): (String, uuid::Uuid) = sqlx::query_as("SELECT op::text, uuid FROM paperrust_sync_log WHERE kind = 1 AND local_id = $1")
        .bind(i64::from(id))
        .fetch_one(&pool)
        .await
        .expect("log");
    assert_eq!((op.as_str(), l_uuid), ("d", doc_uuid));
    // Tag assignment and share count as a change of the document (rev + 1), label change on the label
    let r50 = scalar_i64(&pool, "SELECT rev FROM paperrust_document WHERE id = 50").await;
    sqlx::query("INSERT INTO documents_document_tags (document_id, tag_id) VALUES (50, 2) ON CONFLICT DO NOTHING")
        .execute(&pool)
        .await
        .expect("tag");
    assert_eq!(scalar_i64(&pool, "SELECT rev FROM paperrust_document WHERE id = 50").await, r50 + 1, "Tag-Zuordnung");
    let rt = scalar_i64(&pool, "SELECT rev FROM paperrust_tag WHERE id = 2").await;
    sqlx::query("UPDATE documents_tag SET color = '#123456' WHERE id = 2").execute(&pool).await.expect("tag update");
    assert_eq!(scalar_i64(&pool, "SELECT rev FROM paperrust_tag WHERE id = 2").await, rt + 1, "Tag selbst");
    let tid: i32 = sqlx::query_scalar(
        "INSERT INTO documents_tag (name, color, match, matching_algorithm, is_insensitive, is_inbox_tag, owner_id)
         VALUES ('dm5-neu', '#000000', '', 1, true, false, NULL) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("tag insert über Sicht");
    let tuuid: uuid::Uuid = sqlx::query_scalar("SELECT uuid FROM paperrust_tag WHERE id = $1").bind(tid).fetch_one(&pool).await.expect("uuid");
    assert!(tid > 0);
    let resolved = scalar_i64(&pool, &format!("SELECT local_id::int8 FROM paperrust_sync_object WHERE kind = 2 AND uuid = '{tuuid}'")).await;
    assert_eq!(resolved, i64::from(tid), "neuer Tag über uuid auflösbar");
    sqlx::query("DELETE FROM documents_tag WHERE id = $1").bind(tid).execute(&pool).await.expect("tag delete");
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM paperrust_document_content WHERE document_id = {id}")).await, 0);
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM paperrust_document_file WHERE document_id = {id}")).await, 0);

    // ---- Stage 2: counters = live count, also after writes (through the views)
    assert_eq!(manage(&["datamodel", "counts", "check"]).await, 0, "counts check");
    counters_match_live(&pool, "nach Umbau").await;
    for sql in [
        "UPDATE documents_document SET correspondent_id = 3 WHERE id BETWEEN 10 AND 40",
        "INSERT INTO documents_document_tags (document_id, tag_id) SELECT id, 5 FROM documents_document WHERE id BETWEEN 50 AND 60 ON CONFLICT DO NOTHING",
        "DELETE FROM documents_document_tags WHERE document_id BETWEEN 70 AND 80",
        "UPDATE documents_document SET deleted_at = now() WHERE id IN (90, 91)",
        "UPDATE documents_document SET deleted_at = NULL WHERE id = 196",
        // owner change of a root with versions (201–204 hang off 1 and 2)
        "UPDATE documents_document SET owner_id = 5 WHERE id IN (1, 2)",
        "UPDATE documents_document SET owner_id = NULL WHERE id = 3",
        "UPDATE paperrust_document SET storage_path_id = NULL WHERE id BETWEEN 120 AND 150",
        // field values (stage 3) through the view: new, deleted (soft and hard)
        "INSERT INTO documents_customfieldinstance (created, document_id, field_id, value_monetary) SELECT now(), 160, 7, 'CHF42.50' WHERE NOT EXISTS (SELECT 1 FROM documents_customfieldinstance WHERE document_id = 160 AND field_id = 7)",
        "UPDATE documents_customfieldinstance SET deleted_at = now() WHERE id IN (SELECT id FROM documents_customfieldinstance ORDER BY id LIMIT 3)",
        "DELETE FROM documents_customfieldinstance WHERE id IN (SELECT id FROM documents_customfieldinstance WHERE deleted_at IS NULL ORDER BY id DESC LIMIT 2)",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    // Share to alice for a document of bob (group/guardian → paperrust_grant via trigger)
    sqlx::query(
        "INSERT INTO guardian_userobjectpermission (object_pk, content_type_id, permission_id, user_id)
         SELECT '7', ct.id, p.id, 3 FROM django_content_type ct JOIN auth_permission p ON p.content_type_id = ct.id
          WHERE ct.app_label = 'documents' AND ct.model = 'document' AND p.codename = 'view_document'
         ON CONFLICT DO NOTHING",
    )
    .execute(&pool)
    .await
    .expect("grant");
    counters_match_live(&pool, "nach Schreiben").await;
    assert_eq!(manage(&["datamodel", "counts", "fold"]).await, 0, "fold");
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM paperrust_label_count_delta").await, 0);
    counters_match_live(&pool, "nach Falten").await;
    // the same counts via the share path L5 (view paperrust_grant on paperrust_object_grant)
    pl_authz::grants::set_override(Some(pl_authz::grants::Engine::Grants));
    counters_match_live(&pool, "Freigabe-Weg grants").await;
    // read natively (stage 4: building blocks directly on paperrust_object_grant)
    pl_authz::grants::set_native(true);
    counters_match_live(&pool, "Freigabe-Weg nativ").await;
    pl_authz::grants::set_native(false);
    pl_authz::grants::set_override(None);
    // Revoke the share via the guardian view → bitmask, L5 view and count follow
    sqlx::query("DELETE FROM guardian_userobjectpermission WHERE object_pk = '7' AND user_id = 3")
        .execute(&pool)
        .await
        .expect("revoke");
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM paperrust_grant WHERE subject = 3 AND object_id = 7").await, 0);
    counters_match_live(&pool, "nach Entzug").await;
    assert_eq!(manage(&["datamodel", "counts", "check"]).await, 0, "counts check 2");

    // ---- Rollback: table is back, same content as the view before
    let before_rollback = rows_hash(&pool, "documents_document").await;
    let ident_before = rows_hash(&pool, "(SELECT (kind::int8 << 32) + local_id AS id, uuid, rev FROM paperrust_sync_object WHERE deleted_at IS NULL)").await;
    let seq_before = scalar_i64(&pool, "SELECT last_value FROM paperrust_document_id_seq").await;
    pool.close().await;
    assert_eq!(manage(&["datamodel", "rollback", "--to", "0"]).await, 0, "rollback");
    // (rollback of all stages, newest first)
    let pool = PgPool::connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}")).await.expect("db");
    let kind: String = sqlx::query_scalar("SELECT relkind::text FROM pg_class WHERE oid = 'documents_document'::regclass")
        .fetch_one(&pool)
        .await
        .expect("relkind");
    assert_eq!(kind, "r");
    assert_eq!(rows_hash(&pool, "documents_document").await, before_rollback);
    assert_eq!(
        rows_hash(&pool, "(SELECT (kind::int8 << 32) + local_id AS id, uuid, rev FROM paperrust_sync_object WHERE deleted_at IS NULL)").await,
        ident_before,
        "Stufe 5 zurück: gleiche uuid/rev in der Seitentabelle"
    );
    assert_eq!(scalar_i64(&pool, "SELECT last_value FROM documents_document_id_seq").await, seq_before);
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM pg_class WHERE relname LIKE 'paperrust_document%' AND relkind = 'r'").await, 0);
    let triggers: Vec<String> = sqlx::query_scalar(
        "SELECT tgname::text FROM pg_trigger WHERE tgrelid = 'documents_document'::regclass AND NOT tgisinternal ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("trigger");
    assert_eq!(
        triggers,
        ["paperrust_search_doc_del", "paperrust_search_doc_upd", "paperrust_sync_del", "paperrust_sync_ins", "paperrust_sync_upd"]
    );
    pool.close().await;

    // ---- renewed conversion
    assert_eq!(manage(&["migrate", "--no-input"]).await, 0, "migrate again");
    assert_eq!(manage(&["datamodel", "check"]).await, 0, "check again");
    let pool = PgPool::connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}")).await.expect("db");
    assert_eq!(rows_hash(&pool, "documents_document").await, before_rollback);
    pool.close().await;

    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
}
