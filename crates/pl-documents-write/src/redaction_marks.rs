//! Watch list of redacted versions (`paperrust_redaction`, PaperRust-specific, no upstream counterpart;
//! contract docs/contract/pl-pdftools.md §4.1).
//!
//! After a successful tool run the redact route writes the checksum of the redacted file
//! here, before it enqueues the new version. `pl-documents` reads the table when outputting the
//! versions (`versions[].redacted`), so that the sharing dialog can show which version is redacted.
//! If an entry is missing (e.g. error while writing), the version counts as **not** redacted – the
//! safe side for a warning notice.

use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::PgPool;

/// Migration (name, SQL) for `paperrust manage migrate`.
pub const MIGRATION: (&str, &str) =
    ("pl_documents_write.0001_paperrust_redaction", include_str!("../migrations/0001_paperrust_redaction.sql"));

pub mod migrations {
    pub const ALL: &[(&str, &str)] = &[super::MIGRATION];
}

static READY: AtomicBool = AtomicBool::new(false);

/// Create the table if needed (once per process; parallel processes via an advisory lock).
pub async fn ensure(pool: &PgPool) -> Result<(), sqlx::Error> {
    if READY.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut c = pool.acquire().await?;
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public.paperrust_redaction') IS NOT NULL").fetch_one(&mut *c).await?;
    if !exists {
        let script = format!(
            "DO $paperrust$ BEGIN PERFORM pg_advisory_xact_lock(hashtext('paperrust_redaction'));\n{}\nEND $paperrust$",
            MIGRATION.1
        );
        sqlx::query(&script).execute(&mut *c).await?;
    }
    READY.store(true, Ordering::Relaxed);
    Ok(())
}

/// Remember a successful redaction.
pub async fn record(pool: &PgPool, root_id: i64, source_id: i64, checksum: &str, actor_id: i64) -> Result<(), sqlx::Error> {
    ensure(pool).await?;
    sqlx::query(
        "INSERT INTO paperrust_redaction (root_document_id, source_document_id, checksum, actor_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(i32::try_from(root_id).unwrap_or(i32::MAX))
    .bind(i32::try_from(source_id).unwrap_or(i32::MAX))
    .bind(checksum)
    .bind(i32::try_from(actor_id).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;
    Ok(())
}

/// Is `checksum` a redacted version? Error or missing table: `false`.
pub async fn is_redacted(pool: &PgPool, checksum: &str) -> bool {
    let exists: Result<bool, _> =
        sqlx::query_scalar("SELECT to_regclass('public.paperrust_redaction') IS NOT NULL").fetch_one(pool).await;
    if !matches!(exists, Ok(true)) {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM paperrust_redaction WHERE checksum = $1)")
        .bind(checksum)
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn migration_is_expand_only_and_idempotent() {
        let sql = super::MIGRATION.1.to_ascii_uppercase();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS PAPERRUST_REDACTION"));
        // documents_document is a view after the restructuring stages: no foreign key on it (startup error 2026-11-19)
        for bad in ["DROP ", "ALTER TABLE DOCUMENTS_", "TRUNCATE", "DELETE FROM", "REFERENCES DOCUMENTS_DOCUMENT"] {
            assert!(!sql.contains(bad), "{bad}");
        }
        assert!(super::MIGRATION.0.starts_with("pl_documents_write."));
    }
}
