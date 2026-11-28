// upstream: src/documents/search/_backend.py@v3.4.2 (WriteBatch, TantivyBackend.open,
//           add_or_update, remove, batch_update, rebuild), _schema.py (open_or_rebuild_index,
//           rebuild_in_progress, wipe_index, _write_sentinels), documents/tasks.py
//           (index_optimize, index_document, remove_document_from_index),
//           documents/management/commands/document_index.py (reindex / optimize)
//           tantivy-py 0.26.2 src/index.rs (Index(schema, path, reuse=True) = open_or_create,
//           Index.writer(heap_size=128_000_000, num_threads=0) = tantivy Index::writer(heap)).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pl_search::schema::{REBUILD_MARKER, SETTINGS_FILE};
use sqlx::PgPool;
use tantivy::query::TermQuery;
use tantivy::schema::{IndexRecordOption, Schema};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

use crate::db::{self, LoadMode};
use crate::doc::build_tantivy_doc;
use crate::error::{IndexerError, Result};
use crate::lock::{BATCH_LOCK_TIMEOUT, IndexFileLock, LOCK_FILE, LOCK_TIMEOUT};
use crate::record::DocumentRecord;
use crate::settings::{OpenOutcome, clear_rebuild_marker, open_or_create, open_or_rebuild_index, touch_rebuild_marker, write_sentinels};

/// tantivy-py `Index.writer()` default heap (used by `WriteBatch`).
pub const WRITER_HEAP_BYTES: usize = 128_000_000;
/// `rebuild(writer_heap_bytes=512_000_000)` default.
pub const REBUILD_HEAP_BYTES: usize = 512_000_000;
/// BEFUNDE B8: attempts per write operation on transient file errors
/// ([`IndexerError::is_transient`]); pauses of 0.25 / 0.5 / 1 / 2 s in between.
pub const WRITE_ATTEMPTS: u32 = 5;
const WRITE_RETRY_BASE: Duration = Duration::from_millis(250);

/// `settings.INDEX_DIR` + `settings.SEARCH_LANGUAGE`.
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    pub index_dir: PathBuf,
    pub language: Option<String>,
}

impl IndexerConfig {
    /// From `PAPERLESS_DATA_DIR`, `PAPERLESS_SEARCH_LANGUAGE`, `PAPERLESS_OCR_LANGUAGE`
    /// (same derivation as pl-search).
    pub fn from_env() -> Self {
        let s = pl_search::SearchSettings::from_env();
        Self { index_dir: s.index_dir, language: s.language }
    }

    fn lang(&self) -> Option<&str> {
        self.language.as_deref()
    }
}

/// Result of the convenience `add_or_update` / `remove`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    /// document not found -> nothing written (index_document skips it)
    Skipped,
    /// lock exhausted after all retries: upstream schedules the deferred Celery task
    /// (`index_document` / `remove_document_from_index`, countdown=60). The caller
    /// (task queue) must do that – this crate has no queue.
    Deferred,
}

#[derive(Debug, Clone)]
pub struct RebuildOptions {
    /// `--heap-size-mb` * 1_000_000; default 512 MB.
    pub writer_heap_bytes: usize,
    /// `--recreate`: wipe before (rebuild wipes anyway; kept for the CLI contract).
    pub recreate: bool,
    /// `--if-needed`: skip when `needs_rebuild()` is false.
    pub if_needed: bool,
    /// Hold `.tantivy.lock` during the rebuild (upstream does not; see contract).
    pub take_lock: bool,
}

impl Default for RebuildOptions {
    fn default() -> Self {
        Self { writer_heap_bytes: REBUILD_HEAP_BYTES, recreate: false, if_needed: false, take_lock: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildStats {
    pub documents: usize,
    /// `--if-needed` and the index was up to date ("Search index is up to date.")
    pub skipped_up_to_date: bool,
}

/// Process-level index writer (`TantivyBackend` writer half).
#[derive(Debug, Clone)]
pub struct Indexer {
    cfg: IndexerConfig,
}

impl Indexer {
    /// `TantivyBackend.open()` side effects: wipe+recreate on settings mismatch or
    /// corruption. Like upstream, a missing directory is left alone.
    pub fn open(cfg: IndexerConfig) -> Result<(Self, OpenOutcome)> {
        let outcome = open_or_rebuild_index(&cfg.index_dir, cfg.lang())?;
        Ok((Self { cfg }, outcome))
    }

    /// Without the open side effects (e.g. for a fresh rebuild target).
    pub fn new(cfg: IndexerConfig) -> Self {
        Self { cfg }
    }

    pub fn config(&self) -> &IndexerConfig {
        &self.cfg
    }

    /// `batch_update(lock_timeout)`: acquire `.tantivy.lock` (4 attempts), open a FRESH
    /// index (fresh ManagedDirectory, see upstream comment) and a writer.
    pub async fn batch(&self, lock_timeout: Duration) -> Result<WriteBatch> {
        let dir = &self.cfg.index_dir;
        if !dir.exists() {
            return Err(IndexerError::MissingIndexDir(dir.display().to_string()));
        }
        let lock = IndexFileLock::acquire_with_retries(dir, lock_timeout).await?;
        let index = open_or_create(dir, self.cfg.lang())?;
        let writer: IndexWriter<TantivyDocument> = index.writer(WRITER_HEAP_BYTES)?;
        Ok(WriteBatch { schema: index.schema(), writer: Some(writer), _index: index, lock: Some(lock) })
    }

    /// Batch for attempt `attempt` (0 = first). From the second attempt on (BEFUNDE B8) the
    /// fresh writer first removes orphans of a failed commit (files that appear in no
    /// `meta.json`, e.g. an already written `.del` file) and shifts the opstamp
    /// sequence by `attempt` (deletion of a never-assigned id), so that the commit
    /// produces different file names than the failed one – an orphan that cannot be deleted yet (Windows: "pending deletion", held open by a
    /// reader or scanner) can then no longer block the attempt.
    pub(crate) async fn batch_attempt(&self, lock_timeout: Duration, attempt: u32) -> Result<WriteBatch> {
        let batch = self.batch(lock_timeout).await?;
        if attempt > 0 {
            batch.prepare_retry(attempt)?;
        }
        Ok(batch)
    }

    /// Performs one complete write operation (lock, fresh writer, load, write, commit)
    /// and repeats it on transient file errors up to [`WRITE_ATTEMPTS`] times with a
    /// fresh writer (BEFUNDE B8). Other errors (also `LockExhausted`) return immediately.
    pub(crate) async fn with_retry<T, F, Fut>(&self, what: &str, mut op: F) -> Result<T>
    where
        F: FnMut(u32) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut attempt = 0u32;
        loop {
            match op(attempt).await {
                Err(e) if e.is_transient() && attempt + 1 < WRITE_ATTEMPTS => {
                    let pause = WRITE_RETRY_BASE * 2u32.pow(attempt);
                    tracing::warn!(
                        "search index {what}: {e}; retrying with a fresh writer in {} ms (attempt {}/{WRITE_ATTEMPTS})",
                        pause.as_millis(),
                        attempt + 2
                    );
                    tokio::time::sleep(pause).await;
                    attempt += 1;
                }
                Err(e) if e.is_transient() => {
                    tracing::error!("search index {what}: {e}; giving up after {WRITE_ATTEMPTS} attempts");
                    return Err(e);
                }
                r => return r,
            }
        }
    }

    /// `batch_update()` with the default 30 s per-attempt lock timeout.
    pub async fn batch_default(&self) -> Result<WriteBatch> {
        self.batch(BATCH_LOCK_TIMEOUT).await
    }

    /// `TantivyBackend.add_or_update(document)` (10 s lock timeout, deferred on
    /// exhaustion). `doc_id` may be a version; it is indexed as its root.
    /// BEFUNDE B8: transient file errors are retried with a fresh writer; if they persist,
    /// the function reports `Deferred` (the caller enqueues `index_document` with countdown 60) instead of
    /// losing the change.
    pub async fn add_or_update(&self, pool: &PgPool, doc_id: i64) -> Result<WriteOutcome> {
        let r = self
            .with_retry("add_or_update", |attempt| async move {
                let mut batch = self.batch_attempt(LOCK_TIMEOUT, attempt).await?;
                let written = batch.add_or_update(pool, doc_id).await?;
                batch.commit()?;
                Ok(written)
            })
            .await;
        match r {
            Ok(written) => Ok(if written { WriteOutcome::Written } else { WriteOutcome::Skipped }),
            Err(IndexerError::LockExhausted { attempts, .. }) => {
                tracing::error!(
                    "Search index lock exhausted for document {doc_id} after {attempts} attempts; \
                     scheduling deferred index write"
                );
                Ok(WriteOutcome::Deferred)
            }
            Err(e) if e.is_transient() => {
                tracing::error!("Search index write for document {doc_id} failed ({e}); scheduling deferred index write");
                Ok(WriteOutcome::Deferred)
            }
            Err(e) => Err(e),
        }
    }

    /// `TantivyBackend.remove(doc_id)`.
    pub async fn remove(&self, doc_id: i64) -> Result<WriteOutcome> {
        let r = self
            .with_retry("remove", |attempt| async move {
                let mut batch = self.batch_attempt(LOCK_TIMEOUT, attempt).await?;
                batch.remove(doc_id)?;
                batch.commit()
            })
            .await;
        match r {
            Ok(()) => Ok(WriteOutcome::Written),
            Err(IndexerError::LockExhausted { attempts, .. }) => {
                tracing::error!(
                    "Search index lock exhausted for doc_id {doc_id} after {attempts} attempts; \
                     scheduling deferred index removal"
                );
                Ok(WriteOutcome::Deferred)
            }
            Err(e) if e.is_transient() => {
                tracing::error!("Search index removal of doc_id {doc_id} failed ({e}); scheduling deferred index removal");
                Ok(WriteOutcome::Deferred)
            }
            Err(e) => Err(e),
        }
    }

    /// `tasks.index_document(document_id)`: `Document.objects.get` (not trashed) or
    /// skip; lock errors propagate (Celery autoretry).
    pub async fn index_document_task(&self, pool: &PgPool, doc_id: i64) -> Result<WriteOutcome> {
        if db::root_document_ids(pool, &[doc_id]).await?.is_empty() {
            tracing::info!("index_document: document {doc_id} no longer exists; skipping");
            return Ok(WriteOutcome::Skipped);
        }
        let written = self
            .with_retry("index_document", |attempt| async move {
                let mut batch = self.batch_attempt(BATCH_LOCK_TIMEOUT, attempt).await?;
                let written = batch.add_or_update(pool, doc_id).await?;
                batch.commit()?;
                Ok(written)
            })
            .await?;
        Ok(if written { WriteOutcome::Written } else { WriteOutcome::Skipped })
    }

    /// `tasks.remove_document_from_index(doc_id)`.
    pub async fn remove_document_task(&self, doc_id: i64) -> Result<()> {
        self.with_retry("remove_document_from_index", |attempt| async move {
            let mut batch = self.batch_attempt(BATCH_LOCK_TIMEOUT, attempt).await?;
            batch.remove(doc_id)?;
            batch.commit()
        })
        .await
    }

    /// `with backend.batch_update() as batch: batch.add_or_update_ids(ids)` (e.g.
    /// `bulk_update_documents`) as one whole write operation with retry on transient
    /// file errors (BEFUNDE B8). Returns: number of root documents written.
    pub async fn add_or_update_ids_task(&self, pool: &PgPool, ids: &[i64]) -> Result<usize> {
        self.with_retry("add_or_update_ids", |attempt| async move {
            let mut batch = self.batch_attempt(BATCH_LOCK_TIMEOUT, attempt).await?;
            let n = batch.add_or_update_ids(pool, ids).await?;
            batch.commit()?;
            Ok(n)
        })
        .await
    }

    /// Upsert already loaded records and remove ids in one batch (tests, other data sources).
    /// Retried like [`Indexer::add_or_update`] (BEFUNDE B8).
    pub async fn write_records(&self, upserts: &[DocumentRecord], removals: &[i64]) -> Result<()> {
        self.with_retry("write_records", |attempt| async move {
            let mut b = self.batch_attempt(LOCK_TIMEOUT, attempt).await?;
            for id in removals {
                b.remove(*id)?;
            }
            for r in upserts {
                b.add_or_update_record(r)?;
            }
            b.commit()
        })
        .await
    }

    /// Number of documents in the index (alive, without deletions); `None` if it cannot be opened.
    pub fn document_count(&self) -> Option<u64> {
        indexed_document_count(&self.cfg.index_dir, self.cfg.lang())
    }

    /// `index_optimize` / `document_index optimize`: a no-op upstream.
    pub fn optimize(&self) {
        tracing::info!("index_optimize is a no-op — Tantivy manages segment merging automatically.");
    }

    /// `document_index reindex` → `TantivyBackend.rebuild(documents)`.
    ///
    /// PaperRust: the new index is built in [`REBUILD_STAGING`] and swapped in only when it is
    /// complete (see [`swap_in`]); on any error the previous index stays usable. Upstream wipes
    /// the directory first. `--recreate` therefore needs no upfront wipe (the swap removes every
    /// old file anyway).
    pub async fn rebuild(&self, pool: &PgPool, opts: &RebuildOptions) -> Result<RebuildStats> {
        let dir = self.cfg.index_dir.clone();
        if opts.if_needed && dir.exists() && !crate::settings::needs_rebuild(&dir, self.cfg.lang()) {
            // PaperRust: settings match, but is the content there? An index stamped "current" can still
            // be empty or far behind the database (restored backup, switch from paperless-ngx after a
            // start on an empty database – NAS check 2026-11-21: 0 search hits until a manual reindex).
            // A failed count must not pass for "0 documents" (that would report "up to date").
            let in_db = db::count_root_docs(pool).await.inspect_err(|e| {
                tracing::error!("Could not count the documents in the database to check the search index: {e}");
            })?;
            let in_index = indexed_document_count(&dir, self.cfg.lang()).unwrap_or(0);
            if index_incomplete(in_index, in_db) {
                tracing::warn!("Search index incomplete ({in_index} of {in_db} documents) - rebuilding.");
            } else {
                // A similar count does not prove it is THIS archive (database switched or restored to
                // another archive of similar size): compare a sample of indexed (id, checksum) pairs.
                let sample = indexed_sample(&dir, self.cfg.lang(), IDENTITY_SAMPLE).unwrap_or_default();
                let matched = matching_in_db(pool, &sample).await.inspect_err(|e| {
                    tracing::error!("Could not compare the search index with the database: {e}");
                })?;
                if !index_from_other_archive(sample.len(), matched) {
                    tracing::info!("Search index is up to date.");
                    return Ok(RebuildStats { documents: 0, skipped_up_to_date: true });
                }
                tracing::warn!(
                    "Search index does not match the database ({matched} of {} sampled documents found) - rebuilding.",
                    sample.len()
                );
            }
        }
        // upstream assumes INDEX_DIR exists (paperless creates it at startup).
        std::fs::create_dir_all(&dir)?;
        let _lock = if opts.take_lock { Some(IndexFileLock::acquire_with_retries(&dir, BATCH_LOCK_TIMEOUT).await?) } else { None };
        // L3: what has been noted up to here is covered by the rebuild from the DB
        let dirty_seq = crate::dirty::max_seq(pool).await.unwrap_or(None);
        let ids = db::rebuild_root_ids(pool).await?;
        let count = self.rebuild_from_db(&dir, opts.writer_heap_bytes, pool, &ids).await?;
        if let Some(seq) = dirty_seq
            && let Err(e) = crate::dirty::clear_upto(pool, seq).await
        {
            tracing::warn!("search index dirty list not cleared after rebuild: {e}");
        }
        Ok(RebuildStats { documents: count, skipped_up_to_date: false })
    }

    /// Rebuild from already loaded records (tests, other data sources).
    pub fn rebuild_from_records(&self, records: &[DocumentRecord], heap: usize) -> Result<usize> {
        let dir = self.cfg.index_dir.clone();
        std::fs::create_dir_all(&dir)?;
        let staging = Staging::create(&dir)?;
        let (index, writer) = self.open_staging(&staging, heap)?;
        let schema = index.schema();
        for rec in records {
            writer.add_document(build_tantivy_doc(&schema, rec)?)?;
        }
        self.finish_rebuild(&dir, staging, index, writer)?;
        Ok(records.len())
    }

    /// Fresh empty index in the staging directory (the live index is not touched).
    fn open_staging(&self, staging: &Staging, heap: usize) -> Result<(Index, IndexWriter<TantivyDocument>)> {
        let index = open_or_create(&staging.path, self.cfg.lang())?;
        let writer: IndexWriter<TantivyDocument> = index.writer(heap)?;
        Ok((index, writer))
    }

    async fn rebuild_from_db(&self, dir: &Path, heap: usize, pool: &PgPool, ids: &[i64]) -> Result<usize> {
        let staging = Staging::create(dir)?;
        let (index, writer) = self.open_staging(&staging, heap)?;
        let schema = index.schema();
        let mut count = 0usize;
        // Stream chunk by chunk (_DocumentViewerStream, chunk_size=1000).
        for chunk in ids.chunks(db::CHUNK_SIZE) {
            let records = db::load_records(pool, chunk, LoadMode::REBUILD).await?;
            for rec in &records {
                writer.add_document(build_tantivy_doc(&schema, rec)?)?;
                count += 1;
            }
        }
        // On error above writer, index and staging directory are dropped; the live index stays.
        self.finish_rebuild(dir, staging, index, writer)?;
        Ok(count)
    }

    /// Commit the staged index and swap it in.
    fn finish_rebuild(&self, dir: &Path, staging: Staging, index: Index, mut writer: IndexWriter<TantivyDocument>) -> Result<()> {
        writer.commit()?;
        writer.wait_merging_threads()?;
        drop(index);
        swap_in(dir, &staging.path, self.cfg.lang())?;
        drop(staging);
        Ok(())
    }
}

/// Sub-directory of the index directory in which a rebuild builds the new index (same file
/// system as the live index, so the final moves are plain renames; tantivy ignores
/// sub-directories, and its garbage collection only touches files it manages).
pub const REBUILD_STAGING: &str = ".rebuild-staging";

/// Staging directory of a rebuild; removed when dropped (also after an error).
struct Staging {
    path: PathBuf,
}

impl Staging {
    fn create(index_dir: &Path) -> Result<Self> {
        let path = index_dir.join(REBUILD_STAGING);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!("search index staging directory not removed: {e}");
        }
    }
}

/// Files of the live directory that the swap never deletes as "old index" files.
fn is_kept_name(name: &str) -> bool {
    name == "meta.json"
        || name == ".managed.json"
        || name == SETTINGS_FILE
        || name == REBUILD_MARKER
        || name == LOCK_FILE
        || name.starts_with(".tantivy")
}

/// `std::fs::rename` over an existing file, retried briefly: on Windows a reader that has the
/// target open for a moment (index watcher, virus scanner) makes the replace fail (BEFUNDE B8).
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    let mut attempt = 0u32;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt + 1 < WRITE_ATTEMPTS && e.kind() != std::io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(100) * 2u32.pow(attempt));
                attempt += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Moves the complete index from `staging` into `dir`. Segment files have unique (UUID) names,
/// so they are moved in next to the old ones first; replacing `meta.json` (one rename) is the
/// actual switch for readers and writers; afterwards the old files are removed (best effort:
/// a file a reader still maps on Windows stays until the next rebuild). The rebuild marker covers
/// a crash in the middle (the next start rebuilds again).
fn swap_in(dir: &Path, staging: &Path, language: Option<&str>) -> Result<()> {
    touch_rebuild_marker(dir)?;
    let mut new_files: HashSet<std::ffi::OsString> = HashSet::new();
    for entry in std::fs::read_dir(staging)? {
        let entry = entry?;
        let name = entry.file_name();
        if !entry.file_type()?.is_file() || is_kept_name(&name.to_string_lossy()) {
            continue;
        }
        std::fs::rename(entry.path(), dir.join(&name))?;
        new_files.insert(name);
    }
    write_sentinels(dir, language)?;
    replace_file(&staging.join("meta.json"), &dir.join("meta.json"))?;
    if staging.join(".managed.json").exists() {
        replace_file(&staging.join(".managed.json"), &dir.join(".managed.json"))?;
    }
    let old: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(std::result::Result::ok)
        .filter(|e| {
            let name = e.file_name();
            name != REBUILD_STAGING && !new_files.contains(&name) && !is_kept_name(&name.to_string_lossy())
        })
        .map(|e| e.path())
        .collect();
    let remove = |p: &Path| if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    let failed: Vec<&PathBuf> = old.iter().filter(|p| remove(p).is_err()).collect();
    if !failed.is_empty() {
        // a reader in this process reloads shortly after the meta.json switch and lets go of them
        std::thread::sleep(Duration::from_secs(1));
        let left = failed.iter().filter(|p| remove(p).is_err()).count();
        if left > 0 {
            tracing::debug!("search index rebuild: {left} old file(s) still in use, removed by the next rebuild");
        }
    }
    clear_rebuild_marker(dir)?;
    Ok(())
}

/// `WriteBatch`: everything is committed at once by [`WriteBatch::commit`]; dropping
/// it without commit discards the changes (upstream: exception inside the `with`).
pub struct WriteBatch {
    schema: Schema,
    writer: Option<IndexWriter<TantivyDocument>>,
    _index: Index,
    lock: Option<IndexFileLock>,
}

impl WriteBatch {
    fn writer(&self) -> Result<&IndexWriter<TantivyDocument>> {
        self.writer.as_ref().ok_or_else(|| IndexerError::InvalidValue("WriteBatch already finished".into()))
    }

    /// Preparation of a retry (see [`Indexer::batch_attempt`]).
    fn prepare_retry(&self, attempt: u32) -> Result<()> {
        let writer = self.writer()?;
        match writer.garbage_collect_files().wait() {
            Ok(r) => {
                if !r.failed_to_delete_files.is_empty() {
                    tracing::debug!("search index: {} orphan file(s) not deletable yet", r.failed_to_delete_files.len());
                }
            }
            Err(e) => tracing::debug!("search index garbage collection before retry failed: {e}"),
        }
        let field = self.schema.get_field("id")?;
        for _ in 0..attempt {
            writer.delete_term(Term::from_field_u64(field, u64::MAX));
        }
        Ok(())
    }

    /// `WriteBatch.remove(doc_id)`: `delete_documents_by_query(term_query(id))`.
    pub fn remove(&mut self, doc_id: i64) -> Result<()> {
        let id = u64::try_from(doc_id).map_err(|_| IndexerError::InvalidValue("negative document id".into()))?;
        let field = self.schema.get_field("id")?;
        let query = TermQuery::new(Term::from_field_u64(field, id), IndexRecordOption::Basic);
        self.writer()?.delete_query(Box::new(query))?;
        Ok(())
    }

    /// Upsert of an already loaded record (`remove(pk)` + `add_document`).
    pub fn add_or_update_record(&mut self, rec: &DocumentRecord) -> Result<()> {
        self.remove(rec.id)?;
        let doc = build_tantivy_doc(&self.schema, rec)?;
        self.writer()?.add_document(doc)?;
        Ok(())
    }

    /// `WriteBatch.add_or_update(document)`: a version is indexed as its root.
    /// Returns false when the document does not exist.
    pub async fn add_or_update(&mut self, pool: &PgPool, doc_id: i64) -> Result<bool> {
        let Some(root) = db::root_of(pool, doc_id).await? else {
            return Ok(false);
        };
        let records = db::load_records(pool, &[root], LoadMode::SINGLE).await?;
        let Some(rec) = records.first() else {
            return Ok(false);
        };
        self.add_or_update_record(rec)?;
        Ok(true)
    }

    /// `WriteBatch.add_or_update_ids(ids)`: roots of the (not trashed) ids, bulk loaded;
    /// unknown/trashed ids are skipped silently.
    pub async fn add_or_update_ids(&mut self, pool: &PgPool, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let roots = db::root_document_ids(pool, ids).await?;
        let mut n = 0;
        for chunk in roots.chunks(db::CHUNK_SIZE) {
            let mut records = db::load_records(pool, chunk, LoadMode::BATCH_IDS).await?;
            // queryset order: Document.Meta.ordering ("-created",)
            records.sort_by_key(|r| std::cmp::Reverse(r.created));
            for rec in &records {
                self.add_or_update_record(rec)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// `__exit__` without exception: commit, wait for merges, release writer, then
    /// the file lock.
    pub fn commit(mut self) -> Result<()> {
        let result = (|| -> Result<()> {
            let mut writer = self.writer.take().ok_or_else(|| IndexerError::InvalidValue("WriteBatch already finished".into()))?;
            writer.commit()?;
            writer.wait_merging_threads()?;
            Ok(())
        })();
        self.writer = None;
        self.lock = None;
        result
    }
}

impl Drop for WriteBatch {
    fn drop(&mut self) {
        // writer first (releases tantivy's .tantivy-writer.lock), then .tantivy.lock
        self.writer = None;
        self.lock = None;
    }
}

/// Documents in the index at `dir` (`None` if it cannot be opened).
pub fn indexed_document_count(dir: &Path, language: Option<&str>) -> Option<u64> {
    let index = Index::open_in_dir(dir).ok()?;
    pl_search::tokenizer::register_tokenizers(&index, language);
    Some(index.reader().ok()?.searcher().num_docs())
}

/// Number of indexed documents compared with the database by `reindex --if-needed`.
pub const IDENTITY_SAMPLE: usize = 64;

/// Up to `n` `(id, checksum)` pairs of live documents in the index, spread evenly over all
/// documents (deterministic for a given index). `None` if the index cannot be opened.
pub fn indexed_sample(dir: &Path, language: Option<&str>, n: usize) -> Option<Vec<(i64, String)>> {
    use tantivy::schema::Value;
    let index = Index::open_in_dir(dir).ok()?;
    pl_search::tokenizer::register_tokenizers(&index, language);
    let schema = index.schema();
    let (id_f, sum_f) = (schema.get_field("id").ok()?, schema.get_field("checksum").ok()?);
    let searcher = index.reader().ok()?.searcher();
    let total = usize::try_from(searcher.num_docs()).ok()?;
    if total == 0 || n == 0 {
        return Some(Vec::new());
    }
    let step = total.div_ceil(n).max(1);
    let mut out = Vec::with_capacity(n.min(total));
    let mut seen = 0usize;
    for (ord, seg) in searcher.segment_readers().iter().enumerate() {
        let alive = seg.alive_bitset();
        for doc_id in 0..seg.max_doc() {
            if alive.is_some_and(|a| !a.is_alive(doc_id)) {
                continue;
            }
            seen += 1;
            if !(seen - 1).is_multiple_of(step) || out.len() >= n {
                continue;
            }
            let addr = tantivy::DocAddress::new(u32::try_from(ord).ok()?, doc_id);
            let Ok(doc) = searcher.doc::<TantivyDocument>(addr) else { continue };
            let id = doc.get_first(id_f).and_then(|v| v.as_u64()).and_then(|v| i64::try_from(v).ok());
            let sum = doc.get_first(sum_f).and_then(|v| v.as_str().map(str::to_string));
            if let (Some(id), Some(sum)) = (id, sum) {
                out.push((id, sum));
            }
        }
    }
    Some(out)
}

/// How many of the sampled `(id, checksum)` pairs exist as live root documents with the same
/// checksum in the database.
async fn matching_in_db(pool: &PgPool, sample: &[(i64, String)]) -> Result<usize> {
    if sample.is_empty() {
        return Ok(0);
    }
    let ids: Vec<i64> = sample.iter().map(|(id, _)| *id).collect();
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id::int8, checksum FROM documents_document \
         WHERE id = ANY($1::int8[]) AND deleted_at IS NULL AND root_document_id IS NULL",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    Ok(count_matches(sample, rows))
}

/// Sampled pairs that occur with the same checksum among the database rows.
fn count_matches(sample: &[(i64, String)], db_rows: Vec<(i64, String)>) -> usize {
    let in_db: std::collections::HashMap<i64, String> = db_rows.into_iter().collect();
    sample.iter().filter(|(id, sum)| in_db.get(id) == Some(sum)).count()
}

/// PaperRust: the index belongs to another archive if fewer than half of the sampled indexed
/// documents exist with the same id and checksum in the database (in a consistent state nearly
/// all do; incremental updates keep id and checksum in step).
pub fn index_from_other_archive(sampled: usize, matched: usize) -> bool {
    sampled > 0 && matched.saturating_mul(2) < sampled
}

/// PaperRust: the index counts as incomplete if it is empty while the database has documents, or if
/// it holds less than half of them. Small differences are left to the incremental updates (L3
/// dirty list); a full rebuild for each stray document would slow down every start.
pub fn index_incomplete(in_index: u64, in_db: i64) -> bool {
    let in_db = u64::try_from(in_db).unwrap_or(0);
    in_db > 0 && (in_index == 0 || in_index.saturating_mul(2) < in_db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tantivy::collector::Count;
    use tantivy::query::AllQuery;

    fn record(id: i64, title: &str) -> DocumentRecord {
        let mut r = crate::doc::tests::sample();
        r.id = id;
        r.title = title.into();
        r
    }

    #[test]
    fn incomplete_index_detection() {
        assert!(index_incomplete(0, 1383), "empty index with documents in the DB");
        assert!(index_incomplete(500, 1383), "less than half");
        assert!(!index_incomplete(1380, 1383), "small difference: incremental updates");
        assert!(!index_incomplete(0, 0), "empty archive");
        assert!(!index_incomplete(10, 0));
    }

    #[test]
    fn indexed_count_of_built_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        assert_eq!(indexed_document_count(&dir, Some("en")), None, "no index yet");
        ix.rebuild_from_records(&[record(1, "one"), record(2, "two"), record(3, "three")], 128_000_000).unwrap();
        assert_eq!(indexed_document_count(&dir, Some("en")), Some(3));
        assert_eq!(ix.document_count(), Some(3));
    }

    fn count(dir: &Path) -> usize {
        let index = Index::open_in_dir(dir).unwrap();
        pl_search::tokenizer::register_tokenizers(&index, Some("en"));
        index.reader().unwrap().searcher().search(&AllQuery, &Count).unwrap()
    }

    /// Review: rebuild wiped the old index before the new one was complete; a failure in the
    /// middle left no working index. Now the old index stays searchable and current.
    #[test]
    fn failed_rebuild_keeps_the_old_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        ix.rebuild_from_records(&[record(1, "one"), record(2, "two")], 128_000_000).unwrap();
        // the second record is rejected (negative id) after the first one was added
        assert!(ix.rebuild_from_records(&[record(3, "three"), record(-1, "bad")], 128_000_000).is_err());
        assert_eq!(count(&dir), 2, "old index still complete");
        assert!(!crate::settings::needs_rebuild(&dir, Some("en")), "old index still counts as current");
        assert!(!dir.join(REBUILD_STAGING).exists(), "staging directory cleaned up");
    }

    /// The swap replaces the content completely, leaves no old segment files behind and keeps a
    /// reader opened before the rebuild working (it reloads to the new index).
    #[test]
    fn rebuild_swaps_in_the_new_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        ix.rebuild_from_records(&[record(1, "one"), record(2, "two"), record(3, "three")], 128_000_000).unwrap();
        let reader_index = Index::open_in_dir(&dir).unwrap();
        pl_search::tokenizer::register_tokenizers(&reader_index, Some("en"));
        let reader = reader_index.reader_builder().reload_policy(tantivy::ReloadPolicy::Manual).try_into().unwrap();
        ix.rebuild_from_records(&[record(7, "seven")], 128_000_000).unwrap();
        assert_eq!(count(&dir), 1);
        reader.reload().unwrap();
        assert_eq!(reader.searcher().num_docs(), 1);
        drop(reader);
        drop(reader_index);
        // Windows: files the reader still mapped during the swap may stay until the next rebuild
        if !cfg!(windows) {
            let meta = std::fs::read_to_string(dir.join("meta.json")).unwrap().replace('-', "");
            for e in std::fs::read_dir(&dir).unwrap() {
                let e = e.unwrap();
                let name = e.file_name().to_string_lossy().into_owned();
                if e.file_type().unwrap().is_file() && !is_kept_name(&name) {
                    let seg = name.split('.').next().unwrap_or_default();
                    assert!(meta.contains(seg), "stale file {name} left behind");
                }
            }
        }
        assert!(!dir.join(REBUILD_STAGING).exists());
        assert!(!dir.join(REBUILD_MARKER).exists());
        assert!(!crate::settings::needs_rebuild(&dir, Some("en")));
    }

    /// Review 3: after a database switch to another archive of similar size the count check
    /// alone passed and the old index stayed active. The (id, checksum) sample tells them apart.
    #[test]
    fn index_of_another_archive_with_similar_count_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        let rec = |id: i64, sum: &str| {
            let mut r = record(id, &format!("doc {id}"));
            r.checksum = sum.into();
            r
        };
        let archive_a: Vec<DocumentRecord> = (1..=100).map(|i| rec(i, &format!("a{i:030}"))).collect();
        ix.rebuild_from_records(&archive_a, 128_000_000).unwrap();
        let sample = indexed_sample(&dir, Some("en"), IDENTITY_SAMPLE).unwrap();
        assert!(sample.len() >= IDENTITY_SAMPLE / 2 && sample.len() <= IDENTITY_SAMPLE, "{}", sample.len());
        assert!(sample.windows(2).all(|w| w[0].0 != w[1].0), "spread, no duplicates");

        // same archive: everything matches
        let db_a: Vec<(i64, String)> = (1..=100).map(|i| (i, format!("a{i:030}"))).collect();
        assert!(!index_from_other_archive(sample.len(), count_matches(&sample, db_a)));
        // other archive of nearly the same size, overlapping ids: the count check passes ...
        assert!(!index_incomplete(100, 98));
        // ... but the sample does not match
        let db_b: Vec<(i64, String)> = (1..=98).map(|i| (i, format!("b{i:030}"))).collect();
        assert!(index_from_other_archive(sample.len(), count_matches(&sample, db_b)));
        // a few changed/deleted documents in the same archive are not "another archive"
        let db_a_mostly: Vec<(i64, String)> = (1..=100).filter(|i| i % 10 != 0).map(|i| (i, format!("a{i:030}"))).collect();
        assert!(!index_from_other_archive(sample.len(), count_matches(&sample, db_a_mostly)));
        assert!(!index_from_other_archive(0, 0), "empty index handled by the count check");
    }

    /// Review: `--if-needed` treated a failing document count as 0 documents and reported
    /// "Search index is up to date". The error must reach the caller.
    #[tokio::test]
    async fn if_needed_propagates_db_count_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        ix.rebuild_from_records(&[record(1, "one")], 128_000_000).unwrap();
        // nothing listens on port 1: every query fails
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(2))
            .connect_lazy("postgres://u:p@127.0.0.1:1/x")
            .unwrap();
        let opts = RebuildOptions { if_needed: true, ..Default::default() };
        let r = ix.rebuild(&pool, &opts).await;
        assert!(matches!(r, Err(IndexerError::Db(_))), "got {r:?}");
        assert_eq!(count(&dir), 1, "index untouched");
    }

    #[tokio::test]
    async fn rebuild_then_batch_upsert_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        ix.rebuild_from_records(&[record(1, "one"), record(2, "two")], 128_000_000).unwrap();
        assert!(!dir.join(pl_search::schema::REBUILD_MARKER).exists());
        assert!(!crate::settings::needs_rebuild(&dir, Some("en")));
        assert_eq!(count(&dir), 2);

        let mut b = ix.batch(Duration::from_secs(1)).await.unwrap();
        b.add_or_update_record(&record(2, "two v2")).unwrap();
        b.add_or_update_record(&record(3, "three")).unwrap();
        b.remove(1).unwrap();
        b.commit().unwrap();
        assert_eq!(count(&dir), 2);

        // pl-search reads what we wrote
        let backend = pl_search::SearchBackend::new(pl_search::SearchSettings {
            index_dir: dir.clone(),
            language: Some("en".into()),
            fuzzy_threshold: None,
            time_zone: chrono_tz::UTC,
        });
        let ids = backend.search_ids("v2", None, None, false, pl_search::SearchMode::Text, None, None).unwrap();
        assert_eq!(ids, vec![2]);
    }

    /// Measurement (results/bench/worker/NOTES.md, T3): cost per document when every document is, like
    /// upstream (`add_to_index` in the consumer), locked, written and committed individually,
    /// compared with a single batch. `cargo test --release -p pl-indexer -- --ignored --nocapture commit_cost`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "measurement"]
    async fn commit_cost_per_document_vs_batch() {
        let n: i64 = std::env::var("PL_INDEXER_BENCH_DOCS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
        let base: i64 = std::env::var("PL_INDEXER_BENCH_BASE").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
        let body = "Lorem ipsum dolor sit amet invoice total amount due payment reference ".repeat(40);
        let rec = |id: i64| {
            let mut r = record(id, &format!("doc {id}"));
            r.effective_content = format!("{body} unique{id}");
            r
        };
        for (label, per_doc) in [("per-document commit", true), ("one batch", false)] {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("index");
            let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
            let seed: Vec<DocumentRecord> = (1..=base).map(rec).collect();
            ix.rebuild_from_records(&seed, REBUILD_HEAP_BYTES).unwrap();
            let t = std::time::Instant::now();
            if per_doc {
                for id in base + 1..=base + n {
                    let mut b = ix.batch(LOCK_TIMEOUT).await.unwrap();
                    b.add_or_update_record(&rec(id)).unwrap();
                    b.commit().unwrap();
                }
            } else {
                let mut b = ix.batch(LOCK_TIMEOUT).await.unwrap();
                for id in base + 1..=base + n {
                    b.add_or_update_record(&rec(id)).unwrap();
                }
                b.commit().unwrap();
            }
            let el = t.elapsed();
            assert_eq!(count(&dir), (base + n) as usize);
            println!(
                "{label}: {n} docs on top of {base}: {:.1} ms total, {:.1} ms/doc",
                el.as_secs_f64() * 1e3,
                el.as_secs_f64() * 1e3 / n as f64
            );
        }
    }

    /// BEFUNDE B8: many small batches (upsert = delete + add, i.e. `.del` files) from
    /// two tasks at the same time while a reader (pl-search, `OnCommitWithDelay`, keeps segment
    /// and `.del` files mapped) searches constantly. On Windows this made individual commits fail with
    /// `os error 5` or `FileAlreadyExists(….del)`. Every write via the public paths
    /// must succeed (after a retry if necessary), and in the end every document must be in the index exactly once
    /// with its latest state.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_batches_with_live_reader() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        let n_docs: i64 = 30;
        ix.rebuild_from_records(&(1..=n_docs).map(|i| record(i, &format!("doc{i} v0"))).collect::<Vec<_>>(), 128_000_000).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (dir, stop) = (dir.clone(), stop.clone());
            std::thread::spawn(move || {
                let backend = pl_search::SearchBackend::new(pl_search::SearchSettings {
                    index_dir: dir,
                    language: Some("en".into()),
                    fuzzy_threshold: None,
                    time_zone: chrono_tz::UTC,
                });
                let mut searches = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = backend.search_ids("doc1", None, None, false, pl_search::SearchMode::Text, None, None);
                    searches += 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
                searches
            })
        };
        let rounds: i64 = std::env::var("PL_INDEXER_B8_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        let mut tasks = Vec::new();
        for w in 0..2i64 {
            let ix = ix.clone();
            tasks.push(tokio::spawn(async move {
                let mut errors = Vec::new();
                for r in 0..rounds {
                    let id = 1 + (r * 7 + w * 13) % n_docs;
                    if r % 5 == 4 {
                        // aborted task (lease lost, timeout): batch discarded without commit
                        if let Ok(mut b) = ix.batch(LOCK_TIMEOUT).await {
                            let _ = b.add_or_update_record(&record(id, "dropped"));
                            drop(b);
                        }
                        continue;
                    }
                    let rec = record(id, &format!("doc{id} w{w} r{r}"));
                    if let Err(e) = ix.write_records(&[rec], &[]).await {
                        errors.push(e.to_string());
                    }
                }
                errors
            }));
        }
        let mut errors = Vec::new();
        for t in tasks {
            errors.extend(t.await.unwrap());
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let searches = reader.join().unwrap();
        assert!(errors.is_empty(), "{} failed writes (of {}), e.g. {:?}; {searches} searches", errors.len(), 2 * rounds, errors.first());
        assert_eq!(count(&dir), n_docs as usize, "every document exactly once");
    }

    /// BEFUNDE B8 reproduced (Windows): a foreign handle on `meta.json` without `FILE_SHARE_DELETE`
    /// (like a virus scanner/Windows search right after writing) makes the commit fail when replacing
    /// `meta.json` with "Access denied (os error 5)" – after segment and `.del` files
    /// have already been written. A plain second attempt with a fresh writer produces the same
    /// opstamps and then fails with `FileAlreadyExists(….del)`. Expected: the write succeeds
    /// after release (retry with cleanup and shifted opstamp sequence), index complete.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commit_blocked_by_foreign_handle_is_retried() {
        use std::os::windows::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        ix.rebuild_from_records(&[record(1, "one"), record(2, "two"), record(3, "three")], 128_000_000).unwrap();
        let meta = dir.join("meta.json");
        let (tx, rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            // FILE_SHARE_READ (1): reading allowed, replacing/deleting not
            let f = std::fs::OpenOptions::new().read(true).share_mode(1).open(&meta).unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(600));
            drop(f);
        });
        rx.recv().unwrap();
        let r = ix.write_records(&[record(2, "two zebra")], &[3]).await;
        holder.join().unwrap();
        r.expect("write retried after the foreign handle was closed");
        assert_eq!(count(&dir), 2);
        let backend = pl_search::SearchBackend::new(pl_search::SearchSettings {
            index_dir: dir.clone(),
            language: Some("en".into()),
            fuzzy_threshold: None,
            time_zone: chrono_tz::UTC,
        });
        let ids = backend.search_ids("zebra", None, None, false, pl_search::SearchMode::Text, None, None).unwrap();
        assert_eq!(ids, vec![2]);
        // continue normally afterwards
        ix.write_records(&[record(4, "four")], &[]).await.unwrap();
        assert_eq!(count(&dir), 3);
    }

    #[tokio::test]
    async fn dropped_batch_discards_and_releases_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("index");
        let ix = Indexer::new(IndexerConfig { index_dir: dir.clone(), language: Some("en".into()) });
        ix.rebuild_from_records(&[record(1, "one")], 128_000_000).unwrap();
        {
            let mut b = ix.batch(Duration::from_secs(1)).await.unwrap();
            b.remove(1).unwrap();
            // a second batch cannot get the lock meanwhile
            assert!(IndexFileLock::try_acquire(&dir).unwrap().is_none());
        }
        assert_eq!(count(&dir), 1);
        let b = ix.batch(Duration::from_secs(1)).await.unwrap();
        b.commit().unwrap();
    }
}
