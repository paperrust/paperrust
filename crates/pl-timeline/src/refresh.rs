//! Save content detection (`paperrust_doc_date`, `source='content'`) and maintain its state
//! (`paperrust_doc_date_state`): after ingestion, for outdated documents on read,
//! as backfill (command/task). All only with [`crate::SETTING_EXTRACT`].
//!
//! The full recompute ([`backfill`]) is guarded by a PostgreSQL advisory lock ([`BACKFILL_LOCK_KEY`]),
//! so it runs at most once at a time across all processes on one database.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{Connection, PgConnection, PgPool};

use crate::Result;
use crate::extract::{DateParse, EXTRACTOR_VERSION, extract};
use crate::store::ensure;

/// Is detection switched on? (`get` e.g. `pl_core::config::var`)
pub fn enabled(get: &dyn Fn(&str) -> Option<String>) -> bool {
    crate::truthy(get(crate::SETTING_EXTRACT).as_deref())
}

/// Re-detect one document (or its root document). Returns the number of dates found;
/// `None` if the document does not (any longer) exist.
pub async fn refresh_document(pool: &PgPool, dp: &DateParse, doc_id: i64) -> Result<Option<usize>> {
    ensure(pool).await?;
    let row: Option<(i32, String, NaiveDate, DateTime<Utc>)> = sqlx::query_as(
        "SELECT r.id, r.content, r.created, r.modified FROM documents_document d
           JOIN documents_document r ON r.id = COALESCE(d.root_document_id, d.id)
          WHERE d.id = $1 AND r.deleted_at IS NULL",
    )
    .bind(doc_id as i32)
    .fetch_optional(pool)
    .await?;
    let Some((id, content, created, modified)) = row else {
        return Ok(None);
    };
    let found = extract(&content, Some(created), dp);
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM paperrust_doc_date WHERE document_id = $1 AND source = 'content'").bind(id).execute(&mut *tx).await?;
    for f in &found {
        sqlx::query(
            "INSERT INTO paperrust_doc_date (document_id, kind, date, source, confidence, phrase, page)
             VALUES ($1, $2, $3, 'content', $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(f.kind.as_str())
        .bind(f.date)
        .bind(f.confidence)
        .bind(&f.phrase)
        .bind(f.page)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO paperrust_doc_date_state (document_id, modified, version, found, extracted) VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (document_id) DO UPDATE SET modified = EXCLUDED.modified, version = EXCLUDED.version, found = EXCLUDED.found, extracted = now()",
    )
    .bind(id)
    .bind(modified)
    .bind(EXTRACTOR_VERSION)
    .bind(found.len() as i32)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(found.len()))
}

/// Outdated documents (without detection, modified since the detection or older version),
/// optionally only within `within` (SQL subquery with column `id`).
pub async fn stale_ids(pool: &PgPool, within: Option<&str>, limit: i64) -> Result<Vec<i64>> {
    ensure(pool).await?;
    let filter = within.map(|w| format!("AND d.id IN (SELECT v.id FROM ({w}) AS v(id))")).unwrap_or_default();
    Ok(sqlx::query_scalar(&format!(
        "SELECT d.id::int8 FROM documents_document d LEFT JOIN paperrust_doc_date_state s ON s.document_id = d.id
          WHERE d.deleted_at IS NULL AND d.root_document_id IS NULL
            AND (s.document_id IS NULL OR s.modified < d.modified OR s.version < $1) {filter}
          ORDER BY d.id LIMIT $2"
    ))
    .bind(EXTRACTOR_VERSION)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Re-detect up to `limit` outdated documents; number of detected documents.
pub async fn refresh_stale(pool: &PgPool, dp: &DateParse, within: Option<&str>, limit: i64) -> Result<usize> {
    let ids = stale_ids(pool, within, limit).await?;
    let mut n = 0;
    for id in ids {
        if refresh_document(pool, dp, id).await?.is_some() {
            n += 1;
        }
    }
    Ok(n)
}

/// Result of the backfill.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BackfillReport {
    pub documents: usize,
    pub dates: usize,
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Key of the PostgreSQL advisory lock that guards the full recompute (two-int4 form, so `pg_locks`
/// shows it as `classid`/`objid` with `objsubid = 2`). Fixed values ("PRST", "TLBF"), the same in every
/// server, worker and `manage` process of one database.
pub const BACKFILL_LOCK_KEY: (i32, i32) = (0x5052_5354, 0x544c_4246);

/// Is a backfill currently running in **this** process? Cheap, no database access; for the
/// cluster-wide answer use [`backfill_running_any`].
pub fn backfill_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// Is a backfill running in any process connected to this database (advisory lock held)?
pub async fn backfill_running_any(pool: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND granted
            AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
            AND classid = $1::int4::oid AND objid = $2::int4::oid AND objsubid = 2)",
    )
    .bind(BACKFILL_LOCK_KEY.0)
    .bind(BACKFILL_LOCK_KEY.1)
    .fetch_one(pool)
    .await?)
}

/// Held backfill lock: a session-level advisory lock on a connection taken out of the pool
/// (`detach`), so it can never return to the pool with the lock still held. Dropping it closes that
/// connection, which releases the lock on the server even after an error or a panic.
pub struct BackfillLock {
    conn: Option<PgConnection>,
}

impl BackfillLock {
    /// Release the lock and close its connection gracefully.
    pub async fn release(mut self) {
        if let Some(mut c) = self.conn.take() {
            let unlocked =
                sqlx::query("SELECT pg_advisory_unlock($1, $2)").bind(BACKFILL_LOCK_KEY.0).bind(BACKFILL_LOCK_KEY.1).execute(&mut c).await;
            if let Err(e) = unlocked {
                tracing::warn!(target: "paperrust.timeline", error = %e, "releasing the backfill lock failed; closing its connection");
            }
            // closing the session releases any session lock as well
            let _ = c.close().await;
        }
    }
}

impl Drop for BackfillLock {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Relaxed);
    }
}

/// Try to take the backfill lock (`pg_try_advisory_lock`, never waits). `None` if a backfill is
/// already running in any process on this database.
pub async fn try_lock_backfill(pool: &PgPool) -> Result<Option<BackfillLock>> {
    let mut conn = pool.acquire().await?.detach();
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
        .bind(BACKFILL_LOCK_KEY.0)
        .bind(BACKFILL_LOCK_KEY.1)
        .fetch_one(&mut conn)
        .await?;
    if !got {
        let _ = conn.close().await;
        return Ok(None);
    }
    RUNNING.store(true, Ordering::Relaxed);
    Ok(Some(BackfillLock { conn: Some(conn) }))
}

/// Backfill: all outdated (with `force`: all) root documents. `None` if a backfill is already running
/// (in any process on this database, see [`try_lock_backfill`]).
pub async fn backfill(pool: &PgPool, dp: &DateParse, force: bool) -> Result<Option<BackfillReport>> {
    let Some(lock) = try_lock_backfill(pool).await? else {
        return Ok(None);
    };
    backfill_locked(pool, dp, force, lock).await.map(Some)
}

/// Backfill under a lock taken earlier with [`try_lock_backfill`] (lets an HTTP handler answer
/// "started"/"already running" truthfully before it spawns the work). Releases the lock at the end.
pub async fn backfill_locked(pool: &PgPool, dp: &DateParse, force: bool, lock: BackfillLock) -> Result<BackfillReport> {
    let res = backfill_run(pool, dp, force).await;
    lock.release().await;
    res
}

async fn backfill_run(pool: &PgPool, dp: &DateParse, force: bool) -> Result<BackfillReport> {
    ensure(pool).await?;
    if force {
        sqlx::query("DELETE FROM paperrust_doc_date_state").execute(pool).await?;
    }
    let mut rep = BackfillReport::default();
    loop {
        let ids = stale_ids(pool, None, 200).await?;
        if ids.is_empty() {
            break;
        }
        let mut progressed = false;
        for id in ids {
            if let Some(n) = refresh_document(pool, dp, id).await? {
                rep.documents += 1;
                rep.dates += n;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    tracing::info!(target: "paperrust.timeline", documents = rep.documents, dates = rep.dates, "timeline date backfill finished");
    Ok(rep)
}

/// After ingestion/reprocessing (worker): with detection switched on, re-detect the
/// document. Errors are logged (the ingestion itself was successful).
pub async fn after_consumption(pool: &PgPool, doc_id: i64, get: fn(&str) -> Option<String>) {
    if !enabled(&get) {
        return;
    }
    let dp = DateParse::from_lookup(get);
    match refresh_document(pool, &dp, doc_id).await {
        Ok(Some(n)) => {
            tracing::debug!(target: "paperrust.timeline", document_id = doc_id, dates = n, "timeline dates extracted")
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(target: "paperrust.timeline", document_id = doc_id, error = %e, "timeline date extraction failed")
        }
    }
}
