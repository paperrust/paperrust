//! Review 1 M3: `reprocess_sources` with one query yields the same as the earlier
//! algorithm (one query per root document). Read-only against the reference DB.
//! `PL_DW_TEST_DB=postgres://paperless:paperless@127.0.0.1:54329/paperless cargo test -p pl-documents-write --test reprocess_reference -- --ignored`

use std::collections::BTreeSet;

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PL_DW_TEST_DB)"]
async fn single_query_matches_per_root_algorithm() {
    let url = std::env::var("PL_DW_TEST_DB").expect("PL_DW_TEST_DB");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let ids: Vec<i64> = (1..=210).collect();
    let got = pl_documents_write::bulk_edit::reprocess_sources(&mut conn, &ids).await.unwrap();

    // earlier algorithm
    let rows: Vec<(i32, Option<i32>)> = sqlx::query_as(
        "SELECT id, root_document_id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY id",
    )
    .bind(ids.iter().map(|i| *i as i32).collect::<Vec<_>>())
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let mut expected: Vec<i64> = Vec::new();
    for (id, root) in rows {
        let src = if root.is_some() {
            id
        } else {
            let v: Option<(i32,)> = sqlx::query_as(
                "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL \
                 ORDER BY version_index DESC NULLS LAST, id DESC LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&mut *conn)
            .await
            .unwrap();
            v.map(|(i,)| i).unwrap_or(id)
        };
        if !expected.contains(&i64::from(src)) {
            expected.push(i64::from(src));
        }
    }
    assert_eq!(got, expected);
    // versions 201-204 of documents 1/2 are sources, not the roots
    let set: BTreeSet<i64> = got.iter().copied().collect();
    assert!(!set.contains(&1) && !set.contains(&2));
}
