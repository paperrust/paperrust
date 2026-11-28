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

/// Files found under the media root, plus warnings for entries that were skipped on purpose.
type Walked = (Vec<(String, u64, i64)>, Vec<String>);

/// All files under `MEDIA_ROOT` (except system files), sorted by path.
///
/// A backup that silently misses files is worse than a failed one (the snapshot would look
/// complete and older, complete snapshots could be pruned). Therefore an unreadable media
/// root, an unreadable folder or a failing directory listing is an **error** that aborts the
/// run before a snapshot is written. Symbolic links to folders are followed (a link back to
/// one of its own parent folders is a loop and skipped with a warning); links to files are
/// backed up as regular files; dangling links and entries that vanish during the walk only
/// produce warnings.
fn walk_media(root: &Path) -> Result<Walked, String> {
    struct Walk<'a> {
        root: &'a Path,
        out: Vec<(String, u64, i64)>,
        warnings: Vec<String>,
        /// canonical paths of the folders currently being walked (loop protection)
        stack: Vec<PathBuf>,
    }
    fn unreadable(dir: &Path, e: &std::io::Error) -> String {
        format!("media folder {} is not readable ({e}); backup aborted, no snapshot written", dir.display())
    }
    fn rec(w: &mut Walk<'_>, dir: &Path, rel_dir: &str) -> Result<(), String> {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            // removed during the run (race with a delete): nothing to back up there
            Err(e) if !rel_dir.is_empty() && e.kind() == std::io::ErrorKind::NotFound => {
                w.warnings.push(format!("{rel_dir}: folder vanished during the backup"));
                return Ok(());
            }
            Err(e) => return Err(unreadable(dir, &e)),
        };
        for e in rd {
            let e = e.map_err(|e| unreadable(dir, &e))?;
            let p = e.path();
            let Some(rel) = crate::guard::rel_path(w.root, &p) else { continue };
            let ft = match e.file_type() {
                Ok(ft) => ft,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    w.warnings.push(format!("{rel}: vanished during the backup"));
                    continue;
                }
                Err(err) => return Err(format!("{}: {err}; backup aborted, no snapshot written", p.display())),
            };
            // symbolic links (and Windows junctions): look at the target
            let (is_dir, link_meta) = if ft.is_symlink() {
                match std::fs::metadata(&p) {
                    Ok(m) if m.is_dir() || m.is_file() => (m.is_dir(), Some(m)),
                    Ok(_) => continue,
                    Err(err) => {
                        w.warnings.push(format!("{rel}: dangling symbolic link skipped ({err})"));
                        continue;
                    }
                }
            } else if ft.is_dir() {
                (true, None)
            } else if ft.is_file() {
                (false, None)
            } else {
                continue;
            };
            if is_dir {
                // NAS folders (`@eaDir`, `#recycle`, `.snapshot` ...) whose files are never backed up
                if crate::detect::is_system_file(&format!("{rel}/x")) {
                    continue;
                }
                let canon = match std::fs::canonicalize(&p) {
                    Ok(c) => c,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        w.warnings.push(format!("{rel}: folder vanished during the backup"));
                        continue;
                    }
                    Err(err) => return Err(unreadable(&p, &err)),
                };
                if w.stack.contains(&canon) {
                    w.warnings.push(format!("{rel}: symbolic link loop skipped"));
                    continue;
                }
                w.stack.push(canon);
                let r = rec(w, &p, &rel);
                w.stack.pop();
                r?;
                continue;
            }
            if crate::detect::is_system_file(&rel) {
                continue;
            }
            let m = match link_meta {
                Some(m) => m,
                None => match e.metadata() {
                    Ok(m) => m,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        w.warnings.push(format!("{rel}: vanished during the backup"));
                        continue;
                    }
                    Err(err) => return Err(format!("{}: {err}; backup aborted, no snapshot written", p.display())),
                },
            };
            w.out.push((rel, m.len(), mtime_ns(&m)));
        }
        Ok(())
    }
    let canon_root = std::fs::canonicalize(root).map_err(|e| unreadable(root, &e))?;
    if !canon_root.is_dir() {
        return Err(format!("media folder {} is not a folder; backup aborted, no snapshot written", root.display()));
    }
    let mut w = Walk { root, out: Vec::new(), warnings: Vec::new(), stack: vec![canon_root] };
    rec(&mut w, root, "")?;
    w.out.sort();
    Ok((w.out, w.warnings))
}

/// Refuse pruning after a run that found no files at all while the previous snapshot had
/// files (media folder unmounted/emptied, or wiped by an attack): the good, older snapshots
/// must stay. Returns the warning text if pruning has to be skipped.
fn empty_snapshot_guard(current_files: u64, previous_files: Option<u64>) -> Option<String> {
    match previous_files {
        Some(prev) if current_files == 0 && prev > 0 => Some(format!(
            "this snapshot contains no files while the previous one had {prev}; old backups were not pruned \
             (check that the media folder is mounted and intact)"
        )),
        _ => None,
    }
}

/// Header of a snapshot from its gzip text form (first line only).
fn decode_header(gz: &[u8]) -> Result<SnapshotHeader, String> {
    let mut r = std::io::BufReader::new(flate2::read::GzDecoder::new(gz));
    let mut head = String::new();
    r.read_line(&mut head).map_err(|e| e.to_string())?;
    serde_json::from_str(head.trim_end()).map_err(|e| format!("snapshot header: {e}"))
}

/// Ids of the local plaintext snapshot copies (`DATA_DIR/backup/snapshots/<id>.gz`).
fn local_snapshot_ids(ldir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(ldir) else { return Vec::new() };
    let mut v: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".gz")).map(str::to_string))
        .filter(|id| snapshot_time(id).is_some())
        .collect();
    v.sort();
    v
}

/// File count of the newest local snapshot copy other than `current`.
fn previous_file_count(ldir: &Path, current: &str) -> Option<u64> {
    let mut ids: Vec<(DateTime<Utc>, String)> =
        local_snapshot_ids(ldir).into_iter().filter(|id| id != current).filter_map(|id| snapshot_time(&id).map(|t| (t, id))).collect();
    ids.sort();
    let (_, id) = ids.pop()?;
    let raw = std::fs::read(ldir.join(format!("{id}.gz"))).ok()?;
    decode_header(&raw).ok().map(|h| h.files)
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

/// What to do with an object of the current run.
#[derive(Debug, PartialEq, Eq)]
enum ObjectAction {
    Upload,
    ExtendRetention,
    Nothing,
}

/// `known`: entry in `paperrust_backup_object` (with its retention date), `at_target`: present
/// in this run's listing of `objects/`. The local table is only a cache: an object that
/// vanished at the target (deleted, lost) is uploaded again, otherwise the new snapshot
/// would reference a missing object.
fn object_action(
    known: Option<&Option<DateTime<Utc>>>,
    at_target: bool,
    extend_before: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> ObjectAction {
    match known {
        None => ObjectAction::Upload,
        Some(_) if !at_target => ObjectAction::Upload,
        Some(old) => {
            if let (Some(before), Some(_)) = (extend_before, until)
                && old.is_none_or(|t| t < before)
            {
                ObjectAction::ExtendRetention
            } else {
                ObjectAction::Nothing
            }
        }
    }
}

/// Encrypt and upload an object (if not yet present at the target according to this run's
/// listing `present`, which is updated after the upload).
async fn upload_object(
    store: &Store,
    recipient: &age::x25519::Recipient,
    src: &Path,
    sha: &str,
    tmp_dir: &Path,
    until: Option<DateTime<Utc>>,
    present: &mut HashSet<String>,
) -> Result<u64, String> {
    let key = object_key(sha);
    // do not upload twice after loss of local knowledge (new DB)
    if present.contains(&key) {
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
    res?;
    present.insert(key);
    Ok(n)
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
    // one listing of all objects per run: the local object table is checked against it
    let mut present: HashSet<String> = store.list("objects/").await?.into_iter().collect();
    let mut reuploaded = 0usize;

    // 1. database
    let mut db_entry = None;
    if cfg.database {
        let dump = tmp_dir.join(format!("db-{}.gz", rec.id));
        let stats = dbdump::dump(pool, &dump).await?;
        let d2 = dump.clone();
        let sha = tokio::task::spawn_blocking(move || sha256_file(&d2)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let size = std::fs::metadata(&dump).map(|m| m.len()).unwrap_or(0);
        let known = known_objects(pool, &tname, std::slice::from_ref(&sha)).await.map_err(|e| e.to_string())?;
        let at_target = present.contains(&object_key(&sha));
        if !known.contains_key(&sha) || !at_target {
            if known.contains_key(&sha) {
                reuploaded += 1;
            }
            let n = upload_object(&store, &recipient, &dump, &sha, &tmp_dir, until, &mut present).await?;
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
    let (files, walk_warnings) = tokio::task::spawn_blocking(move || walk_media(&root)).await.map_err(|e| e.to_string())??;
    warnings.extend(walk_warnings);
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
            let k = known.get(&e.sha256);
            let at_target = present.contains(&object_key(&e.sha256));
            match object_action(k, at_target, extend_before, until) {
                ObjectAction::Upload => {
                    if k.is_some() {
                        reuploaded += 1;
                    }
                    let abs = src.media_root.join(&e.path);
                    // content changed since hashing? Then with a new hash on the next run.
                    let n = upload_object(&store, &recipient, &abs, &e.sha256, &tmp_dir, until, &mut present).await?;
                    if n > 0 {
                        rec.files_new += 1;
                        rec.bytes_new += n as i64;
                    }
                    remember_object(pool, &tname, &e.sha256, e.size, until).await.map_err(|e| e.to_string())?;
                }
                ObjectAction::ExtendRetention => {
                    if let Some(new_until) = until {
                        store.extend_retention(&object_key(&e.sha256), new_until).await?;
                        remember_object(pool, &tname, &e.sha256, e.size, Some(new_until)).await.map_err(|e| e.to_string())?;
                    }
                }
                ObjectAction::Nothing => {}
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

    if reuploaded > 0 {
        warnings.push(format!("{reuploaded} object(s) were missing at the backup target and have been uploaded again"));
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
    let snap_files = snap.header.files;
    let plain = snap.encode()?;
    let enc = crypto::encrypt_bytes(&recipient, &plain)?;
    store.put(&snapshot_key(&sid), enc, until).await?;
    // local plaintext copy (for cleanup without the passphrase)
    let ldir = local_dir(&src.data_dir).join("snapshots");
    std::fs::create_dir_all(&ldir).map_err(|e| e.to_string())?;
    std::fs::write(ldir.join(format!("{sid}.gz")), &plain).map_err(|e| e.to_string())?;
    let previous_files = previous_file_count(&ldir, &sid);
    rec.snapshot = Some(sid.clone());

    // 4. cleanup (never in read-only mode: otherwise good states could be displaced by encrypted ones)
    if crate::readonly::is_active() {
        warnings.push("read-only mode: old backups were not pruned".into());
    } else if let Some(w) = empty_snapshot_guard(snap_files, previous_files) {
        warnings.push(w);
    } else {
        match prune(pool, &store, &tname, &src.data_dir, cfg, Some(&sid)).await {
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

/// Which snapshots prune deletes and which it keeps.
#[derive(Debug, Default, PartialEq)]
struct PrunePlan {
    /// listed at the target and outside the retention rule
    delete: Vec<String>,
    /// referenced: kept by the retention rule, the current snapshot and every snapshot that
    /// has a local copy but is missing from the listing (an incomplete listing must not lead
    /// to deleting the objects of those snapshots)
    keep: Vec<String>,
}

/// Retention over all known snapshots (listed + local copies + current); only listed ones
/// can be deleted, the current one never.
fn plan_prune(listed: &[String], local: &[String], current: Option<&str>, cfg: &BackupConfig) -> PrunePlan {
    let mut all: Vec<String> = listed.to_vec();
    all.extend(local.iter().cloned());
    all.extend(current.map(str::to_string));
    all.sort();
    all.dedup();
    let timed: Vec<(String, DateTime<Utc>)> = all.iter().filter_map(|id| snapshot_time(id).map(|t| (id.clone(), t))).collect();
    let times: Vec<DateTime<Utc>> = timed.iter().map(|(_, t)| *t).collect();
    let kept = retention::keep(&times, cfg.keep_last, cfg.keep_daily, cfg.keep_weekly, cfg.keep_monthly);
    let mut plan = PrunePlan::default();
    for (i, (id, _)) in timed.iter().enumerate() {
        let deletable = listed.contains(id) && current != Some(id.as_str()) && !kept.contains(&i);
        if deletable {
            plan.delete.push(id.clone());
        } else {
            plan.keep.push(id.clone());
        }
    }
    plan
}

/// Objects referenced by the given snapshots (from their local copies). `Ok(Err(id))` if a
/// local copy is missing: then nothing may be deleted.
fn referenced_objects(ldir: &Path, ids: &[String]) -> Result<Result<HashSet<String>, String>, String> {
    let mut referenced: HashSet<String> = HashSet::new();
    for id in ids {
        let p = ldir.join(format!("{id}.gz"));
        match std::fs::read(&p) {
            Ok(raw) => {
                let s = Snapshot::decode(&raw)?;
                referenced.extend(s.files.into_iter().map(|f| f.sha256));
                if let Some(db) = s.header.db {
                    referenced.insert(db.object);
                }
            }
            Err(_) => return Ok(Err(id.clone())),
        }
    }
    Ok(Ok(referenced))
}

/// Delete old directories according to the retention rule, then objects no longer in use.
/// `current`: the snapshot just written (always kept and referenced, even if the listing
/// does not show it yet).
pub async fn prune(
    pool: &PgPool,
    store: &Store,
    tname: &str,
    data_dir: &Path,
    cfg: &BackupConfig,
    current: Option<&str>,
) -> Result<PruneStats, String> {
    let mut st = PruneStats::default();
    let keys = store.list("snapshots/").await?;
    let listed: Vec<String> = keys.iter().filter_map(|k| k.strip_prefix("snapshots/")?.strip_suffix(".age").map(str::to_string)).collect();
    let ldir = local_dir(data_dir).join("snapshots");
    let plan = plan_prune(&listed, &local_snapshot_ids(&ldir), current, cfg);
    let mut remaining: Vec<String> = plan.keep;
    for id in &plan.delete {
        if store.delete(&snapshot_key(id)).await? {
            st.snapshots += 1;
            let _ = std::fs::remove_file(ldir.join(format!("{id}.gz")));
        } else {
            // retention period still running: try again on the next run
            remaining.push(id.clone());
        }
    }
    // objects: only if a local copy exists for every remaining directory
    let referenced = match referenced_objects(&ldir, &remaining)? {
        Ok(r) => r,
        Err(id) => {
            st.warnings.push(format!("no local copy of snapshot {id}; unused objects are kept"));
            return Ok(st);
        }
    };
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
    /// tables of the dump missing in the target database (only with `allow_missing_tables`)
    pub db_skipped_tables: Vec<String>,
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
    /// skip tables of the dump that do not exist in the target database instead of failing
    pub allow_missing_tables: bool,
}

/// Destination of a snapshot path below the restore folder `root`. Refuses unsafe paths and
/// any existing path component that is a symbolic link (or junction): otherwise a link placed
/// in the restore folder could redirect writes outside of it. `root` itself may be a link.
fn safe_dest(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.split('/').any(|c| c == ".." || c == "." || c.is_empty()) || rel.contains('\\') || rel.contains(':') {
        return Err(format!("unsafe path in snapshot: {rel}"));
    }
    let parts: Vec<&str> = rel.split('/').collect();
    let mut p = root.to_path_buf();
    let mut exists = true;
    for (i, c) in parts.iter().enumerate() {
        p.push(c);
        if !exists {
            continue;
        }
        let last = i + 1 == parts.len();
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("refusing to restore through a symbolic link: {}", p.display()));
            }
            Ok(m) if !last && !m.is_dir() => return Err(format!("{} is not a folder", p.display())),
            Ok(m) if last && m.is_dir() => return Err(format!("{} is a folder", p.display())),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => exists = false,
            Err(e) => return Err(format!("{}: {e}", p.display())),
        }
    }
    Ok(p)
}

/// Fresh temporary path in `dir` (random name, created exclusively, removed on drop).
fn temp_path_in(dir: &Path, suffix: &str) -> Result<tempfile::TempPath, String> {
    tempfile::Builder::new()
        .prefix(".restore-")
        .suffix(suffix)
        .tempfile_in(dir)
        .map(|f| f.into_temp_path())
        .map_err(|e| format!("{}: {e}", dir.display()))
}

/// Writer that hashes everything written through it.
struct HashingWriter<W> {
    inner: W,
    h: sha2::Sha256,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        sha2::Digest::update(&mut self.h, &buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Decrypt the file `enc` into `out` (`None` = only hash) and return (plaintext bytes,
/// SHA-256 of the plaintext), streaming.
fn decrypt_file_hashing(identity: &age::x25519::Identity, enc: &Path, out: Option<&Path>) -> Result<(u64, String), String> {
    let input = std::io::BufReader::new(std::fs::File::open(enc).map_err(|e| format!("{}: {e}", enc.display()))?);
    let sink: Box<dyn Write> = match out {
        Some(o) => Box::new(std::io::BufWriter::new(
            std::fs::OpenOptions::new().write(true).truncate(true).open(o).map_err(|e| format!("{}: {e}", o.display()))?,
        )),
        None => Box::new(std::io::sink()),
    };
    let mut w = HashingWriter { inner: sink, h: <sha2::Sha256 as sha2::Digest>::new() };
    let n = crypto::decrypt_stream(identity, input, &mut w)?;
    w.flush().map_err(|e| e.to_string())?;
    Ok((n, hex(&sha2::Digest::finalize(w.h))))
}

/// Restore; each file is checked against its checksum after decryption. Objects are
/// downloaded into a temporary file and decrypted from there (never fully in memory).
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
            let dest = safe_dest(dir, &f.path)?;
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
            let parent = dest.parent().ok_or_else(|| format!("unsafe path in snapshot: {}", f.path))?.to_path_buf();
            std::fs::create_dir_all(&parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            let enc = temp_path_in(&parent, ".download.paperrust-tmp")?;
            if !store.get_to_file(&object_key(&f.sha256), &enc).await? {
                return Err(format!("object for {} missing at the backup target", f.path));
            }
            let (id, want, d) = (identity.clone(), f.sha256.clone(), dest.clone());
            let n = tokio::task::spawn_blocking(move || -> Result<u64, String> {
                let out = temp_path_in(&parent, ".paperrust-tmp")?;
                let (n, got) = decrypt_file_hashing(&id, &enc, Some(&out))?;
                drop(enc);
                if got != want {
                    return Err(format!("checksum mismatch after restore: {}", d.display()));
                }
                out.persist(&d).map_err(|e| format!("{}: {}", d.display(), e.error))?;
                Ok(n)
            })
            .await
            .map_err(|e| e.to_string())??;
            st.files += 1;
            st.bytes += n;
        }
    }
    if let (Some(pool), Some(db)) = (opts.db, &snap.header.db) {
        let tmp_dir = std::env::temp_dir();
        let enc = temp_path_in(&tmp_dir, ".age")?;
        if !store.get_to_file(&object_key(&db.object), &enc).await? {
            return Err("database object missing at the backup target".into());
        }
        let plain = temp_path_in(&tmp_dir, ".gz")?;
        let (id, e2, p2) = (identity.clone(), enc.to_path_buf(), plain.to_path_buf());
        let (_, got) =
            tokio::task::spawn_blocking(move || decrypt_file_hashing(&id, &e2, Some(&p2))).await.map_err(|e| e.to_string())??;
        drop(enc);
        if got != db.object {
            return Err("checksum mismatch of the database dump".into());
        }
        let r = dbdump::restore(pool, &plain, opts.allow_missing_tables).await?;
        st.db_tables = Some(r.tables);
        st.db_skipped_tables = r.skipped;
    }
    Ok(st)
}

/// Result of `backup check`.
#[derive(Debug, Default, Clone)]
pub struct CheckReport {
    /// distinct objects referenced by the snapshot
    pub objects: usize,
    /// not present at the target
    pub missing: Vec<String>,
    /// present, but the stored size does not fit the plaintext size (truncated/replaced)
    pub size_mismatch: Vec<String>,
    /// `--deep` only: could not be decrypted or the checksum of the plaintext differs
    pub damaged: Vec<String>,
}

impl CheckReport {
    pub fn is_ok(&self) -> bool {
        self.missing.is_empty() && self.size_mismatch.is_empty() && self.damaged.is_empty()
    }
}

/// age format for one X25519 recipient: header (fixed text + one stanza + MAC), 16-byte
/// payload nonce, then 64 KiB chunks with a 16-byte tag each (at least one chunk).
const AGE_HEADER_MIN: u64 = 22 + 10 + 43 + 1 + 43 + 1 + 4 + 43 + 1;
/// Room for additional header stanzas the age library adds (random-length "grease" stanza,
/// observed header sizes 241..309 bytes).
const AGE_HEADER_SLACK: u64 = 1024;

/// Is `stored` a plausible size of an age-encrypted object with `plain` bytes of plaintext?
pub fn encrypted_size_plausible(plain: u64, stored: u64) -> bool {
    let chunks = plain.div_ceil(64 * 1024).max(1);
    let min = plain + AGE_HEADER_MIN + 16 + 16 * chunks;
    stored >= min && stored <= min + AGE_HEADER_SLACK
}

/// Verify a snapshot: every object present at the target with a plausible size; with `deep`
/// every object is also downloaded, decrypted and its SHA-256 compared with its name.
pub async fn check(
    target: &crate::config::Target,
    identity: &age::x25519::Identity,
    snapshot: &str,
    deep: bool,
) -> Result<CheckReport, String> {
    let snap = load_snapshot(target, identity, snapshot).await?;
    let store = Store::open(target)?;
    let present: HashMap<String, u64> = store.list_sized("objects/").await?.into_iter().collect();
    // plaintext size per object (sorted for a stable report)
    let mut want: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
    for f in &snap.files {
        want.entry(f.sha256.as_str()).or_insert(f.size);
    }
    if let Some(db) = &snap.header.db {
        want.entry(db.object.as_str()).or_insert(db.size);
    }
    let mut rep = CheckReport { objects: want.len(), ..Default::default() };
    for (id, size) in &want {
        match present.get(&object_key(id)) {
            None => rep.missing.push(id.to_string()),
            Some(stored) if !encrypted_size_plausible(*size, *stored) => rep.size_mismatch.push(id.to_string()),
            Some(_) => {}
        }
    }
    if deep {
        let tmp_dir = std::env::temp_dir();
        for id in want.keys() {
            if !present.contains_key(&object_key(id)) {
                continue;
            }
            let enc = temp_path_in(&tmp_dir, ".age")?;
            if !store.get_to_file(&object_key(id), &enc).await? {
                rep.missing.push(id.to_string());
                continue;
            }
            let (ident, e2) = (identity.clone(), enc.to_path_buf());
            let res = tokio::task::spawn_blocking(move || decrypt_file_hashing(&ident, &e2, None)).await.map_err(|e| e.to_string())?;
            drop(enc);
            if res.map(|(_, h)| h).as_deref() != Ok(*id) {
                rep.damaged.push(id.to_string());
            }
        }
    }
    Ok(rep)
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

    // ---- helpers for the regression tests (no database) ----

    /// Folder link: symlink (Unix), symlink or junction (Windows, junctions need no privilege).
    fn dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
                || std::process::Command::new("cmd")
                    .args(["/C", "mklink", "/J"])
                    .arg(link)
                    .arg(target)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
        }
    }

    fn write(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }

    fn cfg(keep_last: usize) -> BackupConfig {
        BackupConfig { target: None, database: false, keep_last, keep_daily: 0, keep_weekly: 0, keep_monthly: 0 }
    }

    struct Repo {
        _dir: tempfile::TempDir,
        work: PathBuf,
        target: crate::config::Target,
        identity: age::x25519::Identity,
        recipient: age::x25519::Recipient,
    }

    async fn repo() -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let target = crate::config::Target::Local(dir.path().join("repo"));
        let (public, secret) = init(&target, &dir.path().join("data"), "a long test passphrase", Some(10)).await.unwrap();
        let identity = crypto::parse_identity(&secret).unwrap();
        assert_eq!(identity.to_public().to_string(), public);
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        Repo { work, target, identity, recipient: crypto::parse_recipient(&public).unwrap(), _dir: dir }
    }

    /// Stores the files as objects and writes snapshot `id` referencing them.
    async fn put_snapshot(r: &Repo, id: &str, files: &[(&str, &[u8])]) -> Snapshot {
        let store = Store::open(&r.target).unwrap();
        let mut entries = Vec::new();
        for (i, (path, data)) in files.iter().enumerate() {
            let src = r.work.join(format!("plain-{id}-{i}"));
            std::fs::write(&src, data).unwrap();
            let enc = r.work.join(format!("enc-{id}-{i}"));
            let (_, sha) = crypto::encrypt_file(&r.recipient, &src, &enc).unwrap();
            if !store.exists(&object_key(&sha)).await.unwrap() {
                store.put_file(&object_key(&sha), &enc, None).await.unwrap();
            }
            entries.push(FileEntry { sha256: sha, size: data.len() as u64, mtime_ns: 0, path: path.to_string() });
        }
        let snap = Snapshot {
            header: SnapshotHeader {
                format: FORMAT,
                id: id.into(),
                created: Utc::now(),
                version: "test".into(),
                files: entries.len() as u64,
                bytes: entries.iter().map(|e| e.size).sum(),
                db: None,
            },
            files: entries,
        };
        let enc = crypto::encrypt_bytes(&r.recipient, &snap.encode().unwrap()).unwrap();
        store.put(&snapshot_key(id), enc, None).await.unwrap();
        snap
    }

    /// Replace a stored object (local target files are write-protected).
    fn overwrite_object(r: &Repo, sha: &str, data: &[u8]) {
        let crate::config::Target::Local(root) = &r.target else { unreachable!() };
        let p = root.join(object_key(sha));
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&p, perm).unwrap();
        std::fs::write(&p, data).unwrap();
    }

    // ---- item 1: walk_media ----

    /// Regression: an unreadable/missing media root used to yield an empty file list (the run
    /// then wrote an empty snapshot and pruned the good ones).
    #[test]
    fn walk_media_missing_root_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        let e = walk_media(&t.path().join("not-mounted")).unwrap_err();
        assert!(e.contains("not readable"), "{e}");
    }

    /// Regression: folder links were skipped silently (`file_type().is_dir()` is false for links).
    #[test]
    fn walk_media_follows_folder_links_and_skips_loops() {
        let t = tempfile::tempdir().unwrap();
        let media = t.path().join("media");
        let outside = t.path().join("outside");
        write(&media, "documents/originals/a.pdf", b"a");
        write(&outside, "b.pdf", b"bb");
        write(&media, "documents/@eaDir/thumb.jpg", b"x");
        let docs = media.join("documents");
        if !dir_link(&outside, &docs.join("linked")) || !dir_link(&docs, &docs.join("loop")) {
            eprintln!("folder links not supported here; test skipped");
            return;
        }
        let (files, warnings) = walk_media(&media).unwrap();
        let paths: Vec<&str> = files.iter().map(|(p, _, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["documents/linked/b.pdf", "documents/originals/a.pdf"], "{warnings:?}");
        assert_eq!(files[0].1, 2);
        assert!(warnings.iter().any(|w| w.contains("documents/loop") && w.contains("loop")), "{warnings:?}");
    }

    /// Regression: an unreadable subfolder was skipped silently.
    #[cfg(unix)]
    #[test]
    fn walk_media_unreadable_subfolder_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let media = t.path().join("media");
        write(&media, "documents/originals/a.pdf", b"a");
        let locked = media.join("documents/archive");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable_anyway = std::fs::read_dir(&locked).is_ok(); // running as root
        let r = walk_media(&media);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if readable_anyway {
            eprintln!("running with root rights; test skipped");
            return;
        }
        assert!(r.unwrap_err().contains("not readable"));
    }

    /// An empty snapshot after one with files must not prune the good, older snapshots.
    #[test]
    fn empty_snapshot_after_full_one_blocks_pruning() {
        assert!(empty_snapshot_guard(0, Some(12)).is_some());
        assert!(empty_snapshot_guard(0, Some(0)).is_none());
        assert!(empty_snapshot_guard(0, None).is_none());
        assert!(empty_snapshot_guard(5, Some(12)).is_none());
        let t = tempfile::tempdir().unwrap();
        let s = |id: &str, files: u64| Snapshot {
            header: SnapshotHeader { format: 1, id: id.into(), created: Utc::now(), version: "t".into(), files, bytes: 0, db: None },
            files: Vec::new(),
        };
        std::fs::write(t.path().join("20261101T000000Z.gz"), s("20261101T000000Z", 3).encode().unwrap()).unwrap();
        std::fs::write(t.path().join("20261102T000000Z.gz"), s("20261102T000000Z", 7).encode().unwrap()).unwrap();
        std::fs::write(t.path().join("20261103T000000Z.gz"), s("20261103T000000Z", 0).encode().unwrap()).unwrap();
        assert_eq!(previous_file_count(t.path(), "20261103T000000Z"), Some(7));
    }

    // ---- item 2: objects known locally but missing at the target ----

    /// Regression: an object known from the local table was never uploaded again, even if
    /// it vanished at the target; later snapshots then referenced a missing object.
    #[test]
    fn known_but_missing_object_is_uploaded_again() {
        let now = Utc::now();
        let until = Some(now + chrono::Duration::days(30));
        let before = Some(now + chrono::Duration::days(15));
        assert_eq!(object_action(Some(&None), false, None, None), ObjectAction::Upload);
        assert_eq!(object_action(Some(&until), false, before, until), ObjectAction::Upload);
        assert_eq!(object_action(None, true, None, None), ObjectAction::Upload);
        assert_eq!(object_action(Some(&None), true, None, None), ObjectAction::Nothing);
        assert_eq!(object_action(Some(&Some(now)), true, before, until), ObjectAction::ExtendRetention);
        assert_eq!(object_action(Some(&until), true, before, until), ObjectAction::Nothing);
    }

    // ---- item 7: prune with an incomplete listing ----

    /// Regression: prune decided only from the listing; with the newest snapshot missing from
    /// a (truncated) listing its objects were deleted. Now the current snapshot and all
    /// snapshots with a local copy are kept and referenced.
    #[test]
    fn prune_keeps_current_and_locally_known_snapshots() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // listing misses the two newest snapshots
        let listed = ids(&["20261101T000000Z", "20261102T000000Z"]);
        let local = ids(&["20261101T000000Z", "20261102T000000Z", "20261103T000000Z"]);
        let plan = plan_prune(&listed, &local, Some("20261104T000000Z"), &cfg(1));
        assert_eq!(plan.delete, ids(&["20261101T000000Z", "20261102T000000Z"]));
        assert_eq!(plan.keep, ids(&["20261103T000000Z", "20261104T000000Z"]));
        // the current snapshot is never deleted, even if listed and retention would drop it
        let plan = plan_prune(&ids(&["20261105T000000Z", "20261104T000000Z"]), &[], Some("20261104T000000Z"), &cfg(1));
        assert_eq!(plan.delete, Vec::<String>::new());
        // the objects of the kept (unlisted) snapshots are referenced
        let t = tempfile::tempdir().unwrap();
        let snap = Snapshot {
            header: SnapshotHeader {
                format: 1,
                id: "20261103T000000Z".into(),
                created: Utc::now(),
                version: "t".into(),
                files: 1,
                bytes: 1,
                db: Some(DbEntry { object: "db".repeat(32), size: 1, tables: 1 }),
            },
            files: vec![FileEntry { sha256: "ab".repeat(32), size: 1, mtime_ns: 0, path: "a".into() }],
        };
        std::fs::write(t.path().join("20261103T000000Z.gz"), snap.encode().unwrap()).unwrap();
        let r = referenced_objects(t.path(), &ids(&["20261103T000000Z"])).unwrap().unwrap();
        assert!(r.contains(&"ab".repeat(32)) && r.contains(&"db".repeat(32)));
        assert_eq!(referenced_objects(t.path(), &ids(&["20261104T000000Z"])).unwrap(), Err("20261104T000000Z".to_string()));
    }

    // ---- item 4/5: restore (streaming, no symlinks) ----

    /// Regression: a link inside the restore folder redirected writes outside of it.
    #[test]
    fn safe_dest_refuses_links() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("restore");
        let outside = t.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        assert!(safe_dest(&root, "documents/originals/a.pdf").unwrap().ends_with("a.pdf"));
        assert!(safe_dest(&root, "../a").is_err());
        assert!(safe_dest(&root, "a/./b").is_err());
        if !dir_link(&outside, &root.join("documents")) {
            eprintln!("folder links not supported here; test skipped");
            return;
        }
        let e = safe_dest(&root, "documents/originals/a.pdf").unwrap_err();
        assert!(e.contains("symbolic link"), "{e}");
    }

    /// Restore end to end on a local target without database: streaming download/decrypt,
    /// checksums verified; a planted folder link makes the restore fail without writing outside.
    #[tokio::test]
    async fn restore_media_streaming_and_link_refusal() {
        let r = repo().await;
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        put_snapshot(&r, "20261101T000000Z", &[("documents/originals/a.pdf", b"hello"), ("documents/archive/big.bin", &big)]).await;
        let dest = r.work.join("restored");
        let st = restore(
            &r.target,
            &r.identity,
            &RestoreOptions { snapshot: "latest", media_dir: Some(&dest), db: None, overwrite: false, allow_missing_tables: false },
        )
        .await
        .unwrap();
        assert_eq!((st.files, st.bytes), (2, 200_005));
        assert_eq!(std::fs::read(dest.join("documents/archive/big.bin")).unwrap(), big);
        // no temporary files left behind
        let leftovers: Vec<_> = std::fs::read_dir(dest.join("documents/archive")).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1);

        let dest2 = r.work.join("restored2");
        let outside = r.work.join("outside");
        std::fs::create_dir_all(&dest2).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        if !dir_link(&outside, &dest2.join("documents")) {
            eprintln!("folder links not supported here; test skipped");
            return;
        }
        let e = restore(
            &r.target,
            &r.identity,
            &RestoreOptions { snapshot: "latest", media_dir: Some(&dest2), db: None, overwrite: false, allow_missing_tables: false },
        )
        .await
        .unwrap_err();
        assert!(e.contains("symbolic link"), "{e}");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing written outside the restore folder");
    }

    // ---- item 6: check compares sizes, --deep verifies contents ----

    #[test]
    fn age_size_model_matches_real_encryption() {
        let (_, public) = crypto::generate();
        let rc = crypto::parse_recipient(&public).unwrap();
        for n in [0usize, 1, 100, 65_535, 65_536, 65_537, 200_000] {
            let data = vec![7u8; n];
            let enc = crypto::encrypt_bytes(&rc, &data).unwrap();
            let len = enc.len() as u64;
            assert!(encrypted_size_plausible(n as u64, len), "plaintext {n}: encrypted {len}");
            // the age library adds a random-length "grease" stanza, so only clear deviations are
            // detected by size (small truncations are found by `--deep`)
            assert!(!encrypted_size_plausible(n as u64, len / 2), "half missing must be detected (plaintext {n})");
            assert!(!encrypted_size_plausible(n as u64, len + 2 * AGE_HEADER_SLACK), "plaintext {n}");
            assert!(!encrypted_size_plausible(n as u64, n as u64), "plaintext size alone is not plausible");
        }
    }

    /// Regression: `backup check` only compared names; a truncated or replaced object passed.
    #[tokio::test]
    async fn check_detects_truncated_and_replaced_objects() {
        let r = repo().await;
        let snap =
            put_snapshot(&r, "20261101T000000Z", &[("a.pdf", b"first file"), ("b.pdf", b"second file"), ("c.pdf", b"third file!")]).await;
        let rep = check(&r.target, &r.identity, "latest", true).await.unwrap();
        assert!(rep.is_ok() && rep.objects == 3, "{rep:?}");
        // truncated object: wrong size (found without --deep)
        let a = &snap.files[0].sha256;
        let crate::config::Target::Local(root) = &r.target else { unreachable!() };
        let full = std::fs::read(root.join(object_key(a))).unwrap();
        overwrite_object(&r, a, &full[..full.len() / 2]);
        // replaced object: same size, other content (found only with --deep)
        let b = &snap.files[1].sha256;
        let other = crypto::encrypt_bytes(&r.recipient, b"other file!").unwrap();
        assert!(encrypted_size_plausible(11, other.len() as u64));
        overwrite_object(&r, b, &other);
        let rep = check(&r.target, &r.identity, "latest", false).await.unwrap();
        assert_eq!(rep.size_mismatch, vec![a.clone()]);
        assert!(rep.damaged.is_empty() && rep.missing.is_empty());
        let rep = check(&r.target, &r.identity, "latest", true).await.unwrap();
        assert_eq!(rep.size_mismatch, vec![a.clone()]);
        let mut damaged = rep.damaged.clone();
        damaged.sort();
        let mut want = vec![a.clone(), b.clone()];
        want.sort();
        assert_eq!(damaged, want);
    }
}
