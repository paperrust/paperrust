//! Review 2 N13: `GROUP_REFERENCES` contains all tables with an FK to `auth_group` (read-only).
//! `PL_AUTH_TEST_DB=postgres://paperless:paperless@127.0.0.1:54329/paperless cargo test -p pl-auth-api --test group_fk_reference -- --ignored`

use std::collections::BTreeSet;

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PL_AUTH_TEST_DB)"]
async fn group_references_cover_all_foreign_keys() {
    let url = std::env::var("PL_AUTH_TEST_DB").expect("PL_AUTH_TEST_DB");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT tc.table_name::text FROM information_schema.table_constraints tc \
         JOIN information_schema.constraint_column_usage ccu ON ccu.constraint_name = tc.constraint_name \
         AND ccu.table_schema = tc.table_schema \
         WHERE tc.constraint_type = 'FOREIGN KEY' AND ccu.table_name = 'auth_group' AND tc.table_name <> 'auth_group'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let db: BTreeSet<String> = rows.into_iter().map(|r| r.0).collect();
    let list: BTreeSet<String> = pl_auth_api::groups::GROUP_REFERENCES.iter().map(|s| s.to_string()).collect();
    assert_eq!(db, list);
}
