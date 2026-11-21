//! Backup → restore → comparison, against two throwaway databases (never `paperless`):
//!
//! ```sh
//! export PGPASSWORD=paperless; P=reference/pg/pgsql/bin/psql
//! for d in pl_guard_test pl_guard_test_target; do
//!   $P -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "DROP DATABASE IF EXISTS $d WITH (FORCE)" \
//!      -c "CREATE DATABASE $d TEMPLATE paperless_snapshot"; done
//! export PAPERRUST_GUARD_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_guard_test
//! export PAPERRUST_GUARD_TEST_TARGET_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_guard_test_target
//! cargo test -p pl-guard -j 2 --test backup_roundtrip -- --ignored --test-threads=1
//! ```
//!
//! The target database has the same schema (as after `manage migrate`); the restore empties
//! all tables and restores the dump. All tables are compared row by row (md5
//! over all rows) and all media files (SHA-256).

#[path = "support/s3mock.rs"]
mod s3mock;

use std::collections::BTreeMap;
use std::path::Path;

use pl_guard::backup::{self, RestoreOptions, Sources};
use pl_guard::config::{BackupConfig, LockMode, Target};
use sqlx::PgPool;

async fn pool(var: &str) -> Option<PgPool> {
    let url = std::env::var(var).ok()?;
    let name = url.rsplit('/').next().unwrap_or_default().split('?').next().unwrap_or_default().to_string();
    assert!(name.starts_with("pl_guard_test"), "refusing to run against database {name}");
    Some(PgPool::connect(&url).await.expect("connect"))
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

fn media(root: &Path) {
    for i in 0..25 {
        let mut d = format!("%PDF-1.7 doc {i}\n").into_bytes();
        d.extend((0..3000u32).map(|x| (x.wrapping_mul(2654435761).wrapping_add(i) >> 13) as u8));
        write(root, &format!("documents/originals/2024/Brief {i:02} – Müller.pdf"), &d);
        write(root, &format!("documents/archive/2024/Brief {i:02} – Müller.pdf"), &d[..d.len() / 2]);
        write(root, &format!("documents/thumbnails/{i:07}.webp"), format!("RIFF....WEBP{i}").as_bytes());
    }
    // same content twice: one object
    write(root, "documents/originals/dup-a.txt", b"same content");
    write(root, "documents/originals/dup-b.txt", b"same content");
    write(root, "media.lock", b"");
}

fn hashes(root: &Path) -> BTreeMap<String, String> {
    fn rec(root: &Path, d: &Path, out: &mut BTreeMap<String, String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                rec(root, &p, out);
            } else {
                let rel = pl_guard::guard::rel_path(root, &p).unwrap();
                if rel != "media.lock" {
                    out.insert(rel, pl_guard::guard::sha256_file(&p).unwrap());
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    rec(root, root, &mut out);
    out
}

async fn table_digests(pool: &PgPool) -> BTreeMap<String, (i64, String)> {
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT tablename::text FROM pg_tables WHERE schemaname = 'public' ORDER BY 1").fetch_all(pool).await.unwrap();
    let mut out = BTreeMap::new();
    for t in tables {
        // run data of the backup itself changes between dump and comparison
        if t.starts_with("paperrust_backup_") {
            continue;
        }
        let q = format!("SELECT count(*)::bigint, coalesce(md5(string_agg(x::text, '|' ORDER BY x::text)), '') FROM public.\"{t}\" x");
        let (n, h): (i64, String) = sqlx::query_as(&q).fetch_one(pool).await.unwrap();
        out.insert(t, (n, h));
    }
    out
}

fn cfg(t: Target) -> BackupConfig {
    BackupConfig { target: Some(t), database: true, keep_last: 10, keep_daily: 7, keep_weekly: 4, keep_monthly: 12 }
}

async fn roundtrip(target: Target, src: PgPool, dst: PgPool) {
    let work = tempfile::tempdir().unwrap();
    let media_root = work.path().join("media");
    let data_dir = work.path().join("data");
    media(&media_root);
    // target DB as after `manage migrate`: the own tables must exist as well
    pl_guard::store::migrate(&dst).await.unwrap();
    let pass = "correct horse battery staple";
    backup::init(&target, &data_dir, pass, Some(10)).await.unwrap();
    let sources = Sources { media_root: media_root.clone(), data_dir: data_dir.clone() };
    let c = cfg(target.clone());

    // 1. first run: everything new (76 files, 75 objects + DB)
    let r1 = backup::run(&src, &sources, &c, "manual").await.unwrap();
    assert_eq!(r1.status, "ok", "{}", r1.error);
    assert_eq!(r1.files_total, 77, "media.lock wird nicht gesichert");
    assert_eq!(r1.files_new, 76, "zwei gleiche Dateien = ein Objekt");
    assert!(r1.db_bytes > 0);
    // 2. second run without changes: nothing new transferred
    let r2 = backup::run(&src, &sources, &c, "manual").await.unwrap();
    assert_eq!(r2.files_new, 0, "dedupliziert");
    // 3. one file changed: exactly one new object
    write(&media_root, "documents/originals/dup-b.txt", b"changed content");
    let r3 = backup::run(&src, &sources, &c, "manual").await.unwrap();
    assert_eq!(r3.files_new, 1);
    let snaps = backup::list_snapshots(&target).await.unwrap();
    assert_eq!(snaps.len(), 3);
    assert_eq!(snaps[0].0, r3.snapshot.clone().unwrap(), "neueste zuerst");

    // contents at the target are encrypted
    let id = backup::identity(&target, Some(pass), None).await.unwrap();
    let (n, missing) = backup::check(&target, &id, "latest").await.unwrap();
    assert!(n >= 76 && missing.is_empty(), "{n} {missing:?}");

    // 4. restore into a new media folder and the second database
    let restored = work.path().join("restored");
    let st =
        backup::restore(&target, &id, &RestoreOptions { snapshot: "latest", media_dir: Some(&restored), db: Some(&dst), overwrite: false })
            .await
            .unwrap();
    assert_eq!(st.files, 77);
    assert!(st.db_tables.unwrap() > 50);
    assert_eq!(hashes(&restored), hashes(&media_root), "Medien Bit für Bit gleich");
    let a = table_digests(&src).await;
    let b = table_digests(&dst).await;
    assert_eq!(a.len(), b.len());
    for (t, v) in &a {
        assert_eq!(Some(v), b.get(t), "Tabelle {t} weicht ab");
    }
    // sequences reset: the next id is free
    let free: bool = sqlx::query_scalar(
        "SELECT nextval(pg_get_serial_sequence('documents_tag', 'id')) > (SELECT coalesce(max(id), 0) FROM documents_tag)",
    )
    .fetch_one(&dst)
    .await
    .unwrap();
    assert!(free);

    // 5. restore an older state specifically: dup-b.txt has the old content there
    let old = work.path().join("restored-old");
    backup::restore(&target, &id, &RestoreOptions { snapshot: &snaps[2].0, media_dir: Some(&old), db: None, overwrite: false })
        .await
        .unwrap();
    assert_eq!(std::fs::read(old.join("documents/originals/dup-b.txt")).unwrap(), b"same content");
    // existing, differing file is not silently overwritten
    assert!(
        backup::restore(&target, &id, &RestoreOptions { snapshot: "latest", media_dir: Some(&old), db: None, overwrite: false })
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "braucht PAPERRUST_GUARD_TEST_DATABASE_URL und PAPERRUST_GUARD_TEST_TARGET_DATABASE_URL (Wegwerf-DBs, siehe Kopf)"]
async fn local_target_roundtrip() {
    let (Some(src), Some(dst)) = (pool("PAPERRUST_GUARD_TEST_DATABASE_URL").await, pool("PAPERRUST_GUARD_TEST_TARGET_DATABASE_URL").await)
    else {
        eprintln!("skipped: database URLs not set");
        return;
    };
    sqlx::query("DROP TABLE IF EXISTS paperrust_backup_object, paperrust_backup_filecache, paperrust_backup_run")
        .execute(&src)
        .await
        .unwrap();
    pl_guard::store::migrate(&src).await.unwrap();
    let t = tempfile::tempdir().unwrap();
    roundtrip(Target::Local(t.path().join("backup")), src, dst).await;
    // no plaintext PDFs at the target
    let mut plain = 0;
    for e in walk(&t.path().join("backup")) {
        if std::fs::read(&e).unwrap().windows(8).any(|w| w == b"%PDF-1.7") {
            plain += 1;
        }
    }
    assert_eq!(plain, 0);
}

#[tokio::test]
#[ignore = "braucht PAPERRUST_GUARD_TEST_DATABASE_URL und PAPERRUST_GUARD_TEST_TARGET_DATABASE_URL (Wegwerf-DBs, siehe Kopf)"]
async fn s3_target_roundtrip_with_object_lock() {
    let (Some(src), Some(dst)) = (pool("PAPERRUST_GUARD_TEST_DATABASE_URL").await, pool("PAPERRUST_GUARD_TEST_TARGET_DATABASE_URL").await)
    else {
        eprintln!("skipped: database URLs not set");
        return;
    };
    sqlx::query("DROP TABLE IF EXISTS paperrust_backup_object, paperrust_backup_filecache, paperrust_backup_run")
        .execute(&src)
        .await
        .unwrap();
    pl_guard::store::migrate(&src).await.unwrap();
    let (ep, state) = s3mock::start().await;
    let target = s3mock::target(&ep, "nas", LockMode::Compliance);
    roundtrip(target.clone(), src, dst).await;
    let g = state.lock().unwrap();
    assert_eq!(g.bad_signatures, 0);
    let locked = g.objects.iter().filter(|(k, o)| k.starts_with("nas/objects/") && o.retain_until.is_some()).count();
    assert!(locked >= 76, "alle Objekte gesperrt: {locked}");
    assert!(g.objects.keys().any(|k| k.starts_with("nas/snapshots/")));
}

fn walk(d: &Path) -> Vec<std::path::PathBuf> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(d).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            v.extend(walk(&p));
        } else {
            v.push(p);
        }
    }
    v
}
