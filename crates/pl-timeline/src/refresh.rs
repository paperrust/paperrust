//! Save content detection (`paperrust_doc_date`, `source='content'`) and maintain its state
//! (`paperrust_doc_date_state`): after ingestion, for outdated documents on read,
//! as backfill (command/task). All only with [`crate::SETTING_EXTRACT`].

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::Result;
use crate::extract::{DateParse, EXTRACTOR_VERSION, extract};
use crate::store::ensure;

/// Erkennung eingeschaltet? (`get` z. B. `pl_core::config::var`)
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
    sqlx::query("DELETE FROM paperrust_doc_date WHERE document_id = $1 AND source = 'content'")
        .bind(id)
        .execute(&mut *tx)
        .await?;
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
    let filter = within
        .map(|w| format!("AND d.id IN (SELECT v.id FROM ({w}) AS v(id))"))
        .unwrap_or_default();
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
pub async fn refresh_stale(
    pool: &PgPool,
    dp: &DateParse,
    within: Option<&str>,
    limit: i64,
) -> Result<usize> {
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

/// Is a backfill currently running in this process?
pub fn backfill_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

struct Running;
impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Relaxed);
    }
}

/// Backfill: all outdated (with `force`: all) root documents. `None` if a
/// backfill is already running in this process.
pub async fn backfill(
    pool: &PgPool,
    dp: &DateParse,
    force: bool,
) -> Result<Option<BackfillReport>> {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    let _guard = Running;
    ensure(pool).await?;
    if force {
        sqlx::query("DELETE FROM paperrust_doc_date_state")
            .execute(pool)
            .await?;
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
    Ok(Some(rep))
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
