//! `document_sanity_checker` and core of `documents.tasks.sanity_check`.
//!
//! upstream: src/documents/sanity_checker.py (`check_sanity`, `SanityCheckMessages`,
//! `_build_present_files`, `_check_*`), src/documents/management/commands/document_sanity_checker.py
//! (output; Rich table as text lines here), src/documents/tasks.py (`sanity_check`) – v3.4.2.
//!
//! Checks, message texts, levels and order of the messages are those of upstream. PaperRust
//! adds (Z2, additive only, without effect on the v1 API and messages):
//!
//! * Checksums are computed in parallel (`PAPERRUST_INTEGRITY_THREADS`); the messages are
//!   then assembled in the upstream order (documents by `-created`).
//! * **Incremental mode** (`PAPERRUST_INTEGRITY_INCREMENTAL`, CLI `--incremental`): a file with the
//!   same path, same size, same modification time and same stored checksum as at
//!   the last successful check (table `paperrust_integrity_state`) is not read again
//!   as long as this confirmation is younger than `PAPERRUST_INTEGRITY_FULL_HASH_DAYS` days.
//!   Presence, readability of thumbnails, orphaned files and empty content are
//!   always checked completely.
//! * **Report** as JSON per run under `DATA_DIR/integrity/`, the last
//!   `PAPERRUST_INTEGRITY_REPORT_KEEP` are kept.
//! * Start/end line in the logger `paperless.integrity` (→ `paperless.log`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;

use crate::args;
use crate::ctx::Ctx;
use crate::{CliError, CliResult};

pub const ERROR: u8 = 40;
pub const WARNING: u8 = 30;
pub const INFO: u8 = 20;

const IGNORABLE_FILES: &[&str] =
    &[".DS_Store", ".DS_STORE", "._*", ".stfolder/*", ".stversions/*", ".localized/*", "desktop.ini", "@eaDir/*", "Thumbs.db"];

/// Own table of the incremental mode (`migrations/0001_paperrust_integrity.sql`).
pub const MIGRATION_SQL: &str = include_str!("../migrations/0001_paperrust_integrity.sql");
/// Entry for `migrate::paperrust_migrations()`.
pub const MIGRATION: (&str, &str) = ("pl_cli.0001_paperrust_integrity", MIGRATION_SQL);

/// Subdirectory of `DATA_DIR` for the JSON reports.
pub const REPORT_DIR: &str = "integrity";
const REPORT_PREFIX: &str = "integrity-";
/// Logger of the PaperRust extra lines (child of `paperless` → `paperless.log`).
pub const LOG_TARGET: &str = "paperless.integrity";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub level: u8,
    pub message: String,
}

/// `SanityCheckMessages` (insertion order of the documents is preserved).
#[derive(Debug, Default)]
pub struct Messages {
    pub order: Vec<Option<i64>>,
    pub messages: BTreeMap<Option<i64>, Vec<Message>>,
    doc_pks: HashSet<i64>,
    err_pks: HashSet<i64>,
    warn_pks: HashSet<i64>,
    info_pks: HashSet<i64>,
    pub has_error: bool,
    pub has_warning: bool,
    pub has_info: bool,
    pub document_error_count: usize,
    pub document_warning_count: usize,
    pub document_info_count: usize,
    pub global_warning_count: usize,
}

impl Messages {
    fn push(&mut self, pk: Option<i64>, level: u8, message: String) {
        if !self.messages.contains_key(&pk) {
            self.order.push(pk);
        }
        self.messages.entry(pk).or_default().push(Message { level, message });
    }

    fn add_doc(&mut self, pk: i64, which: u8) -> bool {
        self.doc_pks.insert(pk);
        let set = match which {
            ERROR => &mut self.err_pks,
            WARNING => &mut self.warn_pks,
            _ => &mut self.info_pks,
        };
        set.insert(pk)
    }

    pub fn error(&mut self, pk: Option<i64>, m: String) {
        self.push(pk, ERROR, m);
        self.has_error = true;
        if let Some(p) = pk
            && self.add_doc(p, ERROR)
        {
            self.document_error_count += 1;
        }
    }

    pub fn warning(&mut self, pk: Option<i64>, m: String) {
        self.push(pk, WARNING, m);
        self.has_warning = true;
        match pk {
            Some(p) => {
                if self.add_doc(p, WARNING) {
                    self.document_warning_count += 1;
                }
            }
            None => self.global_warning_count += 1,
        }
    }

    pub fn info(&mut self, pk: Option<i64>, m: String) {
        self.push(pk, INFO, m);
        self.has_info = true;
        if let Some(p) = pk
            && self.add_doc(p, INFO)
        {
            self.document_info_count += 1;
        }
    }

    fn add(&mut self, pk: Option<i64>, level: u8, m: String) {
        match level {
            ERROR => self.error(pk, m),
            WARNING => self.warning(pk, m),
            _ => self.info(pk, m),
        }
    }

    /// Number of all messages.
    pub fn len(&self) -> usize {
        self.messages.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Result text of upstream `documents.tasks.sanity_check` (on errors with
/// " Check logs for details."; the task then raises `SanityCheckFailedException`).
pub fn task_summary(m: &Messages) -> String {
    if !m.has_error && !m.has_warning && !m.has_info {
        return "No issues detected.".into();
    }
    let mut parts = Vec::new();
    if m.document_error_count > 0 {
        parts.push(format!("{} document(s) with errors", m.document_error_count));
    }
    if m.document_warning_count > 0 {
        parts.push(format!("{} document(s) with warnings", m.document_warning_count));
    }
    if m.document_info_count > 0 {
        parts.push(format!("{} document(s) with infos", m.document_info_count));
    }
    if m.global_warning_count > 0 {
        parts.push(format!("{} global warning(s)", m.global_warning_count));
    }
    let summary = format!("{} found.", parts.join(", "));
    if m.has_error { format!("{summary} Check logs for details.") } else { summary }
}

// ------------------------------------------------------------------ Settings (PaperRust)

/// Settings of the PaperRust extensions. [`Options::default`] = behavior like upstream
/// (hash everything, no state, no report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Do not re-hash unchanged, recently confirmed files.
    pub incremental: bool,
    /// Record confirmed checksums in `paperrust_integrity_state`.
    pub record_state: bool,
    /// Confirmations older than this many days do not count (0 = always hash every file).
    pub full_hash_days: i64,
    /// Parallel readers (≥ 1).
    pub threads: usize,
    /// Number of JSON reports kept (0 = write none).
    pub report_keep: usize,
}

pub const DEFAULT_FULL_HASH_DAYS: i64 = 28;
pub const DEFAULT_REPORT_KEEP: i64 = 10;

fn auto_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)
}

impl Default for Options {
    fn default() -> Self {
        Self { incremental: false, record_state: false, full_hash_days: DEFAULT_FULL_HASH_DAYS, threads: auto_threads(), report_keep: 0 }
    }
}

impl Options {
    /// From `PAPERRUST_INTEGRITY_*`:
    /// `INCREMENTAL` (default no), `FULL_HASH_DAYS` (28), `THREADS` (0 = automatic, ≤ 4),
    /// `REPORT_KEEP` (10, 0 = off).
    pub fn from_env(env: &pl_core::config::Env) -> CliResult<Self> {
        let cfg = |e: pl_core::config::ConfigError| CliError::Command(format!("configuration error: {e}"));
        let incremental = env.bool("PAPERRUST_INTEGRITY_INCREMENTAL", "false").map_err(cfg)?;
        let full_hash_days = env.int_or("PAPERRUST_INTEGRITY_FULL_HASH_DAYS", DEFAULT_FULL_HASH_DAYS).map_err(cfg)?.max(0);
        let threads = match env.int_or("PAPERRUST_INTEGRITY_THREADS", 0).map_err(cfg)? {
            n if n <= 0 => auto_threads(),
            n => usize::try_from(n).unwrap_or(1).min(64),
        };
        let report_keep =
            usize::try_from(env.int_or("PAPERRUST_INTEGRITY_REPORT_KEEP", DEFAULT_REPORT_KEEP).map_err(cfg)?.max(0)).unwrap_or(0);
        Ok(Self { incremental, record_state: incremental, full_hash_days, threads, report_keep })
    }

    /// For the scheduled task: invalid values do not make the check fail, but lead to
    /// a warning and the upstream behavior (complete) with report.
    pub fn for_task(env: &pl_core::config::Env) -> Self {
        Self::from_env(env).unwrap_or_else(|e| {
            tracing::warn!(target: LOG_TARGET, "Integrity check: invalid PAPERRUST_INTEGRITY_* setting ({e}), using a full check");
            Self { report_keep: DEFAULT_REPORT_KEEP as usize, ..Self::default() }
        })
    }

    pub fn mode(&self) -> &'static str {
        if self.incremental { "incremental" } else { "full" }
    }
}

/// Metrics of a run (report, log line).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub documents: usize,
    pub files_hashed: usize,
    pub files_skipped: usize,
    pub bytes_hashed: u64,
    pub orphaned_files: usize,
    pub duration_ms: u128,
}

// ------------------------------------------------------------------ Files

fn walk(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    walk_at(dir, out, 0);
}

/// `Path.glob("**/*")`: does not descend into directory symlinks (review C-N2), depth ≤ 64.
fn walk_at(dir: &Path, out: &mut BTreeSet<PathBuf>, depth: usize) {
    if depth > 64 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk_at(&p, out, depth + 1);
        } else if ft.is_symlink() && p.is_dir() {
            // directory symlink: `glob` returns it, but `is_file()` is false → ignore
            continue;
        } else if !IGNORABLE_FILES.iter().any(|n| e.file_name().to_str() == Some(n)) {
            out.insert(std::path::absolute(&p).unwrap_or(p));
        }
    }
}

fn abs(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
}

/// SHA-256 (hex) and number of bytes read.
fn sha256(p: &Path) -> std::io::Result<(String, u64)> {
    use sha2::Digest;
    use std::io::Read;
    thread_local! {
        // created once per reader thread (not per file: zeroing 1 MiB costs a lot with 200k files)
        static BUF: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0u8; 1 << 20]);
    }
    let mut f = std::fs::File::open(p)?;
    let mut h = sha2::Sha256::new();
    let mut n = 0u64;
    BUF.with_borrow_mut(|buf| -> std::io::Result<()> {
        loop {
            let r = match f.read(buf) {
                Ok(0) => break,
                Ok(r) => r,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            h.update(&buf[..r]);
            n += r as u64;
        }
        Ok(())
    })?;
    Ok((crate::export::fernet::hex_encode(&h.finalize()), n))
}

fn mtime_ns(m: &std::fs::Metadata) -> Option<i64> {
    let d = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_nanos()).ok()
}

// ------------------------------------------------------------------ State (incremental)

/// Kind of a checked file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Original,
    Archive,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Original => "original",
            Kind::Archive => "archive",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "original" => Some(Kind::Original),
            "archive" => Some(Kind::Archive),
            _ => None,
        }
    }
}

/// Row of `paperrust_integrity_state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRow {
    pub document_id: i64,
    pub kind: Kind,
    pub path: String,
    pub size: i64,
    pub mtime_ns: i64,
    pub checksum: String,
    pub verified_at: DateTime<Utc>,
}

pub type StateMap = HashMap<(i64, Kind), StateRow>;

/// Shared data of a run for the reader threads.
struct Run<'a> {
    opts: &'a Options,
    state: &'a StateMap,
    now: DateTime<Utc>,
    thumbnail_dir: &'a Path,
    originals_dir: &'a Path,
    archive_dir: &'a Path,
}

impl Run<'_> {
    fn fresh(&self, verified_at: DateTime<Utc>) -> bool {
        self.opts.full_hash_days > 0 && verified_at > self.now - chrono::Duration::days(self.opts.full_hash_days)
    }
}

/// One row from `documents_document` (only the columns of upstream `.only(...)`).
#[derive(Debug, Clone)]
struct DocRow {
    pk: i64,
    /// `source_path` name (stored file name or `{pk:07}{ext}`).
    fname: String,
    checksum: String,
    archive_checksum: Option<String>,
    archive_filename: Option<String>,
    content_empty: bool,
}

/// Result of a document (computed in a thread, then taken over in order).
#[derive(Debug, Default)]
struct DocOutcome {
    msgs: Vec<(u8, String)>,
    seen: Vec<PathBuf>,
    verified: Vec<StateRow>,
    invalid: Vec<Kind>,
    hashed: usize,
    skipped: usize,
    bytes: u64,
}

/// Checksum of `path` against `expected`: `Ok(None)` matches, `Ok(Some(actual))` mismatch,
/// `Err` read error. In incremental mode possibly confirmed without reading.
fn verify(run: &Run, pk: i64, kind: Kind, path: &Path, expected: &str, o: &mut DocOutcome) -> std::io::Result<Option<String>> {
    let track = run.opts.incremental || run.opts.record_state;
    // fingerprint before reading: if the file changes meanwhile, it will not match on the next run.
    let fp = if track { std::fs::metadata(path).ok().and_then(|m| Some((i64::try_from(m.len()).ok()?, mtime_ns(&m)?))) } else { None };
    let path_s = path.to_string_lossy();
    if run.opts.incremental
        && let Some((size, mt)) = fp
        && let Some(s) = run.state.get(&(pk, kind))
        && s.path == path_s
        && s.size == size
        && s.mtime_ns == mt
        && s.checksum == expected
        && run.fresh(s.verified_at)
    {
        o.skipped += 1;
        return Ok(None);
    }
    let (sum, n) = match sha256(path) {
        Ok(v) => v,
        Err(e) => {
            o.invalid.push(kind);
            return Err(e);
        }
    };
    o.hashed += 1;
    o.bytes += n;
    if sum != expected {
        o.invalid.push(kind);
        return Ok(Some(sum));
    }
    if run.opts.record_state
        && let Some((size, mtime_ns)) = fp
    {
        o.verified.push(StateRow { document_id: pk, kind, path: path_s.into_owned(), size, mtime_ns, checksum: sum, verified_at: run.now });
    }
    Ok(None)
}

/// `_check_document()`: thumbnail, original, archive, content.
fn check_document(run: &Run, d: &DocRow) -> DocOutcome {
    let mut o = DocOutcome::default();
    // _check_thumbnail
    let thumb = abs(run.thumbnail_dir.join(format!("{:07}.webp", d.pk)));
    if !thumb.is_file() {
        o.msgs.push((ERROR, "Thumbnail of document does not exist.".into()));
    } else {
        if let Err(e) = std::fs::read(&thumb) {
            o.msgs.push((ERROR, format!("Cannot read thumbnail file of document: {e}")));
        }
        o.seen.push(thumb);
    }
    // _check_original
    // Security review 2026-11-19 F9: DB name only via safe_join; unsafe → "does not exist"
    let src = abs(pl_consumer::files::join(run.originals_dir, &d.fname));
    if !src.is_file() {
        o.msgs.push((ERROR, "Original of document does not exist.".into()));
        o.invalid.push(Kind::Original);
    } else {
        match verify(run, d.pk, Kind::Original, &src, &d.checksum, &mut o) {
            Err(e) => o.msgs.push((ERROR, format!("Cannot read original file of document: {e}"))),
            Ok(Some(sum)) => o.msgs.push((ERROR, format!("Checksum mismatch. Stored: {}, actual: {sum}.", d.checksum))),
            Ok(None) => {}
        }
        o.seen.push(src);
    }
    // _check_archive
    match (&d.archive_checksum, &d.archive_filename) {
        (Some(_), None) => o.msgs.push((ERROR, "Document has an archive file checksum, but no archive filename.".into())),
        (None, Some(_)) => o.msgs.push((ERROR, "Document has an archive file, but its checksum is missing.".into())),
        (Some(ac), Some(af)) => {
            let ap = abs(pl_consumer::files::join(run.archive_dir, af));
            if !ap.is_file() {
                o.msgs.push((ERROR, "Archived version of document does not exist.".into()));
                o.invalid.push(Kind::Archive);
            } else {
                match verify(run, d.pk, Kind::Archive, &ap, ac, &mut o) {
                    Err(e) => o.msgs.push((ERROR, format!("Cannot read archive file of document: {e}"))),
                    Ok(Some(sum)) => o.msgs.push((ERROR, format!("Checksum mismatch of archived document. Stored: {ac}, actual: {sum}."))),
                    Ok(None) => {}
                }
                o.seen.push(ap);
            }
        }
        (None, None) => {}
    }
    // _check_content
    if d.content_empty {
        o.msgs.push((INFO, "Document contains no OCR data".into()));
    }
    o
}

/// `f` for all elements with `threads` threads; result in input order.
fn parallel_map<T: Sync, R: Send>(items: &[T], threads: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = threads.max(1).min(items.len().max(1));
    if threads == 1 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        local.push((i, f(item)));
                    }
                    local
                })
            })
            .collect();
        for h in handles {
            for (i, r) in h.join().expect("integrity worker thread panicked") {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter().map(|r| r.expect("every item processed")).collect()
}

async fn ensure_state_table(c: &Ctx) -> CliResult<()> {
    sqlx::raw_sql(MIGRATION_SQL).execute(&c.pool).await?;
    Ok(())
}

async fn load_state(c: &Ctx) -> CliResult<StateMap> {
    let rows = sqlx::query("SELECT document_id, kind, path, size, mtime_ns, checksum, verified_at FROM paperrust_integrity_state")
        .fetch_all(&c.pool)
        .await?;
    let mut map = StateMap::with_capacity(rows.len());
    for r in rows {
        let Some(kind) = Kind::parse(r.try_get::<&str, _>(1)?) else { continue };
        let document_id = i64::from(r.try_get::<i32, _>(0)?);
        map.insert(
            (document_id, kind),
            StateRow {
                document_id,
                kind,
                path: r.try_get(2)?,
                size: r.try_get(3)?,
                mtime_ns: r.try_get(4)?,
                checksum: r.try_get(5)?,
                verified_at: r.try_get(6)?,
            },
        );
    }
    Ok(map)
}

/// Write confirmed rows, remove invalid and orphaned ones.
async fn store_state(c: &Ctx, verified: &[StateRow], invalid: &[(i64, Kind)]) -> CliResult<()> {
    let mut tx = c.pool.begin().await?;
    for chunk in verified.chunks(5000) {
        let ids: Vec<i32> = chunk.iter().map(|r| r.document_id as i32).collect();
        let kinds: Vec<&str> = chunk.iter().map(|r| r.kind.as_str()).collect();
        let paths: Vec<&str> = chunk.iter().map(|r| r.path.as_str()).collect();
        let sizes: Vec<i64> = chunk.iter().map(|r| r.size).collect();
        let mtimes: Vec<i64> = chunk.iter().map(|r| r.mtime_ns).collect();
        let sums: Vec<&str> = chunk.iter().map(|r| r.checksum.as_str()).collect();
        let at: Vec<DateTime<Utc>> = chunk.iter().map(|r| r.verified_at).collect();
        sqlx::query(
            "INSERT INTO paperrust_integrity_state (document_id, kind, path, size, mtime_ns, checksum, verified_at)
             SELECT * FROM UNNEST($1::int4[], $2::varchar[], $3::text[], $4::int8[], $5::int8[], $6::varchar[], $7::timestamptz[])
             ON CONFLICT (document_id, kind) DO UPDATE SET path = EXCLUDED.path, size = EXCLUDED.size,
               mtime_ns = EXCLUDED.mtime_ns, checksum = EXCLUDED.checksum, verified_at = EXCLUDED.verified_at",
        )
        .bind(&ids)
        .bind(&kinds)
        .bind(&paths)
        .bind(&sizes)
        .bind(&mtimes)
        .bind(&sums)
        .bind(&at)
        .execute(&mut *tx)
        .await?;
    }
    if !invalid.is_empty() {
        let ids: Vec<i32> = invalid.iter().map(|(p, _)| *p as i32).collect();
        let kinds: Vec<&str> = invalid.iter().map(|(_, k)| k.as_str()).collect();
        sqlx::query(
            "DELETE FROM paperrust_integrity_state s USING UNNEST($1::int4[], $2::varchar[]) AS x(id, kind)
             WHERE s.document_id = x.id AND s.kind = x.kind",
        )
        .bind(&ids)
        .bind(&kinds)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "DELETE FROM paperrust_integrity_state s WHERE NOT EXISTS (SELECT 1 FROM documents_document d WHERE d.id = s.document_id
           AND (s.kind <> 'archive' OR d.archive_filename IS NOT NULL))",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

// ------------------------------------------------------------------ Check run

/// `check_sanity()` like upstream (complete, without state and report).
pub async fn check_sanity(c: &Ctx) -> CliResult<Messages> {
    Ok(check_sanity_with(c, &Options::default()).await?.0)
}

/// `check_sanity()` with the PaperRust options; messages identical to upstream as long as no
/// file is skipped.
pub async fn check_sanity_with(c: &Ctx, opts: &Options) -> CliResult<(Messages, Stats)> {
    let started = Instant::now();
    let media_root = c.cfg.media_root.clone();
    let mut present = tokio::task::spawn_blocking(move || {
        let mut s = BTreeSet::new();
        walk(&media_root, &mut s);
        s
    })
    .await
    .map_err(|e| CliError::Other(format!("integrity check: {e}")))?;
    present.remove(&abs(c.cfg.media_lock.clone()));
    let app_cfg = pl_files::db::app_config(&c.pool).await?;
    let logo = app_cfg.as_ref().and_then(|r| r.app_logo.clone()).filter(|s| !s.is_empty()).or_else(|| c.cfg.app_logo.clone());
    if let Some(l) = logo.filter(|s| !s.is_empty()) {
        present.remove(&abs(c.cfg.media_root.join(l.trim_start_matches('/'))));
    }
    let remote_tiff = pl_files::db::remote_ocr_active(app_cfg.as_ref(), &c.cfg);
    // Document.global_objects (also trash and versions), Meta.ordering = -created;
    // `content` itself is not loaded (only "empty?").
    let rows = sqlx::query(
        "SELECT id, filename, mime_type, checksum, archive_checksum, archive_filename, COALESCE(content, '') = ''
         FROM documents_document ORDER BY created DESC",
    )
    .fetch_all(&c.pool)
    .await?;
    let mut docs = Vec::with_capacity(rows.len());
    for r in rows {
        let pk = i64::from(r.try_get::<i32, _>(0)?);
        let filename: Option<String> = r.try_get(1)?;
        let mime: String = r.try_get(2)?;
        let fname = match filename {
            Some(f) if !f.is_empty() => f,
            _ => format!("{pk:07}{}", pl_files::naming::default_file_extension(&mime, remote_tiff && mime == "image/tiff")),
        };
        docs.push(DocRow {
            pk,
            fname,
            checksum: r.try_get(3)?,
            archive_checksum: r.try_get(4)?,
            archive_filename: r.try_get(5)?,
            content_empty: r.try_get(6)?,
        });
    }
    let state = if opts.incremental || opts.record_state {
        ensure_state_table(c).await?;
        if opts.incremental { load_state(c).await? } else { StateMap::new() }
    } else {
        StateMap::new()
    };
    let (opts2, dirs) = (opts.clone(), (c.cfg.thumbnail_dir.clone(), c.cfg.originals_dir.clone(), c.cfg.archive_dir.clone()));
    let n_docs = docs.len();
    let (docs, outcomes) = tokio::task::spawn_blocking(move || {
        let run =
            Run { opts: &opts2, state: &state, now: Utc::now(), thumbnail_dir: &dirs.0, originals_dir: &dirs.1, archive_dir: &dirs.2 };
        let outcomes = parallel_map(&docs, opts2.threads, |d| check_document(&run, d));
        (docs, outcomes)
    })
    .await
    .map_err(|e| CliError::Other(format!("integrity check: {e}")))?;

    let mut m = Messages::default();
    let mut stats = Stats { documents: n_docs, ..Stats::default() };
    let mut verified = Vec::new();
    let mut invalid = Vec::new();
    for (d, o) in docs.iter().zip(outcomes) {
        for p in &o.seen {
            present.remove(p);
        }
        for (level, msg) in o.msgs {
            m.add(Some(d.pk), level, msg);
        }
        stats.files_hashed += o.hashed;
        stats.files_skipped += o.skipped;
        stats.bytes_hashed += o.bytes;
        verified.extend(o.verified);
        invalid.extend(o.invalid.into_iter().map(|k| (d.pk, k)));
    }
    stats.orphaned_files = present.len();
    for f in present {
        m.warning(None, format!("Orphaned file in media dir: {}", f.display()));
    }
    if opts.record_state
        && let Err(e) = store_state(c, &verified, &invalid).await
    {
        tracing::warn!(target: LOG_TARGET, "Integrity check: could not store verified checksums: {e}");
    }
    stats.duration_ms = started.elapsed().as_millis();
    Ok((m, stats))
}

// ------------------------------------------------------------------ Report

fn level_name(level: u8) -> &'static str {
    match level {
        ERROR => "ERROR",
        WARNING => "WARNING",
        _ => "INFO",
    }
}

async fn titles(c: &Ctx, m: &Messages) -> HashMap<i64, String> {
    let pks: Vec<i32> = m.order.iter().flatten().filter_map(|p| i32::try_from(*p).ok()).collect();
    if pks.is_empty() {
        return HashMap::new();
    }
    sqlx::query_as::<_, (i32, String)>("SELECT id, title FROM documents_document WHERE id = ANY($1)")
        .bind(&pks)
        .fetch_all(&c.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(i, t)| (i64::from(i), t))
        .collect()
}

/// JSON report of a run (format 1).
pub fn report_json(
    m: &Messages,
    stats: &Stats,
    opts: &Options,
    trigger: &str,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    titles: &HashMap<i64, String>,
) -> Value {
    let status = if m.has_error {
        "error"
    } else if m.is_empty() {
        "ok"
    } else {
        "issues"
    };
    let mut issues = Vec::with_capacity(m.len());
    for pk in &m.order {
        for msg in m.messages.get(pk).map(Vec::as_slice).unwrap_or_default() {
            issues.push(json!({
                "level": level_name(msg.level),
                "document_id": pk,
                "title": pk.map(|p| titles.get(&p).cloned().unwrap_or_else(|| "Unknown".into())),
                "message": msg.message,
            }));
        }
    }
    json!({
        "format": 1,
        "started_at": started_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "finished_at": finished_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "duration_seconds": stats.duration_ms as f64 / 1000.0,
        "trigger": trigger,
        "mode": opts.mode(),
        "full_hash_days": opts.full_hash_days,
        "threads": opts.threads,
        "status": status,
        "result": task_summary(m),
        "documents": stats.documents,
        "files": {
            "hashed": stats.files_hashed,
            "skipped": stats.files_skipped,
            "bytes_hashed": stats.bytes_hashed,
            "orphaned": stats.orphaned_files,
        },
        "counts": {
            "document_errors": m.document_error_count,
            "document_warnings": m.document_warning_count,
            "document_infos": m.document_info_count,
            "global_warnings": m.global_warning_count,
            "messages": m.len(),
        },
        "issues": issues,
    })
}

/// Write the report to `dir` and trim to the newest `keep`. Returns: path.
pub fn write_report(dir: &Path, report: &Value, finished_at: DateTime<Utc>, keep: usize) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let base = format!("{REPORT_PREFIX}{}", finished_at.format("%Y%m%dT%H%M%S%.3fZ"));
    let mut path = dir.join(format!("{base}.json"));
    let mut i = 1;
    while path.exists() {
        path = dir.join(format!("{base}-{i}.json"));
        i += 1;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(report).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)?;
    prune_reports(dir, keep)?;
    Ok(path)
}

/// Keep only the newest `keep` reports (names are sortable by time).
pub fn prune_reports(dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(REPORT_PREFIX) && n.ends_with(".json")))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(keep);
    for p in names.into_iter().take(excess) {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// End line (logger `paperless.integrity`).
pub fn finish_line(m: &Messages, stats: &Stats, opts: &Options, report: Option<&Path>) -> String {
    let mut s = format!(
        "Integrity check ({}) finished in {:.1} s: {} document(s), {} file(s) hashed ({}), {} unchanged file(s) skipped, {} orphaned file(s). Result: {}",
        opts.mode(),
        stats.duration_ms as f64 / 1000.0,
        stats.documents,
        stats.files_hashed,
        human_bytes(stats.bytes_hashed),
        stats.files_skipped,
        stats.orphaned_files,
        task_summary(m),
    );
    if let Some(p) = report {
        s.push_str(&format!(" Report: {}", p.display()));
    }
    s
}

/// Check run with log lines and report (task `documents.tasks.sanity_check` and
/// `document_sanity_checker`). `trigger`: `"task"` or `"manage"`.
pub async fn run(c: &Ctx, opts: &Options, trigger: &str) -> CliResult<(Messages, Stats, Option<PathBuf>)> {
    let started_at = Utc::now();
    tracing::info!(
        target: LOG_TARGET,
        "Integrity check started (mode {}, {} thread(s), full re-hash after {} day(s))",
        opts.mode(),
        opts.threads,
        opts.full_hash_days
    );
    let (m, stats) = check_sanity_with(c, opts).await?;
    let finished_at = Utc::now();
    let mut report_path = None;
    if opts.report_keep > 0 {
        let t = titles(c, &m).await;
        let report = report_json(&m, &stats, opts, trigger, started_at, finished_at, &t);
        match write_report(&c.cfg.data_dir.join(REPORT_DIR), &report, finished_at, opts.report_keep) {
            Ok(p) => report_path = Some(p),
            Err(e) => tracing::warn!(target: LOG_TARGET, "Integrity check: could not write report: {e}"),
        }
    }
    let line = finish_line(&m, &stats, opts, report_path.as_deref());
    if m.has_error {
        tracing::warn!(target: LOG_TARGET, "{line}");
    } else {
        tracing::info!(target: LOG_TARGET, "{line}");
    }
    Ok((m, stats, report_path))
}

// ------------------------------------------------------------------ manage document_sanity_checker

fn label(level: u8) -> &'static str {
    match level {
        ERROR => "ERROR",
        WARNING => "WARN",
        _ => "INFO",
    }
}

const OPTS: &[args::Opt] = &[args::PROGRESS, args::flag(&["--incremental"], "incremental")];

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 0)?;
    let c = Ctx::connect().await?;
    // upstream: always complete. `--incremental` (PaperRust) uses the stored state;
    // if PAPERRUST_INTEGRITY_INCREMENTAL is on, a complete run also refreshes it.
    let mut opts = Options::from_env(&c.cfg.env)?;
    opts.incremental = a.flag("incremental");
    opts.record_state |= opts.incremental;
    let (m, _, _) = run(&c, &opts, "manage").await?;
    if !m.has_error && !m.has_warning && !m.has_info {
        println!("Sanity Check: No issues detected.");
        return Ok(());
    }
    let titles = titles(&c, &m).await;
    println!("Sanity Check Results");
    println!("{:<7} | {:<20} | Issue", "Level", "Document");
    for pk in &m.order {
        let doc_label = match pk {
            Some(p) => format!("#{p} {}", titles.get(p).map(String::as_str).unwrap_or("Unknown")),
            None => "(global)".to_string(),
        };
        for msg in m.messages.get(pk).map(Vec::as_slice).unwrap_or(&[]) {
            println!("{:<7} | {:<20} | {}", label(msg.level), doc_label, msg.message);
        }
    }
    let mut parts = Vec::new();
    if m.document_error_count > 0 {
        parts.push(format!("{} document(s) with errors", m.document_error_count));
    }
    if m.document_warning_count > 0 {
        parts.push(format!("{} document(s) with warnings", m.document_warning_count));
    }
    if m.document_info_count > 0 {
        parts.push(format!("{} document(s) with infos", m.document_info_count));
    }
    if m.global_warning_count > 0 {
        parts.push(format!("{} global warning(s)", m.global_warning_count));
    }
    if parts.is_empty() {
        println!("\nNo issues found.");
    } else {
        let summary = if parts.len() > 1 {
            format!("{} and {}", parts[..parts.len() - 1].join(", "), parts[parts.len() - 1])
        } else {
            parts[0].clone()
        };
        println!("\nFound {summary}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> pl_core::config::Env {
        pl_core::config::Env::from_pairs(pairs.iter().copied())
    }

    #[test]
    fn options_defaults_are_upstream_behaviour() {
        let o = Options::from_env(&env(&[])).unwrap();
        assert!(!o.incremental && !o.record_state);
        assert_eq!(o.full_hash_days, 28);
        assert_eq!(o.report_keep, 10);
        assert!((1..=4).contains(&o.threads));
        assert_eq!(o.mode(), "full");
        let d = Options::default();
        assert!(!d.incremental && !d.record_state && d.report_keep == 0);
    }

    #[test]
    fn options_from_env() {
        let o = Options::from_env(&env(&[
            ("PAPERRUST_INTEGRITY_INCREMENTAL", "yes"),
            ("PAPERRUST_INTEGRITY_FULL_HASH_DAYS", "7"),
            ("PAPERRUST_INTEGRITY_THREADS", "2"),
            ("PAPERRUST_INTEGRITY_REPORT_KEEP", "0"),
        ]))
        .unwrap();
        assert_eq!(o, Options { incremental: true, record_state: true, full_hash_days: 7, threads: 2, report_keep: 0 });
        assert_eq!(o.mode(), "incremental");
        // transition name PAPERLESS_RS_* counts as an alias
        assert!(Options::from_env(&env(&[("PAPERLESS_RS_INTEGRITY_INCREMENTAL", "1")])).unwrap().incremental);
        let neg =
            Options::from_env(&env(&[("PAPERRUST_INTEGRITY_FULL_HASH_DAYS", "-3"), ("PAPERRUST_INTEGRITY_REPORT_KEEP", "-1")])).unwrap();
        assert_eq!((neg.full_hash_days, neg.report_keep), (0, 0));
        assert!(Options::from_env(&env(&[("PAPERRUST_INTEGRITY_INCREMENTAL", "vielleicht")])).is_err());
        assert!(Options::from_env(&env(&[("PAPERRUST_INTEGRITY_THREADS", "x")])).is_err());
    }

    fn msgs(entries: &[(Option<i64>, u8, &str)]) -> Messages {
        let mut m = Messages::default();
        for (pk, l, s) in entries {
            m.add(*pk, *l, (*s).to_string());
        }
        m
    }

    #[test]
    fn messages_counts_like_upstream() {
        let m = msgs(&[
            (Some(3), ERROR, "a"),
            (Some(3), ERROR, "b"),
            (Some(3), INFO, "Document contains no OCR data"),
            (Some(1), INFO, "Document contains no OCR data"),
            (None, WARNING, "Orphaned file in media dir: x"),
            (None, WARNING, "Orphaned file in media dir: y"),
        ]);
        assert_eq!(m.order, vec![Some(3), Some(1), None]);
        assert_eq!((m.document_error_count, m.document_warning_count, m.document_info_count, m.global_warning_count), (1, 0, 2, 2));
        assert_eq!(m.len(), 6);
        assert_eq!(
            task_summary(&m),
            "1 document(s) with errors, 2 document(s) with infos, 2 global warning(s) found. Check logs for details."
        );
        assert_eq!(task_summary(&Messages::default()), "No issues detected.");
        assert_eq!(task_summary(&msgs(&[(Some(1), INFO, "x")])), "1 document(s) with infos found.");
    }

    #[test]
    fn parallel_map_keeps_order() {
        let items: Vec<u32> = (0..1000).collect();
        for t in [1, 3, 8] {
            assert_eq!(parallel_map(&items, t, |x| x * 2), items.iter().map(|x| x * 2).collect::<Vec<_>>());
        }
        assert!(parallel_map(&Vec::<u32>::new(), 4, |x| *x).is_empty());
    }

    fn hex_sha(b: &[u8]) -> String {
        use sha2::Digest;
        crate::export::fernet::hex_encode(&sha2::Sha256::digest(b))
    }

    struct Fx {
        _dir: tempfile::TempDir,
        thumbs: PathBuf,
        orig: PathBuf,
        arch: PathBuf,
    }

    fn fixture() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let (thumbs, orig, arch) = (dir.path().join("thumbnails"), dir.path().join("originals"), dir.path().join("archive"));
        for d in [&thumbs, &orig, &arch] {
            std::fs::create_dir_all(d).unwrap();
        }
        Fx { _dir: dir, thumbs, orig, arch }
    }

    fn doc(fx: &Fx, pk: i64, body: &[u8], archive: Option<&[u8]>) -> DocRow {
        std::fs::write(fx.thumbs.join(format!("{pk:07}.webp")), b"webp").unwrap();
        std::fs::write(fx.orig.join(format!("{pk:07}.pdf")), body).unwrap();
        if let Some(a) = archive {
            std::fs::write(fx.arch.join(format!("{pk:07}.pdf")), a).unwrap();
        }
        DocRow {
            pk,
            fname: format!("{pk:07}.pdf"),
            checksum: hex_sha(body),
            archive_checksum: archive.map(hex_sha),
            archive_filename: archive.map(|_| format!("{pk:07}.pdf")),
            content_empty: false,
        }
    }

    fn run_doc(fx: &Fx, opts: &Options, state: &StateMap, d: &DocRow) -> DocOutcome {
        let run = Run { opts, state, now: Utc::now(), thumbnail_dir: &fx.thumbs, originals_dir: &fx.orig, archive_dir: &fx.arch };
        check_document(&run, d)
    }

    /// Security review 2026-11-19 F9: a DB name with `..` (manipulated foreign DB) points to a
    /// real file outside originals/archive – it is not read, but counts as missing.
    #[test]
    fn db_filename_outside_root_counts_as_missing() {
        let fx = fixture();
        let opts = Options::default();
        let st = StateMap::new();
        let mut d = doc(&fx, 3, b"original", Some(b"archive"));
        // file next to the media folders, with a matching checksum
        std::fs::write(fx.orig.parent().unwrap().join("outside.pdf"), b"original").unwrap();
        d.fname = "../outside.pdf".into();
        d.archive_filename = Some("../outside.pdf".into());
        d.archive_checksum = Some(hex_sha(b"original"));
        let o = run_doc(&fx, &opts, &st, &d);
        let msgs: Vec<&str> = o.msgs.iter().map(|(_, m)| m.as_str()).collect();
        assert!(msgs.contains(&"Original of document does not exist."), "{msgs:?}");
        assert!(msgs.contains(&"Archived version of document does not exist."), "{msgs:?}");
        assert_eq!(o.hashed, 0);
        assert_eq!(o.seen.len(), 1, "nur das Vorschaubild");
        // Windows remainder (F12): stream name rejected only on Windows
        d.fname = "0000003.pdf:strom".into();
        d.archive_filename = None;
        d.archive_checksum = None;
        let o = run_doc(&fx, &opts, &st, &d);
        assert!(o.msgs.iter().any(|(_, m)| m == "Original of document does not exist."));
    }

    #[test]
    fn check_document_messages_and_order() {
        let fx = fixture();
        let opts = Options::default();
        let st = StateMap::new();
        let ok = doc(&fx, 1, b"original", Some(b"archive"));
        let o = run_doc(&fx, &opts, &st, &ok);
        assert!(o.msgs.is_empty(), "{:?}", o.msgs);
        assert_eq!((o.hashed, o.skipped, o.seen.len()), (2, 0, 3));
        assert!(o.verified.is_empty(), "no state without record_state");

        let mut bad = doc(&fx, 2, b"x", Some(b"y"));
        bad.checksum = "0".repeat(64);
        bad.archive_checksum = Some("1".repeat(64));
        bad.content_empty = true;
        std::fs::remove_file(fx.thumbs.join("0000002.webp")).unwrap();
        let o = run_doc(&fx, &opts, &st, &bad);
        let texts: Vec<(u8, String)> = o.msgs.clone();
        assert_eq!(
            texts,
            vec![
                (ERROR, "Thumbnail of document does not exist.".to_string()),
                (ERROR, format!("Checksum mismatch. Stored: {}, actual: {}.", "0".repeat(64), hex_sha(b"x"))),
                (ERROR, format!("Checksum mismatch of archived document. Stored: {}, actual: {}.", "1".repeat(64), hex_sha(b"y"))),
                (INFO, "Document contains no OCR data".to_string()),
            ]
        );
        assert_eq!(o.invalid, vec![Kind::Original, Kind::Archive]);

        let mut missing = doc(&fx, 3, b"z", Some(b"w"));
        std::fs::remove_file(fx.orig.join("0000003.pdf")).unwrap();
        std::fs::remove_file(fx.arch.join("0000003.pdf")).unwrap();
        let o = run_doc(&fx, &opts, &st, &missing);
        assert_eq!(
            o.msgs.iter().map(|m| m.1.as_str()).collect::<Vec<_>>(),
            vec!["Original of document does not exist.", "Archived version of document does not exist."]
        );
        missing.archive_filename = None;
        let o = run_doc(&fx, &opts, &st, &missing);
        assert_eq!(o.msgs[1].1, "Document has an archive file checksum, but no archive filename.");
        missing.archive_filename = Some("0000003.pdf".into());
        missing.archive_checksum = None;
        let o = run_doc(&fx, &opts, &st, &missing);
        assert_eq!(o.msgs[1].1, "Document has an archive file, but its checksum is missing.");
    }

    fn incremental(days: i64) -> Options {
        Options { incremental: true, record_state: true, full_hash_days: days, threads: 1, report_keep: 0 }
    }

    fn to_state(rows: Vec<StateRow>) -> StateMap {
        rows.into_iter().map(|r| ((r.document_id, r.kind), r)).collect()
    }

    #[test]
    fn incremental_skips_unchanged_and_rehashes_changes() {
        let fx = fixture();
        let opts = incremental(28);
        let d = doc(&fx, 5, b"original", Some(b"archive"));
        let first = run_doc(&fx, &opts, &StateMap::new(), &d);
        assert_eq!((first.hashed, first.skipped, first.verified.len()), (2, 0, 2));
        let st = to_state(first.verified);
        let second = run_doc(&fx, &opts, &st, &d);
        assert_eq!((second.hashed, second.skipped, second.bytes), (0, 2, 0));
        assert!(second.msgs.is_empty() && second.verified.is_empty());

        // confirmation expired → hash again
        let mut old = st.clone();
        for r in old.values_mut() {
            r.verified_at = Utc::now() - chrono::Duration::days(29);
        }
        assert_eq!(run_doc(&fx, &opts, &old, &d).hashed, 2);
        // 0 days = always hash
        assert_eq!(run_doc(&fx, &incremental(0), &st, &d).hashed, 2);
        // changed stored checksum (document reprocessed) → hash
        let mut d2 = d.clone();
        d2.checksum = "f".repeat(64);
        let o = run_doc(&fx, &opts, &st, &d2);
        assert_eq!((o.hashed, o.skipped), (1, 1));
        assert!(o.msgs[0].1.starts_with("Checksum mismatch. Stored: fff"));
        assert_eq!(o.invalid, vec![Kind::Original]);

        // content changed (different size) → detected
        std::fs::write(fx.orig.join("0000005.pdf"), b"tampered!").unwrap();
        let o = run_doc(&fx, &opts, &st, &d);
        assert_eq!((o.hashed, o.skipped), (1, 1));
        assert!(o.msgs[0].1.starts_with("Checksum mismatch. Stored: "), "{:?}", o.msgs);

        // same size, timestamp reset: only the full check finds it
        let p = fx.orig.join("0000005.pdf");
        std::fs::write(&p, b"original").unwrap();
        let fresh = to_state(run_doc(&fx, &opts, &StateMap::new(), &d).verified);
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::fs::write(&p, b"0riginal").unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(mtime).unwrap();
        assert!(run_doc(&fx, &opts, &fresh, &d).msgs.is_empty(), "silent change within the window is skipped");
        assert_eq!(run_doc(&fx, &Options::default(), &fresh, &d).msgs.len(), 1, "full mode finds it");
    }

    #[test]
    fn full_mode_ignores_state() {
        let fx = fixture();
        let d = doc(&fx, 9, b"a", None);
        let st = to_state(run_doc(&fx, &incremental(28), &StateMap::new(), &d).verified);
        let o = run_doc(&fx, &Options { record_state: true, ..Options::default() }, &st, &d);
        assert_eq!((o.hashed, o.skipped, o.verified.len()), (1, 0, 1));
    }

    #[test]
    fn reports_written_and_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let m = msgs(&[(Some(7), ERROR, "Original of document does not exist."), (None, WARNING, "Orphaned file in media dir: /m/x")]);
        let stats = Stats { documents: 3, files_hashed: 4, files_skipped: 1, bytes_hashed: 2048, orphaned_files: 1, duration_ms: 1500 };
        let opts = incremental(28);
        let t0 = DateTime::parse_from_rfc3339("2026-11-08T00:30:00Z").unwrap().with_timezone(&Utc);
        let titles = HashMap::from([(7, "Rechnung".to_string())]);
        let r = report_json(&m, &stats, &opts, "task", t0, t0 + chrono::Duration::seconds(2), &titles);
        assert_eq!(r["status"], "error");
        assert_eq!(r["mode"], "incremental");
        assert_eq!(r["result"], "1 document(s) with errors, 1 global warning(s) found. Check logs for details.");
        assert_eq!(r["files"]["skipped"], 1);
        assert_eq!(
            r["issues"][0],
            json!({"level": "ERROR", "document_id": 7, "title": "Rechnung", "message": "Original of document does not exist."})
        );
        assert_eq!(r["issues"][1]["document_id"], Value::Null);
        assert_eq!(r["issues"][1]["title"], Value::Null);
        assert_eq!(r["started_at"], "2026-11-08T00:30:00.000Z");
        assert_eq!(report_json(&Messages::default(), &stats, &opts, "task", t0, t0, &titles)["status"], "ok");

        std::fs::write(dir.path().join("other.json"), "{}").unwrap();
        let mut paths = Vec::new();
        for i in 0..5 {
            paths.push(write_report(dir.path(), &r, t0 + chrono::Duration::minutes(i), 3).unwrap());
        }
        // same point in time → unique name
        let dup = write_report(dir.path(), &r, t0 + chrono::Duration::minutes(4), 3).unwrap();
        assert_ne!(dup, paths[4]);
        let mut left: Vec<String> =
            std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "integrity-20261108T003300.000Z.json",
                "integrity-20261108T003400.000Z-1.json",
                "integrity-20261108T003400.000Z.json",
                "other.json"
            ]
        );
        let back: Value = serde_json::from_slice(&std::fs::read(&paths[4]).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn finish_line_text() {
        let m = msgs(&[(Some(1), INFO, "Document contains no OCR data")]);
        let stats =
            Stats { documents: 2, files_hashed: 3, files_skipped: 1, bytes_hashed: 3 * 1024 * 1024, orphaned_files: 0, duration_ms: 2500 };
        assert_eq!(
            finish_line(&m, &stats, &Options { threads: 2, ..Options::default() }, Some(Path::new("r.json"))),
            "Integrity check (full) finished in 2.5 s: 2 document(s), 3 file(s) hashed (3.0 MiB), 1 unchanged file(s) skipped, 0 orphaned file(s). Result: 1 document(s) with infos found. Report: r.json"
        );
        assert_eq!(human_bytes(10), "10 B");
    }
}
