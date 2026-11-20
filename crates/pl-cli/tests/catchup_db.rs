//! `manage migrate` on a paperless-ngx 3.1.3 database: missing migrations are caught up in
//! place (like Django on update), after which the state is v3.4.2.
//!
//! Prerequisite: a copy of the 3.1.3 test database (tests/dbmigrate/make_v313_fixture.sh), name
//! starts with `pl_mig_` (never `paperless`):
//!
//! ```sh
//! bash tests/dbmigrate/make_v313_fixture.sh
//! psql … -c "CREATE DATABASE pl_mig_manage TEMPLATE pl_mig_v313"
//! export PAPERRUST_CATCHUP_TEST_DB=pl_mig_manage   # server 127.0.0.1:54329, user paperless
//! cargo test -p pl-cli -j 2 --test catchup_db -- --ignored --test-threads=1
//! ```

use sqlx::PgPool;

#[tokio::test]
#[ignore = "braucht eine 3.1.3-Testdatenbank (PAPERRUST_CATCHUP_TEST_DB, siehe Kopf)"]
async fn manage_migrate_catches_up_313() {
    let Ok(db) = std::env::var("PAPERRUST_CATCHUP_TEST_DB") else { return };
    assert!(db.starts_with("pl_mig_"), "refusing to run against database {db}");
    let port = std::env::var("PAPERRUST_CATCHUP_TEST_PORT").unwrap_or_else(|_| "54329".into());
    // SAFETY: only test in this file, runs before any other thread accesses the environment.
    unsafe {
        for (k, v) in [
            ("PAPERLESS_DBENGINE", "postgresql"),
            ("PAPERLESS_DBHOST", "127.0.0.1"),
            ("PAPERLESS_DBPORT", port.as_str()),
            ("PAPERLESS_DBNAME", db.as_str()),
            ("PAPERLESS_DBUSER", "paperless"),
            ("PAPERLESS_DBPASS", "paperless"),
        ] {
            std::env::set_var(k, v);
        }
    }
    let url = format!("postgres://paperless:paperless@127.0.0.1:{port}/{db}");
    let pool = PgPool::connect(&url).await.expect("connect");
    let count = |sql: &'static str| {
        let pool = pool.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&pool).await.expect(sql) }
    };
    assert_eq!(count("SELECT count(*) FROM django_migrations").await, 242, "Fixture ist nicht 3.1.3");
    let docs = count("SELECT count(*) FROM documents_document").await;

    // --check reports pending migrations (exit 1), changes nothing
    assert_eq!(pl_cli::run_async(vec!["migrate".into(), "--check".into()]).await, 1);
    assert_eq!(count("SELECT count(*) FROM django_migrations").await, 242);

    assert_eq!(pl_cli::run_async(vec!["migrate".into(), "--no-input".into()]).await, 0);
    assert_eq!(count("SELECT count(*) FROM django_migrations").await, 247);
    assert_eq!(count("SELECT count(*) FROM documents_document").await, docs, "Dokumente unverändert");
    assert_eq!(count("SELECT count(*) FROM documents_documentbarcode").await, 0);
    assert_eq!(
        count("SELECT count(*) FROM django_content_type WHERE app_label = 'documents' AND model = 'documentbarcode'").await,
        1,
        "post_migrate: ContentType ergänzt"
    );
    // new columns from 0017/0018 are NULL, RunPython 0016: no more ai_enabled = false
    assert_eq!(
        count("SELECT count(*) FROM paperless_applicationconfiguration WHERE llm_embedding_api_key IS NOT NULL OR barcode_store_values IS NOT NULL OR ai_enabled = false").await,
        0
    );

    // second run: nothing left to do, --check clean
    assert_eq!(pl_cli::run_async(vec!["migrate".into(), "--no-input".into()]).await, 0);
    assert_eq!(count("SELECT count(*) FROM django_migrations").await, 247);
    assert_eq!(pl_cli::run_async(vec!["migrate".into(), "--check".into()]).await, 0);
}
