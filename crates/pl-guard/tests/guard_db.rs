//! Guard against a throwaway database (copy of `paperless_snapshot`, name `pl_guard_test*`):
//! catalogue from `documents_document`, findings/event/read-only state in the tables, lifting.
//! For invocation see `backup_roundtrip.rs` (same variable `PAPERRUST_GUARD_TEST_DATABASE_URL`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use pl_guard::config::GuardConfig;
use pl_guard::db::{DbCatalog, DbReporter};
use pl_guard::detect::Area;
use pl_guard::guard::{Catalog, Engine, Reporter};
use sqlx::PgPool;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("PAPERRUST_GUARD_TEST_DATABASE_URL").ok()?;
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_guard_test"), "refusing to run against database {name}");
    Some(PgPool::connect(&url).await.expect("connect"))
}

fn pdf(i: i64) -> Vec<u8> {
    format!("%PDF-1.7\n guard test {i}\n%%EOF\n").into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "braucht PAPERRUST_GUARD_TEST_DATABASE_URL (Wegwerf-DB, siehe backup_roundtrip.rs)"]
async fn guard_against_database() {
    let Some(pool) = pool().await else { return };
    let t = tempfile::tempdir().unwrap();
    let media = t.path().join("media");
    let data = t.path().join("data");
    std::fs::create_dir_all(media.join("documents/originals")).unwrap();
    std::fs::create_dir_all(media.join("documents/archive")).unwrap();
    std::fs::create_dir_all(media.join("documents/thumbnails")).unwrap();
    // set file names/checksums of the copy to match the generated files
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id::bigint FROM documents_document ORDER BY id LIMIT 30").fetch_all(&pool).await.unwrap();
    for id in &ids {
        let body = pdf(*id);
        std::fs::write(media.join(format!("documents/originals/g{id:07}.pdf")), &body).unwrap();
        std::fs::write(media.join(format!("documents/thumbnails/{id:07}.webp")), b"RIFF\0\0\0\0WEBPVP8 ").unwrap();
        let sha = pl_guard::backup::hex_of(&<sha2::Sha256 as sha2::Digest>::digest(&body));
        sqlx::query("UPDATE documents_document SET filename = $1, checksum = $2, archive_filename = NULL, archive_checksum = NULL WHERE id = $3")
            .bind(format!("g{id:07}.pdf"))
            .bind(sha)
            .bind(*id as i32)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("DROP TABLE IF EXISTS paperrust_guard_state, paperrust_guard_event, paperrust_guard_finding").execute(&pool).await.unwrap();
    pl_guard::store::migrate(&pool).await.unwrap();

    let cat = DbCatalog { pool: pool.clone() };
    let first = ids[0];
    let e = cat.lookup(Area::Originals, &format!("g{first:07}.pdf")).await.unwrap().unwrap();
    assert_eq!(e.doc_id, first);
    assert!(e.checksum.is_some());
    assert!(cat.lookup(Area::Thumbnails, &format!("{first:07}.webp")).await.unwrap().is_some());
    assert!(cat.lookup(Area::Originals, "unknown.pdf").await.unwrap().is_none());
    assert_eq!(cat.sample(0, 5).await.unwrap().len(), 5);

    let env = pl_core::config::Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "guard-test"),
        ("PAPERLESS_MEDIA_ROOT", media.to_str().unwrap()),
        ("PAPERLESS_DATA_DIR", data.to_str().unwrap()),
    ]);
    let cfg = pl_core::config::Config::from_env_map(env).expect("config");
    let state = pl_core::AppState::with_pool(cfg, pool.clone()).expect("state");
    let mut gcfg = GuardConfig::from_lookup(|_| None);
    gcfg.threshold = 10;
    gcfg.grace_secs = 0;
    let reporter = Arc::new(DbReporter { state: state.clone(), cfg: gcfg.clone() });
    let mut engine = Engine::new(media.clone(), gcfg, Arc::new(DbCatalog { pool: pool.clone() }), reporter.clone());
    let now = Instant::now();
    engine.scan(now).await;
    // attack on 12 originals: encrypted in place (same name)
    for id in ids.iter().take(12) {
        let p = media.join(format!("documents/originals/g{id:07}.pdf"));
        let mut d = std::fs::read(&p).unwrap();
        for (i, b) in d.iter_mut().enumerate() {
            *b ^= (i as u8).wrapping_mul(31).wrapping_add(7);
        }
        d.extend(std::iter::repeat_n(0xA5, 10));
        std::fs::write(&p, d).unwrap();
    }
    engine.scan(now).await;
    let step = engine.process_due(now + Duration::from_secs(1)).await;
    assert_eq!(step.findings, 12, "{step:?}");
    assert!(step.triggered);
    assert!(pl_guard::readonly::is_active());
    assert!(reporter.is_triggered());

    let st = pl_guard::store::state(&pool).await.unwrap();
    assert!(st.read_only);
    let ev = st.event_id.expect("event");
    let findings = pl_guard::store::findings(&pool, Some(ev), 100).await.unwrap();
    assert_eq!(findings.len(), 12);
    assert!(findings.iter().all(|f| f.reason == "modified" || f.reason == "encrypted"), "{findings:?}");
    let event = pl_guard::store::get_event(&pool, ev).await.unwrap().unwrap();
    assert_eq!((event.status.as_str(), event.files), ("open", 12));

    // tasks are rejected, backup/check continue to run
    assert!(pl_guard::readonly::task_blocked("documents.tasks.consume_file"));
    assert!(!pl_guard::readonly::task_blocked(pl_guard::TASK_BACKUP));

    // a further finding attaches to the open event
    std::fs::write(media.join("documents/originals/HOW_TO_DECRYPT.txt"), b"pay").unwrap();
    engine.note_change("documents/originals/HOW_TO_DECRYPT.txt".into(), now);
    engine.process_due(now + Duration::from_secs(2)).await;
    assert_eq!(pl_guard::store::get_event(&pool, ev).await.unwrap().unwrap().files, 13);

    // admin lifts it
    pl_guard::store::lift_read_only(&pool, Some(2), "checked, restored from snapshot").await.unwrap();
    assert!(!pl_guard::readonly::refresh(&pool).await.unwrap());
    let event = pl_guard::store::get_event(&pool, ev).await.unwrap().unwrap();
    assert_eq!(event.status, "resolved");
    assert_eq!(event.resolved_by, Some(2));
}
