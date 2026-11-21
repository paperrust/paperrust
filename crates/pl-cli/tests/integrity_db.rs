//! Integrity check (Z2) against a private DB copy and a synthetic media directory.
//!
//! Prerequisite: `PAPERRUST_INTEGRITY_TEST_DATABASE_URL` pointing to a database whose name starts
//! with `pl_integrity_test` (copy of `paperless_snapshot`, never `paperless`):
//!
//! ```sh
//! export PGPASSWORD=paperless; P=reference/pg/pgsql/bin/psql
//! $P -h 127.0.0.1 -p 54329 -U paperless -d postgres \
//!   -c "DROP DATABASE IF EXISTS pl_integrity_test WITH (FORCE)" \
//!   -c "CREATE DATABASE pl_integrity_test TEMPLATE paperless_snapshot"
//! export PAPERRUST_INTEGRITY_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_integrity_test
//! cargo test -p pl-cli -j 4 --test integrity_db -- --ignored --test-threads=1
//! ```
//!
//! The test rewrites file names/checksums of the copy to match the generated files
//! (repeatable) and compares the full and the incremental run.

use std::path::{Path, PathBuf};

use pl_cli::sanity::{self, ERROR, INFO, Options, WARNING};
use sqlx::PgPool;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("PAPERRUST_INTEGRITY_TEST_DATABASE_URL").ok()?;
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_integrity_test"), "refusing to run against database {name}");
    Some(PgPool::connect(&url).await.expect("connect"))
}

fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn orig_body(id: i32) -> Vec<u8> {
    format!("original {id}").into_bytes()
}
fn arch_body(id: i32) -> Vec<u8> {
    format!("archive {id}").into_bytes()
}

struct Setup {
    _root: tempfile::TempDir,
    ctx: pl_cli::ctx::Ctx,
    ids: Vec<i32>,
    archived: Vec<i32>,
}

impl Setup {
    fn media(&self) -> &Path {
        &self.ctx.cfg.media_root
    }
    fn orig(&self, id: i32) -> PathBuf {
        self.ctx.cfg.originals_dir.join(format!("{id:07}.pdf"))
    }
    fn thumb(&self, id: i32) -> PathBuf {
        self.ctx.cfg.thumbnail_dir.join(format!("{id:07}.webp"))
    }
}

/// Files + matching rows: every document has an original and a thumbnail, even IDs an archive,
/// IDs with `id % 10 == 3` no content.
async fn setup(pool: PgPool) -> Setup {
    let root = tempfile::tempdir().unwrap();
    let media = root.path().join("media");
    let data = root.path().join("data");
    let env = pl_core::config::Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "integrity-test"),
        ("PAPERLESS_MEDIA_ROOT", media.to_str().unwrap()),
        ("PAPERLESS_DATA_DIR", data.to_str().unwrap()),
    ]);
    let cfg = pl_core::config::Config::from_env_map(env).expect("config");
    sqlx::raw_sql(
        "UPDATE documents_document SET filename = lpad(id::text, 7, '0') || '.pdf',
           checksum = encode(sha256(convert_to('original ' || id, 'UTF8')), 'hex'),
           archive_filename = CASE WHEN id % 2 = 0 THEN lpad(id::text, 7, '0') || '.pdf' END,
           archive_checksum = CASE WHEN id % 2 = 0 THEN encode(sha256(convert_to('archive ' || id, 'UTF8')), 'hex') END,
           content = CASE WHEN id % 10 = 3 THEN '' ELSE 'text ' || id END;
         DROP TABLE IF EXISTS paperrust_integrity_state;",
    )
    .execute(&pool)
    .await
    .expect("prepare rows");
    let ids: Vec<i32> = sqlx::query_scalar("SELECT id FROM documents_document ORDER BY created DESC").fetch_all(&pool).await.unwrap();
    assert!(ids.len() >= 20, "snapshot has documents");
    for d in [&cfg.originals_dir, &cfg.archive_dir, &cfg.thumbnail_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut archived = Vec::new();
    for &id in &ids {
        std::fs::write(cfg.originals_dir.join(format!("{id:07}.pdf")), orig_body(id)).unwrap();
        std::fs::write(cfg.thumbnail_dir.join(format!("{id:07}.webp")), b"RIFF....WEBP").unwrap();
        if id % 2 == 0 {
            std::fs::write(cfg.archive_dir.join(format!("{id:07}.pdf")), arch_body(id)).unwrap();
            archived.push(id);
        }
    }
    // lock file and ignorable file are not orphaned files
    std::fs::write(&cfg.media_lock, b"").unwrap();
    std::fs::write(cfg.originals_dir.join("Thumbs.db"), b"x").unwrap();
    Setup { _root: root, ctx: pl_cli::ctx::Ctx { cfg, pool }, ids, archived }
}

/// Messages as a list, sorted (`-created` has ties in the snapshot, whose order
/// PostgreSQL – as with upstream – does not define).
fn flat(m: &sanity::Messages) -> Vec<(Option<i64>, u8, String)> {
    let mut v: Vec<_> = m.order.iter().flat_map(|pk| m.messages[pk].iter().map(move |x| (*pk, x.level, x.message.clone()))).collect();
    v.sort();
    v
}

fn incremental() -> Options {
    Options { incremental: true, record_state: true, full_hash_days: 28, threads: 3, report_keep: 0 }
}

async fn state_rows(p: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM paperrust_integrity_state").fetch_one(p).await.unwrap()
}

#[tokio::test]
#[ignore = "braucht PAPERRUST_INTEGRITY_TEST_DATABASE_URL (DB-Kopie pl_integrity_test*)"]
async fn integrity_full_vs_incremental() {
    let Some(pool) = pool().await else {
        eprintln!("SKIP: PAPERRUST_INTEGRITY_TEST_DATABASE_URL not set");
        return;
    };
    let s = setup(pool.clone()).await;
    let n = s.ids.len();
    let files = n + s.archived.len();
    let empty: Vec<i32> = s.ids.iter().copied().filter(|id| id % 10 == 3).collect();

    // 1. clean inventory: only "no OCR data" infos, in -created order
    let clean = sanity::check_sanity(&s.ctx).await.unwrap();
    let mut expected: Vec<_> = empty.iter().map(|id| (Some(i64::from(*id)), INFO, "Document contains no OCR data".to_string())).collect();
    expected.sort();
    // global messages come last, documents in -created order
    let created: std::collections::HashMap<i32, chrono::NaiveDate> =
        sqlx::query_as::<_, (i32, chrono::NaiveDate)>("SELECT id, created FROM documents_document")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .collect();
    let ts: Vec<_> = clean.order.iter().flatten().map(|id| created[&(*id as i32)]).collect();
    assert!(ts.windows(2).all(|w| w[0] >= w[1]), "documents in -created order");
    assert_eq!(flat(&clean), expected);
    assert!(!clean.has_error && !clean.has_warning);
    assert_eq!(sanity::task_summary(&clean), format!("{} document(s) with infos found.", empty.len()));

    // 2. incremental, first run: hash everything and record it
    let (m1, st1) = sanity::check_sanity_with(&s.ctx, &incremental()).await.unwrap();
    assert_eq!(flat(&m1), expected);
    assert_eq!((st1.documents, st1.files_hashed, st1.files_skipped), (n, files, 0));
    assert_eq!(state_rows(&pool).await, files as i64);

    // 3. second run: read nothing anymore, same messages
    let (m2, st2) = sanity::check_sanity_with(&s.ctx, &incremental()).await.unwrap();
    assert_eq!(flat(&m2), expected);
    assert_eq!((st2.files_hashed, st2.files_skipped, st2.bytes_hashed), (0, files, 0));

    // 4. damage: original changed (different size), archive deleted, thumbnail deleted,
    //    orphaned file; foreign state row is removed on save
    let (a, b, c) = (s.ids[1], s.archived[0], s.ids[n - 1]);
    std::fs::write(s.orig(a), b"tampered content").unwrap();
    std::fs::remove_file(s.ctx.cfg.archive_dir.join(format!("{b:07}.pdf"))).unwrap();
    std::fs::remove_file(s.thumb(c)).unwrap();
    let orphan = s.media().join("documents").join("originals").join("stray.pdf");
    std::fs::write(&orphan, b"?").unwrap();
    sqlx::query("INSERT INTO paperrust_integrity_state VALUES (-5, 'original', 'x', 1, 1, 'x', now())").execute(&pool).await.unwrap();

    let full = sanity::check_sanity(&s.ctx).await.unwrap();
    let (inc, st3) = sanity::check_sanity_with(&s.ctx, &incremental()).await.unwrap();
    assert_eq!(flat(&inc), flat(&full), "incremental reports the same as full after changes");
    assert_eq!(st3.files_hashed, 1, "only the changed original is read");
    let errors: Vec<_> = flat(&full).into_iter().filter(|x| x.1 == ERROR).collect();
    assert!(errors.contains(&(
        Some(i64::from(a)),
        ERROR,
        format!("Checksum mismatch. Stored: {}, actual: {}.", sha(&orig_body(a)), sha(b"tampered content"))
    )));
    assert!(errors.contains(&(Some(i64::from(b)), ERROR, "Archived version of document does not exist.".into())));
    assert!(errors.contains(&(Some(i64::from(c)), ERROR, "Thumbnail of document does not exist.".into())));
    assert_eq!(errors.len(), 3);
    let warnings: Vec<_> = flat(&full).into_iter().filter(|x| x.1 == WARNING).collect();
    let orphan_abs = std::path::absolute(&orphan).unwrap();
    assert_eq!(warnings, vec![(None, WARNING, format!("Orphaned file in media dir: {}", orphan_abs.display()))]);
    assert_eq!(full.global_warning_count, 1);
    assert!(sanity::task_summary(&full).ends_with("found. Check logs for details."));
    // state: deviation and missing archive removed, foreign row removed
    assert_eq!(state_rows(&pool).await, files as i64 - 2);

    // 5. original restored → confirmed again
    std::fs::write(s.orig(a), orig_body(a)).unwrap();
    let (m5, st5) = sanity::check_sanity_with(&s.ctx, &incremental()).await.unwrap();
    assert_eq!(st5.files_hashed, 1);
    assert!(!flat(&m5).iter().any(|x| x.0 == Some(i64::from(a))));
    assert_eq!(state_rows(&pool).await, files as i64 - 1);

    // 6. run(): JSON report, only the last 2 remain
    let opts = Options { report_keep: 2, ..incremental() };
    let mut last = None;
    for _ in 0..3 {
        let (_, _, p) = sanity::run(&s.ctx, &opts, "task").await.unwrap();
        last = p;
    }
    let dir = s.ctx.cfg.data_dir.join(sanity::REPORT_DIR);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    let report: serde_json::Value = serde_json::from_slice(&std::fs::read(last.expect("report path")).unwrap()).unwrap();
    assert_eq!(report["status"], "error");
    assert_eq!(report["mode"], "incremental");
    assert_eq!(report["trigger"], "task");
    assert_eq!(report["documents"], n);
    assert_eq!(report["files"]["orphaned"], 1);
    let title: String = sqlx::query_scalar("SELECT title FROM documents_document WHERE id = $1").bind(b).fetch_one(&pool).await.unwrap();
    assert!(
        report["issues"].as_array().unwrap().iter().any(|i| i["document_id"] == b
            && i["title"] == title.as_str()
            && i["message"] == "Archived version of document does not exist."),
        "{report:#}"
    );
}
