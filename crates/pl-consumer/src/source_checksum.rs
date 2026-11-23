// PaperRust-specific (no upstream counterpart): checksum of the delivered file for documents whose
// stored file differs from it because PaperRust shrank it on ingest (pdftools.rs). Upstream compares
// the checksum of the incoming file with `checksum`/`archive_checksum` (consumer.py
// `ConsumerPreflightPlugin.pre_check_duplicate`); with shrinking the stored `checksum` is that of the
// smaller file, so the same original would no longer be found. This side table keeps the original's
// checksum; `db::find_duplicates` matches it as well. Contract: docs/contract/pl-consumer.md.

use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::{PgConnection, PgPool};

/// Migration (name, SQL) for `paperrust manage migrate` (listed in `workflow_ext::migrations::ALL`,
/// which pl-cli includes).
pub const MIGRATION: (&str, &str) =
    ("pl_consumer.0002_paperrust_source_checksum", include_str!("../migrations/0002_paperrust_source_checksum.sql"));

static READY: AtomicBool = AtomicBool::new(false);

const EXISTS_SQL: &str = "SELECT to_regclass('public.paperrust_source_checksum') IS NOT NULL";

fn create_script() -> String {
    // One command (DO block): the lock serializes concurrent creators until the end of the
    // (implicit or outer) transaction.
    format!(
        "DO $paperrust$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('paperrust_source_checksum'));
{}
END $paperrust$",
        MIGRATION.1
    )
}

/// Only for tests with several throwaway databases in one process: forget "ready".
#[doc(hidden)]
pub fn forget_ready_for_tests() {
    READY.store(false, Ordering::Relaxed);
}

/// Has [`ensure_pool`] already confirmed the table in this process?
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Create the table via a pool connection (no outer transaction) and remember it as ready.
pub async fn ensure_pool(pool: &PgPool) -> Result<(), sqlx::Error> {
    if is_ready() {
        return Ok(());
    }
    let mut c = pool.acquire().await?;
    let exists: bool = sqlx::query_scalar(EXISTS_SQL).fetch_one(&mut *c).await?;
    if !exists {
        sqlx::query(&create_script()).execute(&mut *c).await?;
    }
    READY.store(true, Ordering::Relaxed);
    Ok(())
}

/// Ensure the table on the caller's connection (also inside a transaction); does not remember
/// "ready" (the outer transaction could roll back).
pub async fn ensure_conn(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    if is_ready() {
        return Ok(());
    }
    let exists: bool = sqlx::query_scalar(EXISTS_SQL).fetch_one(&mut *conn).await?;
    if !exists {
        sqlx::query(&create_script()).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Whether the source checksum must be recorded: only when PaperRust replaced the stored file by a
/// shrunk copy and the checksums really differ. Other differences (pre-consume script changing the
/// working copy) are left as upstream has them (Known difference in the contract).
pub fn needs_record(shrunk: bool, original: Option<&str>, stored: &str) -> Option<String> {
    match original {
        Some(o) if shrunk && o != stored => Some(o.to_string()),
        _ => None,
    }
}

/// Record the delivered file's checksum for `document_id` (inside the store transaction, so it is
/// visible together with the document row at COMMIT).
pub async fn record(conn: &mut PgConnection, document_id: i64, checksum: &str) -> Result<(), sqlx::Error> {
    ensure_conn(conn).await?;
    let id = i32::try_from(document_id).map_err(|e| sqlx::Error::Protocol(format!("document id {document_id}: {e}")))?;
    sqlx::query(
        "INSERT INTO paperrust_source_checksum (document_id, checksum) VALUES ($1, $2) \
         ON CONFLICT (document_id) DO UPDATE SET checksum = EXCLUDED.checksum",
    )
    .bind(id)
    .bind(checksum)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_only_for_shrunk_files_with_other_checksum() {
        assert_eq!(needs_record(true, Some("a"), "b"), Some("a".to_string()));
        assert_eq!(needs_record(true, Some("a"), "a"), None, "same bytes: nothing to add");
        assert_eq!(needs_record(false, Some("a"), "b"), None, "pre-consume script change stays as upstream");
        assert_eq!(needs_record(true, None, "b"), None);
    }

    #[test]
    fn migration_is_idempotent_sql() {
        let sql = MIGRATION.1;
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS paperrust_source_checksum"));
        assert!(sql.contains("CREATE INDEX IF NOT EXISTS"));
        assert!(!sql.contains("REFERENCES"), "no FK: documents_document is a view from data model stage 1");
        assert!(create_script().starts_with("DO $paperrust$"));
    }
}
