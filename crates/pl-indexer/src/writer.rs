// upstream: src/documents/search/_backend.py@v3.4.2 (WriteBatch, TantivyBackend.open,
//           add_or_update, remove, batch_update, rebuild), _schema.py (open_or_rebuild_index,
//           rebuild_in_progress, wipe_index, _write_sentinels), documents/tasks.py
//           (index_optimize, index_document, remove_document_from_index),
//           documents/management/commands/document_index.py (reindex / optimize)
//           tantivy-py 0.26.2 src/index.rs (Index(schema, path, reuse=True) = open_or_create,
//           Index.writer(heap_size=128_000_000, num_threads=0) = tantivy Index::writer(heap)).

use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::PgPool;
use tantivy::query::TermQuery;
use tantivy::schema::{IndexRecordOption, Schema};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

use crate::db::{self, LoadMode};
use crate::doc::build_tantivy_doc;
use crate::error::{IndexerError, Result};
use crate::lock::{BATCH_LOCK_TIMEOUT, IndexFileLock, LOCK_TIMEOUT};
use crate::record::DocumentRecord;
use crate::settings::{
    OpenOutcome, clear_rebuild_marker, open_or_create, open_or_rebuild_index, touch_rebuild_marker,
    wipe_index, write_sentinels,
};

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
        Self {
            index_dir: s.index_dir,
            language: s.language,
        }
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
        Self {
            writer_heap_bytes: REBUILD_HEAP_BYTES,
            recreate: false,
            if_needed: false,
            take_lock: true,
        }
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
        Ok(WriteBatch {
            schema: index.schema(),
            writer: Some(writer),
            _index: index,
            lock: Some(lock),
        })
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

    /// `index_optimize` / `document_index optimize`: a no-op upstream.
    pub fn optimize(&self) {
        tracing::info!("index_optimize is a no-op — Tantivy manages segment merging automatically.");
    }

    /// `document_index reindex` → `TantivyBackend.rebuild(documents)`.
    pub async fn rebuild(&self, pool: &PgPool, opts: &RebuildOptions) -> Result<RebuildStats> {
        let dir = self.cfg.index_dir.clone();
        if opts.if_needed && dir.exists() && !crate::settings::needs_rebuild(&dir, self.cfg.lang()) {
            tracing::info!("Search index is up to date.");
            return Ok(RebuildStats { documents: 0, skipped_up_to_date: true });
        }
        // upstream assumes INDEX_DIR exists (paperless creates it at startup).
        std::fs::create_dir_all(&dir)?;
        let _lock = if opts.take_lock {
            Some(IndexFileLock::acquire_with_retries(&dir, BATCH_LOCK_TIMEOUT).await?)
        } else {
            None
        };
        if opts.recreate {
            wipe_index(&dir)?;
        }
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
        let (index, writer) = self.start_rebuild(&dir, heap)?;
        let schema = index.schema();
        for rec in records {
            writer.add_document(build_tantivy_doc(&schema, rec)?)?;
        }
        finish_rebuild(&dir, writer)?;
        Ok(records.len())
    }

    fn start_rebuild(&self, dir: &Path, heap: usize) -> Result<(Index, IndexWriter<TantivyDocument>)> {
        wipe_index(dir)?;
        // `with rebuild_in_progress(path)`: marker first, then the empty stamped index.
        touch_rebuild_marker(dir)?;
        let index = open_or_create(dir, self.cfg.lang())?;
        write_sentinels(dir, self.cfg.lang())?;
        let writer: IndexWriter<TantivyDocument> = index.writer(heap)?;
        Ok((index, writer))
    }

    async fn rebuild_from_db(
        &self,
        dir: &Path,
        heap: usize,
        pool: &PgPool,
        ids: &[i64],
    ) -> Result<usize> {
        let (index, writer) = self.start_rebuild(dir, heap)?;
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
        // On error above the writer is dropped uncommitted and the marker stays.
        finish_rebuild(dir, writer)?;
        Ok(count)
    }
}

fn finish_rebuild(dir: &Path, mut writer: IndexWriter<TantivyDocument>) -> Result<()> {
    writer.commit()?;
    writer.wait_merging_threads()?;
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
        self.writer
            .as_ref()
            .ok_or_else(|| IndexerError::InvalidValue("WriteBatch already finished".into()))
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
        let id = u64::try_from(doc_id)
            .map_err(|_| IndexerError::InvalidValue("negative document id".into()))?;
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
            let mut writer = self
                .writer
                .take()
                .ok_or_else(|| IndexerError::InvalidValue("WriteBatch already finished".into()))?;
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

    fn count(dir: &Path) -> usize {
        let index = Index::open_in_dir(dir).unwrap();
        pl_search::tokenizer::register_tokenizers(&index, Some("en"));
        index.reader().unwrap().searcher().search(&AllQuery, &Count).unwrap()
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
        let ids = backend
            .search_ids("v2", None, None, false, pl_search::SearchMode::Text, None, None)
            .unwrap();
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
        ix.rebuild_from_records(&(1..=n_docs).map(|i| record(i, &format!("doc{i} v0"))).collect::<Vec<_>>(), 128_000_000)
            .unwrap();
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
