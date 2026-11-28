//! PaperRust system notes (`documents_note` with `user_id = NULL`) written by ingestion.
//!
//! Extension without upstream counterpart: when a parser limit (pages, time, text size, MSG
//! property size) cut the extracted text short, the document gets one note saying so, so users
//! know that search and suggestions only see part of the text. Same mechanism as the AI
//! transcription note of pl-ai (`server::vision::record_inner`): one system note per document
//! and marker, updated instead of duplicated; the `format` column (PaperRust notes) is filled if
//! present.

use sqlx::PgConnection;

/// Start of the note text; also used to find an earlier note of this kind.
pub const TRUNCATION_MARKER: &str = "Search text incomplete";

/// Note text for a truncation description (`Truncation::message`, several joined by `; `).
pub fn truncation_note(message: &str) -> String {
    format!(
        "{TRUNCATION_MARKER}: {message}. The file itself is stored completely; only the text used for search, \
         matching and suggestions is cut off."
    )
}

/// Sets the truncation note of `doc_id` (inserts it or updates the existing system note).
pub async fn set_truncation(conn: &mut PgConnection, doc_id: i64, message: &str) -> Result<(), sqlx::Error> {
    upsert(conn, doc_id, TRUNCATION_MARKER, &truncation_note(message)).await
}

/// Soft-deletes the truncation note of `doc_id` (reprocessing produced the complete text).
pub async fn clear_truncation(conn: &mut PgConnection, doc_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE documents_note SET deleted_at = now(), restored_at = NULL \
         WHERE document_id = $1 AND user_id IS NULL AND deleted_at IS NULL AND note LIKE $2",
    )
    .bind(doc_id as i32)
    .bind(like_prefix(TRUNCATION_MARKER))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `LIKE` pattern `<marker>:%` (the markers contain no `%`/`_`).
fn like_prefix(marker: &str) -> String {
    format!("{marker}:%")
}

async fn upsert(conn: &mut PgConnection, doc_id: i64, marker: &str, text: &str) -> Result<(), sqlx::Error> {
    let doc = doc_id as i32;
    let existing: Option<i32> = sqlx::query_scalar(
        "SELECT id FROM documents_note WHERE document_id = $1 AND user_id IS NULL AND deleted_at IS NULL AND note LIKE $2 \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(doc)
    .bind(like_prefix(marker))
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(id) = existing {
        sqlx::query("UPDATE documents_note SET note = $1 WHERE id = $2").bind(text).bind(id).execute(&mut *conn).await?;
        return Ok(());
    }
    let has_format: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = 'documents_note' AND column_name = 'format')",
    )
    .fetch_one(&mut *conn)
    .await?;
    let sql = if has_format {
        "INSERT INTO documents_note (note, format, created, document_id, user_id) VALUES ($1, 'plain', now(), $2, NULL)"
    } else {
        "INSERT INTO documents_note (note, created, document_id, user_id) VALUES ($1, now(), $2, NULL)"
    };
    sqlx::query(sql).bind(text).bind(doc).execute(&mut *conn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_starts_with_marker_and_keeps_message() {
        let msg = "Text extraction stopped at the page limit (2): only 2 of 3 pages were read";
        let n = truncation_note(msg);
        assert!(n.starts_with("Search text incomplete: "), "{n}");
        assert!(n.contains(msg));
        // the lookup pattern matches the note
        let p = like_prefix(TRUNCATION_MARKER);
        assert!(n.starts_with(p.trim_end_matches('%')));
    }
}
