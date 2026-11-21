//! Automatic, encrypted, deduplicated backups with versions.
//!
//! * **Content-addressed:** every file under `MEDIA_ROOT` is stored as object `objects/ab/<sha256>`
//!   (name = SHA-256 of the plaintext). Unchanged files are never transferred again;
//!   whether a file has to be re-hashed is decided by a cache (path, size,
//!   modification time → checksum, table `paperrust_backup_filecache`).
//! * **Database:** dump via `COPY` ([`dbdump`]), gzip, likewise as an object (same state →
//!   same object).
//! * **Directory per run** (`snapshots/<time>.age`): list path → checksum, gzip + age.
//! * **Encryption** before upload with age ([`crypto`]); for backing up the public
//!   key is enough, for restoring the passphrase is needed.
//! * **Retention** ([`retention`]) and **S3 Object Lock** (retention period per object, extension
//!   for objects that continue to be used).

pub mod crypto;
pub mod dbdump;
pub mod retention;
pub mod s3;
pub mod target;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::config::BackupConfig;
use crate::guard::{hex, sha256_file};
use crate::store::{self, BackupRun};
use target::Store;

pub const FORMAT: u32 = 1;

/// `config.json` at the target (plaintext, no secret).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoConfig {
    pub format: u32,
    pub tool: String,
    pub created: DateTime<Utc>,
    /// public age key (`age1…`)
    pub recipient: String,
}

/// Local copy (`DATA_DIR/backup/repo.json`): detects a replaced `config.json` at the target.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalRepo {
    target: String,
    recipient: String,
}

/// Header of a directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotHeader {
    pub format: u32,
    pub id: String,
    pub created: DateTime<Utc>,
    pub version: String,
    pub files: u64,
    pub bytes: u64,
    pub db: Option<DbEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbEntry {
    pub object: String,
    pub size: u64,
    pub tables: usize,
}

/// One backed-up file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub sha256: String,
    pub size: u64,
    pub mtime_ns: i64,
    /// relative to `MEDIA_ROOT`, with `/`
    pub path: String,
}

/// Complete directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub header: SnapshotHeader,
    pub files: Vec<FileEntry>,
}

impl Snapshot {
    /// text form (before gzip): first line JSON header, then per file `sha\tsize\tmtime\tpath`.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
        let w = |gz: &mut flate2::write::GzEncoder<Vec<u8>>, s: &str| gz.write_all(s.as_bytes()).map_err(|e| e.to_string());
        w(&mut gz, &serde_json::to_string(&self.header).map_err(|e| e.to_string())?)?;
        w(&mut gz, "\n")?;
        for f in &self.files {
            w(&mut gz, &format!("{}\t{}\t{}\t{}\n", f.sha256, f.size, f.mtime_ns, f.path.replace(['\t', '\n'], " ")))?;
        }
        gz.finish().map_err(|e| e.to_string())
    }

    pub fn decode(gz: &[u8]) -> Result<Self, String> {
        let r = std::io::BufReader::new(flate2::read::GzDecoder::new(gz));
        let mut lines = r.lines();
        let head = lines.next().ok_or("empty snapshot")?.map_err(|e| e.to_string())?;
        let header: SnapshotHeader = serde_json::from_str(&head).map_err(|e| format!("snapshot header: {e}"))?;
        let mut files = Vec::new();
        for l in lines {
            let l = l.map_err(|e| e.to_string())?;
            if l.is_empty() {
                continue;
            }
            let mut p = l.splitn(4, '\t');
            let (Some(sha), Some(size), Some(mt), Some(path)) = (p.next(), p.next(), p.next(), p.next()) else {
                return Err("damaged snapshot line".into());
            };
            files.push(FileEntry {
                sha256: sha.to_string(),
                size: size.parse().map_err(|_| "damaged snapshot size")?,
                mtime_ns: mt.parse().map_err(|_| "damaged snapshot mtime")?,
                path: path.to_string(),
            });
        }
        Ok(Self { header, files })
    }
}

pub fn object_key(sha: &str) -> String {
    format!("objects/{}/{sha}", &sha[..2.min(sha.len())])
}

fn snapshot_key(id: &str) -> String {
    format!("snapshots/{id}.age")
}

/// Time from a directory id (`20261114T021500Z`, optionally with `-n`).
pub fn snapshot_time(id: &str) -> Option<DateTime<Utc>> {
    let base = id.split('-').next().unwrap_or(id);
    chrono::NaiveDateTime::parse_from_str(base, "%Y%m%dT%H%M%SZ").ok().map(|n| n.and_utc())
}

fn local_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("backup")
}

async fn read_repo_config(store: &Store) -> Result<RepoConfig, String> {
    let raw = store.get("config.json").await?.ok_or("backup target is not initialized (run `paperrust manage backup init`)")?;
    let c: RepoConfig = serde_json::from_slice(&raw).map_err(|e| format!("config.json: {e}"))?;
    if c.format != FORMAT {
        return Err(format!("unsupported backup format {}", c.format));
    }
    Ok(c)
}

/// Set up the target. Returns (public key, secret key for safekeeping).
pub async fn init(
    target: &crate::config::Target,
    data_dir: &Path,
    passphrase: &str,
    work_factor: Option<u8>,
) -> Result<(String, String), String> {
    use age::secrecy::ExposeSecret;
    if passphrase.chars().count() < 12 {
        return Err("the passphrase must have at least 12 characters".into());
    }
    let store = Store::open(target)?;
    if store.exists("config.json").await? {
        return Err("backup target is already initialized".into());
    }
    let (secret, public) = crypto::generate();
    let wrapped = crypto::wrap_identity(&secret, passphrase, work_factor)?;
    let until = store.retain_until(Utc::now());
    store.put("keys/repo.key.age", wrapped, until).await?;
    let cfg = RepoConfig {
        format: FORMAT,
        tool: format!("{} {}", pl_core::brand::PRODUCT_NAME, pl_core::brand::PAPERRUST_VERSION),
        created: Utc::now(),
        recipient: public.clone(),
    };
    store.put("config.json", serde_json::to_vec_pretty(&cfg).map_err(|e| e.to_string())?, None).await?;
    write_local_repo(data_dir, &target.display(), &public)?;
    Ok((public, secret.expose_secret().to_string()))
}

fn write_local_repo(data_dir: &Path, target: &str, recipient: &str) -> Result<(), String> {
    let dir = local_dir(data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let v = serde_json::to_vec_pretty(&LocalRepo { target: target.into(), recipient: recipient.into() }).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("repo.json"), v).map_err(|e| e.to_string())
}

/// Verify the public key: must match the local copy (otherwise someone with
/// write access to the target could redirect future backups to their own key).
fn check_recipient(data_dir: &Path, target: &str, remote: &str) -> Result<(), String> {
    let p = local_dir(data_dir).join("repo.json");
    match std::fs::read(&p) {
        Ok(raw) => {
            let l: LocalRepo = serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", p.display()))?;
            if l.target == target && l.recipient != remote {
                return Err("the public key in config.json at the backup target changed; refusing to back up (possible tampering). \
                            If this is intended, delete DATA_DIR/backup/repo.json"
                    .into());
            }
            if l.target != target {
                write_local_repo(data_dir, target, remote)?;
            }
            Ok(())
        }
        Err(_) => write_local_repo(data_dir, target, remote),
    }
}

/// Where and what is backed up.
#[derive(Debug, Clone)]
pub struct Sources {
    pub media_root: PathBuf,
    pub data_dir: PathBuf,
}

fn mtime_ns(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

fn walk_media(root: &Path) -> Vec<(String, u64, i64)> {
    fn rec(dir: &Path, root: &Path, out: &mut Vec<(String, u64, i64)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                rec(&p, root, out);
            } else if ft.is_file()
                && let Some(rel) = crate::guard::rel_path(root, &p)
            {
                if crate::detect::is_system_file(&rel) {
                    continue;
                }
                if let Ok(m) = e.metadata() {
                    out.push((rel, m.len(), mtime_ns(&m)));
                }
            }
        }
    }
    let mut out = Vec::new();
    rec(root, root, &mut out);
    out.sort();
    out
}

/// Checksums from the cache (only if size and modification time are equal).
async fn cached_hashes(pool: &PgPool, chunk: &[(String, u64, i64)]) -> Result<HashMap<String, String>, sqlx::Error> {
    let paths: Vec<&str> = chunk.iter().map(|(p, _, _)| p.as_str()).collect();
    let rows = sqlx::query("SELECT path, size, mtime_ns, sha256 FROM paperrust_backup_filecache WHERE path = ANY($1)")
        .bind(&paths)
        .fetch_all(pool)
        .await?;
    let want: HashMap<&str, (u64, i64)> = chunk.iter().map(|(p, s, m)| (p.as_str(), (*s, *m))).collect();
    let mut out = HashMap::new();
    for r in rows {
        let p: String = r.try_get("path")?;
        let s: i64 = r.try_get("size")?;
        let m: i64 = r.try_get("mtime_ns")?;
        let h: String = r.try_get("sha256")?;
        if want.get(p.as_str()) == Some(&(s as u64, m)) {
            out.insert(p, h);
        }
    }
    Ok(out)
}

async fn known_objects(pool: &PgPool, target: &str, ids: &[String]) -> Result<HashMap<String, Option<DateTime<Utc>>>, sqlx::Error> {
    let rows = sqlx::query("SELECT id::text AS id, retain_until FROM paperrust_backup_object WHERE target = $1 AND id = ANY($2)")
        .bind(target)
        .bind(ids)
        .fetch_all(pool)
        .await?;
    rows.iter().map(|r| Ok((r.try_get::<String, _>("id")?.trim().to_string(), r.try_get("retain_until")?))).collect()
}

async fn remember_object(pool: &PgPool, target: &str, id: &str, size: u64, until: Option<DateTime<Utc>>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO paperrust_backup_object (target, id, size, retain_until) VALUES ($1, $2, $3, $4)
         ON CONFLICT (target, id) DO UPDATE SET retain_until = GREATEST(paperrust_backup_object.retain_until, EXCLUDED.retain_until)",
    )
    .bind(target)
    .bind(id)
    .bind(size as i64)
    .bind(until)
    .execute(pool)
    .await?;
    Ok(())
}

/// Encrypt and upload an object (if not yet present at the target).
async fn upload_object(
    store: &Store,
    recipient: &age::x25519::Recipient,
    src: &Path,
    sha: &str,
    tmp_dir: &Path,
    until: Option<DateTime<Utc>>,
) -> Result<u64, String> {
    let key = object_key(sha);
    // do not upload twice after loss of local knowledge (new DB)
    if store.exists(&key).await? {
        return Ok(0);
    }
    let tmp = tmp_dir.join(format!("{sha}.age.tmp"));
    let (r, s, t) = (recipient.clone(), src.to_path_buf(), tmp.clone());
    let (n, got) = tokio::task::spawn_blocking(move || crypto::encrypt_file(&r, &s, &t)).await.map_err(|e| e.to_string())??;
    if got != sha {
        // file changed since hashing: do not store under a wrong name
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{} changed during the backup; run it again", src.display()));
    }
    let res = store.put_file(&key, &tmp, until).await;
    let _ = std::fs::remove_file(&tmp);
    res.map(|_| n)
}

/// One run: back up database + media, write the directory, clean up old states.
pub async fn run(pool: &PgPool, src: &Sources, cfg: &BackupConfig, trigger: &str) -> Result<BackupRun, String> {
    let target = cfg.target.clone().ok_or("no backup target configured (PAPERRUST_BACKUP_TARGET)")?;
    store::ensure(pool).await.map_err(|e| e.to_string())?;
    // only one run at a time (also across processes)
    // Own connection detached from the pool (BEFUNDE B2, like pl-mail fetch/tasks): it holds the
    // lock for the whole run without taking a connection away from the pool; on an error
    // in the middle the lock ends with the closed connection instead of staying attached
    // to a pool connection.
    let mut lock_conn = pool.acquire().await.map_err(|e| e.to_string())?.detach();
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('paperrust_backup_run'))")
        .fetch_one(&mut lock_conn)
        .await
        .map_err(|e| e.to_string())?;
    if !got {
        return Err("another backup is running".into());
    }
    let tname = target.display();
    let id = store::start_run(pool, trigger, &tname).await.map_err(|e| e.to_string())?;
    let mut rec = BackupRun { id, status: "running".into(), trigger: trigger.into(), target: tname.clone(), ..Default::default() };
    let res = run_inner(pool, src, cfg, &target, &mut rec).await;
    match &res {
        Ok(()) => rec.status = "ok".into(),
        Err(e) => {
            rec.status = "error".into();
            rec.error = e.chars().take(2000).collect();
        }
    }
    if let Err(e) = store::finish_run(pool, &rec).await {
        tracing::error!(target: "paperless.backup", error = %e, "could not store backup run");
    }
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('paperrust_backup_run'))").execute(&mut lock_conn).await;
    let _ = sqlx::Connection::close(lock_conn).await;
    match res {
        Ok(()) => {
            tracing::info!(
                target: "paperless.backup",
                snapshot = rec.snapshot.as_deref().unwrap_or(""),
                files = rec.files_total,
                new_files = rec.files_new,
                new_bytes = rec.bytes_new,
                pruned = rec.pruned,
                "backup finished"
            );
            Ok(rec)
        }
        Err(e) => {
            tracing::error!(target: "paperless.backup", error = %e, "backup failed");
            Err(e)
        }
    }
}

async fn run_inner(
    pool: &PgPool,
    src: &Sources,
    cfg: &BackupConfig,
    target: &crate::config::Target,
    rec: &mut BackupRun,
) -> Result<(), String> {
    let store = Store::open(target)?;
    let tname = target.display();
    let repo = read_repo_config(&store).await?;
    check_recipient(&src.data_dir, &tname, &repo.recipient)?;
    let recipient = crypto::parse_recipient(&repo.recipient)?;
    let tmp_dir = local_dir(&src.data_dir).join("tmp");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("{}: {e}", tmp_dir.display()))?;
    let now = Utc::now();
    let until = store.retain_until(now);
    let lock_days = store.lock_days();
    // objects whose lock expires in less than half the period are extended
    let extend_before = (lock_days > 0).then(|| now + chrono::Duration::days((lock_days / 2).max(1)));
    let mut warnings: Vec<String> = Vec::new();

    // 1. database
    let mut db_entry = None;
    if cfg.database {
        let dump = tmp_dir.join(format!("db-{}.gz", rec.id));
        let stats = dbdump::dump(pool, &dump).await?;
        let d2 = dump.clone();
        let sha = tokio::task::spawn_blocking(move || sha256_file(&d2)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let size = std::fs::metadata(&dump).map(|m| m.len()).unwrap_or(0);
        let known = known_objects(pool, &tname, std::slice::from_ref(&sha)).await.map_err(|e| e.to_string())?;
        if !known.contains_key(&sha) {
            let n = upload_object(&store, &recipient, &dump, &sha, &tmp_dir, until).await?;
            if n > 0 {
                rec.bytes_new += n as i64;
            }
            remember_object(pool, &tname, &sha, size, until).await.map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_file(&dump);
        rec.db_bytes = size as i64;
        db_entry = Some(DbEntry { object: sha, size, tables: stats.tables });
    }

    // 2. media
    let root = src.media_root.clone();
    let files = tokio::task::spawn_blocking(move || walk_media(&root)).await.map_err(|e| e.to_string())?;
    let mut entries: Vec<FileEntry> = Vec::with_capacity(files.len());
    for chunk in files.chunks(500) {
        let start = entries.len();
        let cached = cached_hashes(pool, chunk).await.map_err(|e| e.to_string())?;
        let mut hashed: Vec<(String, u64, i64, String)> = Vec::new();
        for (path, size, mt) in chunk {
            let sha = match cached.get(path) {
                Some(h) => h.clone(),
                None => {
                    let abs = src.media_root.join(path);
                    match tokio::task::spawn_blocking(move || sha256_file(&abs)).await.map_err(|e| e.to_string())? {
                        Ok(h) => {
                            hashed.push((path.clone(), *size, *mt, h.clone()));
                            h
                        }
                        Err(e) => {
                            // file disappeared/locked during the run: do not abort the run
                            warnings.push(format!("{path}: {e}"));
                            continue;
                        }
                    }
                }
            };
            entries.push(FileEntry { sha256: sha, size: *size, mtime_ns: *mt, path: path.clone() });
        }
        for (p, s, m, h) in &hashed {
            sqlx::query(
                "INSERT INTO paperrust_backup_filecache (path, size, mtime_ns, sha256) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (path) DO UPDATE SET size = EXCLUDED.size, mtime_ns = EXCLUDED.mtime_ns, sha256 = EXCLUDED.sha256",
            )
            .bind(p)
            .bind(*s as i64)
            .bind(*m)
            .bind(h)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        // objects of this section
        let ids: Vec<String> = entries[start..].iter().map(|e| e.sha256.clone()).collect::<HashSet<_>>().into_iter().collect();
        let known = known_objects(pool, &tname, &ids).await.map_err(|e| e.to_string())?;
        let mut done: HashSet<String> = HashSet::new();
        for e in &entries[start..] {
            if !done.insert(e.sha256.clone()) {
                continue;
            }
            match known.get(&e.sha256) {
                None => {
                    let abs = src.media_root.join(&e.path);
                    // content changed since hashing? Then with a new hash on the next run.
                    let n = upload_object(&store, &recipient, &abs, &e.sha256, &tmp_dir, until).await?;
                    if n > 0 {
                        rec.files_new += 1;
                        rec.bytes_new += n as i64;
                    }
                    remember_object(pool, &tname, &e.sha256, e.size, until).await.map_err(|e| e.to_string())?;
                }
                Some(old) => {
                    if let (Some(before), Some(new_until)) = (extend_before, until)
                        && old.is_none_or(|t| t < before)
                    {
                        store.extend_retention(&object_key(&e.sha256), new_until).await?;
                        remember_object(pool, &tname, &e.sha256, e.size, Some(new_until)).await.map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }
    if let (Some(db), Some(before), Some(new_until)) = (&db_entry, extend_before, until) {
        let k = known_objects(pool, &tname, std::slice::from_ref(&db.object)).await.map_err(|e| e.to_string())?;
        if k.get(&db.object).is_some_and(|t| t.is_none_or(|t| t < before)) {
            store.extend_retention(&object_key(&db.object), new_until).await?;
            remember_object(pool, &tname, &db.object, db.size, Some(new_until)).await.map_err(|e| e.to_string())?;
        }
    }

    // 3. directory
    let mut sid = now.format("%Y%m%dT%H%M%SZ").to_string();
    let existing: HashSet<String> = store.list("snapshots/").await?.into_iter().collect();
    let mut n = 1;
    while existing.contains(&snapshot_key(&sid)) {
        n += 1;
        sid = format!("{}-{n}", now.format("%Y%m%dT%H%M%SZ"));
    }
    rec.files_total = entries.len() as i64;
    rec.bytes_total = entries.iter().map(|e| e.size as i64).sum();
    let snap = Snapshot {
        header: SnapshotHeader {
            format: FORMAT,
            id: sid.clone(),
            created: now,
            version: pl_core::brand::PAPERRUST_VERSION.to_string(),
            files: entries.len() as u64,
            bytes: rec.bytes_total as u64,
            db: db_entry,
        },
        files: entries,
    };
    let plain = snap.encode()?;
    let enc = crypto::encrypt_bytes(&recipient, &plain)?;
    store.put(&snapshot_key(&sid), enc, until).await?;
    // local plaintext copy (for cleanup without the passphrase)
    let ldir = local_dir(&src.data_dir).join("snapshots");
    std::fs::create_dir_all(&ldir).map_err(|e| e.to_string())?;
    std::fs::write(ldir.join(format!("{sid}.gz")), &plain).map_err(|e| e.to_string())?;
    rec.snapshot = Some(sid);

    // 4. cleanup (never in read-only mode: otherwise good states could be displaced by encrypted ones)
    if crate::readonly::is_active() {
        warnings.push("read-only mode: old backups were not pruned".into());
    } else {
        match prune(pool, &store, &tname, &src.data_dir, cfg).await {
            Ok(p) => {
                rec.pruned = p.snapshots as i32;
                warnings.extend(p.warnings);
            }
            Err(e) => warnings.push(format!("prune: {e}")),
        }
    }
    if !warnings.is_empty() {
        rec.warnings = warnings.iter().take(50).cloned().collect::<Vec<_>>().join("\n");
    }
    Ok(())
}

/// Result of the cleanup.
#[derive(Debug, Default)]
pub struct PruneStats {
    pub snapshots: usize,
    pub objects: usize,
    pub warnings: Vec<String>,
}

/// Delete old directories according to the retention rule, then objects no longer in use.
pub async fn prune(pool: &PgPool, store: &Store, tname: &str, data_dir: &Path, cfg: &BackupConfig) -> Result<PruneStats, String> {
    let mut st = PruneStats::default();
    let keys = store.list("snapshots/").await?;
    let ids: Vec<String> = keys.iter().filter_map(|k| k.strip_prefix("snapshots/")?.strip_suffix(".age").map(str::to_string)).collect();
    let timed: Vec<(String, DateTime<Utc>)> = ids.iter().filter_map(|id| snapshot_time(id).map(|t| (id.clone(), t))).collect();
    let times: Vec<DateTime<Utc>> = timed.iter().map(|(_, t)| *t).collect();
    let keep = retention::keep(&times, cfg.keep_last, cfg.keep_daily, cfg.keep_weekly, cfg.keep_monthly);
    let ldir = local_dir(data_dir).join("snapshots");
    let mut remaining: Vec<String> = Vec::new();
    for (i, (id, _)) in timed.iter().enumerate() {
        if keep.contains(&i) {
            remaining.push(id.clone());
            continue;
        }
        if store.delete(&snapshot_key(id)).await? {
            st.snapshots += 1;
            let _ = std::fs::remove_file(ldir.join(format!("{id}.gz")));
        } else {
            // retention period still running: try again on the next run
            remaining.push(id.clone());
        }
    }
    // objects: only if a local copy exists for every remaining directory
    let mut referenced: HashSet<String> = HashSet::new();
    for id in &remaining {
        let p = ldir.join(format!("{id}.gz"));
        match std::fs::read(&p) {
            Ok(raw) => {
                let s = Snapshot::decode(&raw)?;
                referenced.extend(s.files.into_iter().map(|f| f.sha256));
                if let Some(db) = s.header.db {
                    referenced.insert(db.object);
                }
            }
            Err(_) => {
                st.warnings.push(format!("no local copy of snapshot {id}; unused objects are kept"));
                return Ok(st);
            }
        }
    }
    let known: Vec<String> = sqlx::query_scalar::<_, String>("SELECT id::text FROM paperrust_backup_object WHERE target = $1")
        .bind(tname)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;
    let now = Utc::now();
    for id in known.into_iter().map(|s| s.trim().to_string()) {
        if referenced.contains(&id) {
            continue;
        }
        let until: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT retain_until FROM paperrust_backup_object WHERE target = $1 AND id = $2")
                .bind(tname)
                .bind(&id)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?
                .flatten();
        if until.is_some_and(|u| u > now) {
            continue;
        }
        if store.delete(&object_key(&id)).await? {
            sqlx::query("DELETE FROM paperrust_backup_object WHERE target = $1 AND id = $2")
                .bind(tname)
                .bind(&id)
                .execute(pool)
                .await
                .map_err(|e| e.to_string())?;
            st.objects += 1;
        }
    }
    Ok(st)
}

/// Directories at the target (id, time), newest first.
pub async fn list_snapshots(target: &crate::config::Target) -> Result<Vec<(String, DateTime<Utc>)>, String> {
    let store = Store::open(target)?;
    let mut v: Vec<(String, DateTime<Utc>)> = store
        .list("snapshots/")
        .await?
        .iter()
        .filter_map(|k| k.strip_prefix("snapshots/")?.strip_suffix(".age").map(str::to_string))
        .filter_map(|id| snapshot_time(&id).map(|t| (id, t)))
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    Ok(v)
}

/// Key of the target: from a key file or with the passphrase.
pub async fn identity(
    target: &crate::config::Target,
    passphrase: Option<&str>,
    key_file: Option<&Path>,
) -> Result<age::x25519::Identity, String> {
    if let Some(p) = key_file {
        let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        return crypto::parse_identity(&s);
    }
    let pass = passphrase.ok_or("passphrase required (PAPERRUST_BACKUP_PASSPHRASE or --key-file)")?;
    let store = Store::open(target)?;
    let wrapped = store.get("keys/repo.key.age").await?.ok_or("keys/repo.key.age missing at the backup target")?;
    let pass = pass.to_string();
    tokio::task::spawn_blocking(move || crypto::unwrap_identity(&wrapped, &pass)).await.map_err(|e| e.to_string())?
}

/// Load and decrypt a directory (`latest` = newest).
pub async fn load_snapshot(target: &crate::config::Target, identity: &age::x25519::Identity, id: &str) -> Result<Snapshot, String> {
    let id = if id == "latest" {
        list_snapshots(target).await?.first().map(|(i, _)| i.clone()).ok_or("no snapshot at the backup target")?
    } else {
        id.to_string()
    };
    let store = Store::open(target)?;
    let enc = store.get(&snapshot_key(&id)).await?.ok_or_else(|| format!("snapshot {id} not found"))?;
    let plain = crypto::decrypt_bytes(identity, &enc)?;
    Snapshot::decode(&plain)
}

/// Result of a restore.
#[derive(Debug, Default, Clone)]
pub struct RestoreStats {
    pub snapshot: String,
    pub files: u64,
    pub bytes: u64,
    pub skipped_existing: u64,
    pub db_tables: Option<usize>,
}

/// What is restored.
pub struct RestoreOptions<'a> {
    pub snapshot: &'a str,
    /// media to here (empty or new folder); `None` = no media
    pub media_dir: Option<&'a Path>,
    /// restore the database here (migrated, will be replaced); `None` = no database
    pub db: Option<&'a PgPool>,
    /// overwrite existing, differing files
    pub overwrite: bool,
}

/// Restore; each file is checked against its checksum after decryption.
pub async fn restore(
    target: &crate::config::Target,
    identity: &age::x25519::Identity,
    opts: &RestoreOptions<'_>,
) -> Result<RestoreStats, String> {
    let snap = load_snapshot(target, identity, opts.snapshot).await?;
    let store = Store::open(target)?;
    let mut st = RestoreStats { snapshot: snap.header.id.clone(), ..Default::default() };
    if let Some(dir) = opts.media_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for f in &snap.files {
            if f.path.split('/').any(|c| c == ".." || c.is_empty()) || f.path.contains('\\') || f.path.contains(':') {
                return Err(format!("unsafe path in snapshot: {}", f.path));
            }
            let dest = f.path.split('/').fold(dir.to_path_buf(), |p, c| p.join(c));
            if dest.exists() {
                let d2 = dest.clone();
                let have =
                    tokio::task::spawn_blocking(move || sha256_file(&d2)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
                if have == f.sha256 {
                    st.skipped_existing += 1;
                    continue;
                }
                if !opts.overwrite {
                    return Err(format!("{} exists with different content (use --overwrite)", dest.display()));
                }
            }
            let enc =
                store.get(&object_key(&f.sha256)).await?.ok_or_else(|| format!("object for {} missing at the backup target", f.path))?;
            let (id, want, d) = (identity.clone(), f.sha256.clone(), dest.clone());
            let n = tokio::task::spawn_blocking(move || -> Result<u64, String> {
                if let Some(parent) = d.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let tmp = d.with_file_name(format!(
                    ".{}.restore.paperrust-tmp",
                    d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                ));
                let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| e.to_string())?);
                let n = crypto::decrypt_stream(&id, enc.as_slice(), &mut out)?;
                out.flush().map_err(|e| e.to_string())?;
                drop(out);
                let got = sha256_file(&tmp).map_err(|e| e.to_string())?;
                if got != want {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(format!("checksum mismatch after restore: {}", d.display()));
                }
                std::fs::rename(&tmp, &d).map_err(|e| e.to_string())?;
                Ok(n)
            })
            .await
            .map_err(|e| e.to_string())??;
            st.files += 1;
            st.bytes += n;
        }
    }
    if let (Some(pool), Some(db)) = (opts.db, &snap.header.db) {
        let enc = store.get(&object_key(&db.object)).await?.ok_or("database object missing at the backup target")?;
        let tmp = tempfile::Builder::new().prefix("paperrust-restore-").suffix(".gz").tempfile().map_err(|e| e.to_string())?;
        {
            let mut out = std::io::BufWriter::new(tmp.as_file());
            crypto::decrypt_stream(identity, enc.as_slice(), &mut out)?;
            out.flush().map_err(|e| e.to_string())?;
        }
        let got = sha256_file(tmp.path()).map_err(|e| e.to_string())?;
        if got != db.object {
            return Err("checksum mismatch of the database dump".into());
        }
        let r = dbdump::restore(pool, tmp.path()).await?;
        st.db_tables = Some(r.tables);
    }
    Ok(st)
}

/// Verify: all objects of the directory present (without decrypting the contents).
pub async fn check(
    target: &crate::config::Target,
    identity: &age::x25519::Identity,
    snapshot: &str,
) -> Result<(usize, Vec<String>), String> {
    let snap = load_snapshot(target, identity, snapshot).await?;
    let store = Store::open(target)?;
    let present: HashSet<String> = store.list("objects/").await?.into_iter().collect();
    let mut missing = Vec::new();
    let mut ids: Vec<&str> = snap.files.iter().map(|f| f.sha256.as_str()).collect();
    if let Some(db) = &snap.header.db {
        ids.push(&db.object);
    }
    let uniq: HashSet<&str> = ids.into_iter().collect();
    for id in &uniq {
        if !present.contains(&object_key(id)) {
            missing.push(id.to_string());
        }
    }
    Ok((uniq.len(), missing))
}

/// Short form of a hash for messages.
pub fn short(sha: &str) -> &str {
    &sha[..12.min(sha.len())]
}

/// Hex of bytes (for tests).
pub fn hex_of(b: &[u8]) -> String {
    hex(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrip() {
        let s = Snapshot {
            header: SnapshotHeader {
                format: 1,
                id: "20261114T021500Z".into(),
                created: Utc::now(),
                version: "0.1.0".into(),
                files: 2,
                bytes: 3,
                db: Some(DbEntry { object: "ab".repeat(32), size: 9, tables: 4 }),
            },
            files: vec![
                FileEntry { sha256: "cd".repeat(32), size: 1, mtime_ns: 5, path: "documents/originals/a b.pdf".into() },
                FileEntry { sha256: "ef".repeat(32), size: 2, mtime_ns: -1, path: "documents/archive/ü.pdf".into() },
            ],
        };
        assert_eq!(Snapshot::decode(&s.encode().unwrap()).unwrap(), s);
    }

    #[test]
    fn ids_and_keys() {
        assert_eq!(object_key("abcdef"), "objects/ab/abcdef");
        assert!(snapshot_time("20261114T021500Z").is_some());
        assert!(snapshot_time("20261114T021500Z-2").is_some());
        assert!(snapshot_time("nonsense").is_none());
    }
}
