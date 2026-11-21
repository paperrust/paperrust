//! L3: catching up the search index for permissions/trash (ARCHITECTURE-V2 §5, docs/contract/pl-search.md
//! "L3").
//!
//! Triggers of the migration `pl_indexer.0001_paperrust_search_dirty` note in
//! `paperrust_search_dirty` every document whose visibility or liveness changes
//! (owner, trash, attach/detach version, deletion, guardian grant change/revocation).
//! As long as rows are present there, pl-search searches exactly (intersection with SQL). [`drain_once`] re-indexes
//! the noted documents (or removes them) and afterwards deletes exactly the rows that were seen
//! (same `seq`; a repeated marking during the run stays in place).

use sqlx::PgPool;

use crate::error::{IndexerError, Result};
use crate::writer::Indexer;

/// Maximum number of documents per run.
pub const DRAIN_BATCH: i64 = 1000;

/// Migration applied?
pub async fn installed(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT to_regclass('public.paperrust_search_dirty') IS NOT NULL").fetch_one(pool).await?)
}

/// Open entries (count).
pub async fn pending_count(pool: &PgPool) -> Result<i64> {
    if !installed(pool).await? {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM paperrust_search_dirty").fetch_one(pool).await?)
}

/// Largest assigned `seq` (for [`clear_upto`] after a full rebuild); `None` without
/// migration.
pub async fn max_seq(pool: &PgPool) -> Result<Option<i64>> {
    if !installed(pool).await? {
        return Ok(None);
    }
    Ok(Some(sqlx::query_scalar::<_, i64>("SELECT COALESCE(max(seq), 0) FROM paperrust_search_dirty").fetch_one(pool).await?))
}

/// After a rebuild from the DB: everything up to `seq` is done.
pub async fn clear_upto(pool: &PgPool, seq: i64) -> Result<u64> {
    Ok(sqlx::query("DELETE FROM paperrust_search_dirty WHERE seq <= $1").bind(seq).execute(pool).await?.rows_affected())
}

/// Result of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Drained {
    /// noted documents in this run
    pub seen: usize,
    /// re-indexed root documents
    pub indexed: usize,
    /// lock held (Python/Rust writer) → retry later
    pub deferred: bool,
}

/// One run: remove up to [`DRAIN_BATCH`] noted documents + (if live root document
/// or version → root) rewrite them, commit, call `after_commit` (reload readers), and
/// only then delete the rows that were seen.
pub async fn drain_once(ix: &Indexer, pool: &PgPool, after_commit: impl FnOnce()) -> Result<Drained> {
    let rows: Vec<(i64, i64)> =
        sqlx::query_as("SELECT doc_id, seq FROM paperrust_search_dirty ORDER BY seq LIMIT $1").bind(DRAIN_BATCH).fetch_all(pool).await?;
    if rows.is_empty() {
        return Ok(Drained::default());
    }
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let seqs: Vec<i64> = rows.iter().map(|r| r.1).collect();
    let live: Vec<i64> = ids.iter().copied().filter(|id| *id > 0).collect();
    // BEFUNDE B8: whole write operation with retry on transient file errors
    let (ids_ref, live_ref) = (&ids, &live);
    let written = ix
        .with_retry("dirty drain", |attempt| async move {
            let mut batch = ix.batch_attempt(crate::lock::BATCH_LOCK_TIMEOUT, attempt).await?;
            for id in ids_ref {
                if *id >= 0 {
                    batch.remove(*id)?;
                }
            }
            let indexed = batch.add_or_update_ids(pool, live_ref).await?;
            batch.commit()?;
            Ok(indexed)
        })
        .await;
    let indexed = match written {
        Ok(n) => n,
        Err(IndexerError::LockExhausted { .. }) => {
            return Ok(Drained { seen: ids.len(), indexed: 0, deferred: true });
        }
        Err(e) => return Err(e),
    };
    after_commit();
    sqlx::query(
        "DELETE FROM paperrust_search_dirty d USING unnest($1::int8[], $2::int8[]) AS t(doc_id, seq) \
          WHERE d.doc_id = t.doc_id AND d.seq = t.seq",
    )
    .bind(&ids)
    .bind(&seqs)
    .execute(pool)
    .await?;
    Ok(Drained { seen: ids.len(), indexed, deferred: false })
}
