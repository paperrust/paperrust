//! `custom_field_query_matching_ids` (workflow trigger, security review final K-N1) against the
//! real database: evaluated over **one connection** it must yield the same set as over
//! the pool path of the document list (`custom_field_query_cond` with `&PgPool`). Read-only.
//!
//!   cargo test -p pl-docfilter -j 1 --test trigger_cfq_db -- --ignored --nocapture
//!
//! Environment: PL_REF_DB (default postgres://paperless:paperless@127.0.0.1:54329/paperless).

use std::collections::HashSet;

use pl_core::sqlbind::SqlxBinder;
use pl_docfilter::sql::DOC;
use pl_docfilter::{CustomFieldQueryMatch, FilterContext, cfquery, custom_field_query_matching_ids};
use sea_query::{Alias, PostgresQueryBuilder, Query};
use sqlx::Row;

#[tokio::test]
#[ignore = "braucht die Referenz-DB (reference/README.md)"]
async fn conn_path_equals_pool_path() {
    let db = std::env::var("PL_REF_DB").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/paperless".into());
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&db).await.expect("db");
    let tz = chrono_tz::Tz::UTC;
    let ids: Vec<i64> = sqlx::query_scalar::<_, i32>("SELECT id FROM documents_document ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("ids")
        .into_iter()
        .map(i64::from)
        .collect();
    let fields: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, data_type FROM documents_customfield ORDER BY id").fetch_all(&pool).await.expect("fields");
    let mut queries: Vec<String> = Vec::new();
    for (id, dt) in &fields {
        queries.push(format!("[{id}, \"exists\", true]"));
        queries.push(format!("[{id}, \"exists\", false]"));
        queries.push(format!("[{id}, \"isnull\", true]"));
        if dt == "string" {
            queries.push(format!("[{id}, \"icontains\", \"a\"]"));
        }
    }
    if let Some((a, _)) = fields.first() {
        queries.push(format!("[\"OR\", [[{a}, \"exists\", true], [\"NOT\", [{a}, \"exists\", true]]]]"));
    }
    let ctx = FilterContext::new(tz);
    let mut conn = pool.acquire().await.expect("conn");
    let mut checked = 0;
    for q in &queries {
        let cond = cfquery::custom_field_query_cond(q, "filter_custom_field_query", &ctx, &pool).await.expect("cond");
        let mut sel = Query::select().column((Alias::new(DOC), Alias::new("id"))).from(Alias::new(DOC)).to_owned();
        cond.apply_to(&mut sel);
        let (sql, values) = sel.build_sqlx(PostgresQueryBuilder);
        let expected: HashSet<i64> =
            sqlx::query_with(&sql, values).fetch_all(&pool).await.expect("pool").iter().map(|r| i64::from(r.get::<i32, _>(0))).collect();
        let got = custom_field_query_matching_ids(&mut conn, q, &ids, tz).await.expect("conn path");
        assert_eq!(got, CustomFieldQueryMatch::Ids(expected), "{q}");
        checked += 1;
    }
    // invalid (ValidationError) → Invalid like upstream `documents.none()`
    for bad in ["not json", "[999999, \"exists\", true]", "[\"AND\"]"] {
        assert_eq!(
            custom_field_query_matching_ids(&mut conn, bad, &ids, tz).await.expect("invalid"),
            CustomFieldQueryMatch::Invalid,
            "{bad}"
        );
    }
    // empty id set → no matches, no query
    if let Some(q) = queries.first() {
        assert_eq!(
            custom_field_query_matching_ids(&mut conn, q, &[], tz).await.expect("empty"),
            CustomFieldQueryMatch::Ids(HashSet::new())
        );
    }
    println!("{checked} Abfragen über {} Dokumente verglichen", ids.len());
}
