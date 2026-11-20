//! Hit cache (`pl_store::textcache`, docs/PERF-SCALE.md §12): same results as the
//! text condition – also after changes **after** the hit set was built.
//!
//! Procedure per search text: build the hit set; then in **one** transaction change rows (content
//! newly matches / no longer matches, title, new row, deleted, trash, new version with
//! matching content) and compare in the same transaction: ids in order and count
//! with hit set == without (pure text condition), for superusers and restricted users.
//! At the end `ROLLBACK` – the DB stays unchanged. In addition a second comparison with a
//! hit set that was built *before* a confirmed change (other connection,
//! `COMMIT`), afterwards rolling back that change.
//!
//! Only against a DB **copy** (e.g. from `paperless_snapshot`):
//!
//!   PL_TEXTCACHE_DB=postgres://paperless:paperless@127.0.0.1:54329/pl_textcache \
//!     cargo test -p pl-store -j 2 --test textcache_db -- --ignored --nocapture

use std::sync::Arc;

use pl_domain::filter::{DocFilter, Pred, Sort, TextCol, TextLookup};
use pl_store::docsql::{Builder, TextHit, Visibility};
use sea_query::{PostgresQueryBuilder, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use sqlx::{PgConnection, Row};

async fn ids(conn: &mut PgConnection, q: &SelectStatement) -> Vec<i64> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values)
        .fetch_all(&mut *conn)
        .await
        .unwrap_or_else(|e| panic!("{e}: {sql}"))
        .iter()
        .map(|r| i64::from(r.get::<i32, _>(0)))
        .collect()
}

async fn count(conn: &mut PgConnection, q: &SelectStatement) -> i64 {
    let (sql, values) = pl_core::pagination::count_statement(q).build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).fetch_one(&mut *conn).await.map(|r| r.get::<i64, _>(0)).expect("count")
}

fn leaf(col: TextCol, lookup: TextLookup, v: &str) -> Pred {
    Pred::Text { col, lookup, value: v.into() }
}

/// Compares all forms (title_content, title only, content only, NOT) with and without a hit set.
async fn compare(
    conn: &mut PgConnection,
    vis: &[Visibility],
    fold: Option<&pl_docfilter::textfold::UpperFold>,
    hits: &[TextHit],
    lookup: TextLookup,
    v: &str,
    label: &str,
) -> usize {
    let preds = [
        Pred::Or(vec![leaf(TextCol::Title, lookup, v), leaf(TextCol::Content, lookup, v)]),
        leaf(TextCol::Title, lookup, v),
        leaf(TextCol::Content, lookup, v),
        Pred::Not(Box::new(leaf(TextCol::Content, lookup, v))),
    ];
    let mut checked = 0;
    for vi in vis {
        for p in &preds {
            let f = DocFilter { pred: p.clone(), sort: Sort::Default };
            let plain = Builder::new(vi, fold).select(&f);
            let cached = Builder::new(vi, fold).with_hits(hits.to_vec()).select(&f);
            let s = cached.to_string(PostgresQueryBuilder);
            // own data model: no hit set for the title
            let title_only = matches!(p, Pred::Text { col: TextCol::Title, .. });
            let has_title_hit = hits.iter().any(|h| h.col == TextCol::Title);
            if !title_only || has_title_hit {
                assert!(s.contains("get_bit("), "{label}: Treffermenge nicht benutzt");
            }
            let (a, b) = (ids(conn, &plain).await, ids(conn, &cached).await);
            assert_eq!(a, b, "{label} {v:?} {p:?}: ids");
            let (ca, cb) = (count(conn, &plain).await, count(conn, &cached).await);
            assert_eq!(ca, cb, "{label} {v:?} {p:?}: count");
            assert_eq!(ca, a.len() as i64);
            checked += 1;
        }
    }
    checked
}

async fn build_hits(pool: &sqlx::PgPool, fold: Option<&pl_docfilter::textfold::UpperFold>, lookup: TextLookup, v: &str) -> Vec<TextHit> {
    let p = Pred::Or(vec![leaf(TextCol::Title, lookup, v), leaf(TextCol::Content, lookup, v)]);
    let hits = pl_store::docsql::text_hits_for(pool, fold, &p).await;
    // own data model: only the content (titles live in the narrow table)
    let want = if native(pool).await { 1 } else { 2 };
    assert_eq!(hits.len(), want, "Treffermengen für Titel und Inhalt");
    hits
}

async fn native(pool: &sqlx::PgPool) -> bool {
    pl_store::textcache::layout(pool).await.expect("layout") == pl_store::textcache::Layout::Native
}

/// Parallel build (several connections, one snapshot) == one query.
async fn parallel_equals_single(pool: &sqlx::PgPool, fold: Option<&pl_docfilter::textfold::UpperFold>, v: &str) {
    use pl_docfilter::sql::TextLookup as L;
    use pl_store::textcache::{HitColumn, build_for_tests};
    for c in [HitColumn::Content, HitColumn::Title] {
        let one = build_for_tests(pool, fold, c, L::IContains, v, 1).await.expect("one");
        let par = build_for_tests(pool, fold, c, L::IContains, v, 3).await.expect("par");
        match (one, par) {
            (Some(a), Some(b)) => {
                assert_eq!(a.hits, b.hits, "{c:?} {v:?}: Anzahl");
                assert_eq!(a.bitmap(), b.bitmap(), "{c:?} {v:?}: Bitmaske");
            }
            (a, b) => assert_eq!(a.is_some(), b.is_some(), "{c:?} {v:?}"),
        }
    }
}

/// New row as a copy of an existing one (all NOT-NULL columns), with its own content.
async fn insert_copy(conn: &mut PgConnection, from: i64, content: &str, root: Option<i64>) -> i64 {
    let cols: Vec<String> = sqlx::query_scalar(
        "SELECT quote_ident(column_name) FROM information_schema.columns \
          WHERE table_name = 'documents_document' AND table_schema = current_schema() AND is_generated = 'NEVER' \
            AND column_name NOT IN ('id', 'content', 'checksum', 'root_document_id', 'archive_serial_number', 'filename', 'archive_filename', 'archive_checksum') \
          ORDER BY ordinal_position",
    )
    .fetch_all(&mut *conn)
    .await
    .expect("columns");
    let list = cols.join(", ");
    let sql = format!(
        "INSERT INTO documents_document ({list}, content, checksum, root_document_id) \
         SELECT {list}, $2, md5(random()::text), $3 FROM documents_document WHERE id = $1 RETURNING id"
    );
    let id: i32 = sqlx::query_scalar(&sql).bind(from as i32).bind(content).bind(root.map(|r| r as i32)).fetch_one(&mut *conn).await.expect("insert");
    i64::from(id)
}

#[tokio::test]
#[ignore = "braucht eine DB-Kopie (PL_TEXTCACHE_DB), siehe Modulkopf"]
async fn hit_cache_equals_text_condition() {
    let db = std::env::var("PL_TEXTCACHE_DB").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/pl_textcache".into());
    assert!(!db.trim_end_matches('/').ends_with("/paperless"), "nie gegen die Referenz-DB `paperless`");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(3).connect(&db).await.expect("db");
    let fold = pl_docfilter::FilterDb::upper_fold(&pool).await;
    let fold = fold.as_deref();

    let mut vis = vec![Visibility::All];
    for id in [3i64, 4, 5] {
        if let Some(u) = pl_authz::user::load_user_by_id(&pool, id).await.expect("user") {
            vis.push(Visibility::for_user(&u));
        }
    }
    let live: Vec<i64> = sqlx::query_scalar::<_, i32>(
        "SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 6",
    )
    .fetch_all(&pool)
    .await
    .expect("ids")
    .into_iter()
    .map(i64::from)
    .collect();
    assert!(live.len() == 6, "mindestens 6 lebende Dokumente");

    // special characters of the folding (ı → I, ſ → S): "invoice"/"steuer" match this document only via
    // the folded form – tests the build condition `textcache::scan_cond` (PERF-SCALE §14).
    let special_old: String =
        sqlx::query_scalar("SELECT content FROM documents_document WHERE id = $1").bind(live[2] as i32).fetch_one(&pool).await.expect("old");
    sqlx::query("UPDATE documents_document SET content = $2 WHERE id = $1")
        .bind(live[2] as i32)
        .bind("Zahlung ınvoıce 2024, ſteuer und the reſt")
        .execute(&pool)
        .await
        .expect("special");
    if fold.is_some() {
        for v in ["invoice", "steuer"] {
            let h = build_hits(&pool, fold, TextLookup::IContains, v).await;
            let content = h.iter().find(|x| x.col == TextCol::Content).expect("Inhaltsmenge");
            assert!(content.set.contains(live[2]), "{v}: Treffer über Sonderzeichen fehlt in der Menge");
        }
    }

    let mut checked = 0;
    for (lookup, v) in [
        (TextLookup::IContains, "e"),
        (TextLookup::IContains, "invoice"),
        (TextLookup::IContains, "steuer"),
        (TextLookup::IContains, "quarterly-tax"),
        (TextLookup::IStartsWith, "the"),
        (TextLookup::IContains, "zzqxnotfound"),
        (TextLookup::IContains, "beitrag"),
        (TextLookup::IExact, "ohne"),
    ] {
        if lookup == TextLookup::IContains {
            parallel_equals_single(&pool, fold, v).await;
        }
        let hits = build_hits(&pool, fold, lookup, v).await;
        let mut tx = pool.begin().await.expect("tx");
        checked += compare(&mut tx, &vis, fold, &hits, lookup, v, "unverändert").await;
        // changes after the build (own transaction: xmin = own, newer transaction)
        let probe = format!("XX {} XX", v.to_uppercase());
        sqlx::query("UPDATE documents_document SET content = $2 WHERE id = $1").bind(live[0] as i32).bind(&probe).execute(&mut *tx).await.expect("u1");
        sqlx::query("UPDATE documents_document SET content = 'nichts passendes 0' , title = 'ohne' WHERE id = $1").bind(live[1] as i32).execute(&mut *tx).await.expect("u2");
        sqlx::query("UPDATE documents_document SET title = $2 WHERE id = $1").bind(live[2] as i32).bind(&probe).execute(&mut *tx).await.expect("u3");
        sqlx::query("DELETE FROM documents_document_tags WHERE document_id = $1").bind(live[3] as i32).execute(&mut *tx).await.expect("d0");
        sqlx::query("UPDATE documents_document SET deleted_at = now() WHERE id = $1").bind(live[4] as i32).execute(&mut *tx).await.expect("trash");
        let _new = insert_copy(&mut tx, live[5], &probe, None).await;
        let _ver = insert_copy(&mut tx, live[5], "Version ohne Treffer", Some(live[5])).await;
        let _ver2 = insert_copy(&mut tx, live[1], &probe, Some(live[1])).await;
        if native(&pool).await {
            // own data model: change text directly in the text table (narrow row stays the same)
            sqlx::query("UPDATE paperrust_document_content SET content = $2 WHERE document_id = $1")
                .bind(live[3] as i32)
                .bind(&probe)
                .execute(&mut *tx)
                .await
                .expect("native");
        }
        checked += compare(&mut tx, &vis, fold, &hits, lookup, v, "nach Änderungen").await;
        tx.rollback().await.expect("rollback");
    }

    // confirmed change from another transaction after the build
    let v = "invoice";
    let hits = build_hits(&pool, fold, TextLookup::IContains, v).await;
    let old: String = sqlx::query_scalar("SELECT content FROM documents_document WHERE id = $1").bind(live[0] as i32).fetch_one(&pool).await.expect("old");
    sqlx::query("UPDATE documents_document SET content = 'kein Treffer hier' WHERE id = $1").bind(live[0] as i32).execute(&pool).await.expect("commit");
    let mut conn = pool.acquire().await.expect("conn");
    checked += compare(&mut conn, &vis, fold, &hits, TextLookup::IContains, v, "bestätigt").await;
    drop(conn);
    sqlx::query("UPDATE documents_document SET content = $2 WHERE id = $1").bind(live[0] as i32).bind(old).execute(&pool).await.expect("restore");
    sqlx::query("UPDATE documents_document SET content = $2 WHERE id = $1").bind(live[2] as i32).bind(special_old).execute(&pool).await.expect("restore special");
    let _ = Arc::strong_count(&hits[0].set);
    println!("textcache: {checked} Vergleiche gleich");
}
