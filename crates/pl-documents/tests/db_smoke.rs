//! Read-only smoke test against the reference DB (reference/README.md).
//! `PL_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/paperless`
//! `cargo test -p pl-documents -j 2 -- --ignored`

use pl_documents::{DOCUMENT_FIELDS, SerializeAction, SerializeOpts, serialize_documents_db};

#[tokio::test]
#[ignore = "braucht die Referenz-DB (nur lesend)"]
async fn serialize_first_documents() {
    let url = std::env::var("PL_TEST_DATABASE_URL").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/paperless".into());
    let db = sqlx::PgPool::connect(&url).await.expect("db");
    let ids: Vec<i64> =
        sqlx::query_scalar::<_, i32>("SELECT id FROM documents_document WHERE root_document_id IS NULL ORDER BY id LIMIT 5")
            .fetch_all(&db)
            .await
            .expect("ids")
            .into_iter()
            .map(i64::from)
            .collect();
    let su: i32 =
        sqlx::query_scalar("SELECT id FROM auth_user WHERE is_superuser ORDER BY id LIMIT 1").fetch_one(&db).await.expect("superuser");
    let viewer = pl_authz::user::load_user_by_id(&db, i64::from(su)).await.expect("load").expect("viewer");
    let opts = SerializeOpts { action: SerializeAction::Retrieve, viewset_queryset: true, ..Default::default() };
    let docs = serialize_documents_db(&db, chrono_tz::UTC, &viewer, &ids, &opts).await.expect("serialize");
    assert_eq!(docs.len(), ids.len());
    let expected: Vec<&str> = DOCUMENT_FIELDS.iter().copied().filter(|f| *f != "permissions").collect();
    for d in &docs {
        let keys: Vec<&str> = d.as_object().expect("object").keys().map(String::as_str).collect();
        assert_eq!(keys, expected);
    }
}
