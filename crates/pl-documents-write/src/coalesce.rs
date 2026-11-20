// upstream: src/documents/bulk_edit.py@v3.4.2 (every method: `bulk_update_documents.apply_async(
//           kwargs={"document_ids": affected_docs})`), src/documents/tasks.py@v3.4.2
//           (bulk_update_documents: per document `document_updated` + `post_save`, then index).
//
//! Coalesce the `bulk_update_documents` follow-up work (tests/stress/BEFUNDE.md B6).
//!
//! upstream enqueues a separate `bulk_update_documents` task over all affected documents per
//! bulk change. Many quick bulk changes (UI, scripts) thus fill the
//! queue faster than the worker processes it: search index and file names then lag
//! far behind.
//!
//! PaperRust instead appends the ids to a **not yet started** `bulk_update_documents`
//! task (same transaction as the change, so visible only after the commit) if there is
//! one. The appended ids stay **with repetitions**: a document that occurred in `k`
//! coalesced changes appears `k` times in `document_ids`, and
//! `pl_consumer::Consumer::bulk_update_signals` runs the "document updated" workflows
//! (incl. webhooks/e-mails), WebSocket and file name check `k` times for it – exactly as often
//! as upstream with `k` tasks. Only the search index and cache clearing run once (they only act on
//! the final state).
//!
//! Conditions for appending (otherwise a new task as upstream):
//! * PaperRust queue (`paperrust_task_queue`) present, `PAPERRUST_BULK_UPDATE_COALESCE`
//!   not `off`/`false`/`0`;
//! * target task `documents.tasks.bulk_update_documents`, state `queued`, never delivered
//!   (`attempts = 0`), due (`eta <= now()`), without expiry, revocation, group, callbacks, args and
//!   with exactly the kwarg `document_ids` (list);
//! * at most [`MAX_IDS`] entries in total;
//! * row not currently locked (`FOR UPDATE SKIP LOCKED`: the worker is just picking it up or another
//!   change is just appending to it) – never wait.

use serde_json::Value;
use sqlx::PgConnection;

use crate::enqueue::BULK_UPDATE_DOCUMENTS;
use crate::error::ApiError;

/// Maximum number of entries in `document_ids` of a coalesced task.
pub const MAX_IDS: usize = 20_000;

/// Coalescing enabled? (`PAPERRUST_BULK_UPDATE_COALESCE`, default on)
pub fn enabled(lookup: impl Fn(&str) -> Option<String>) -> bool {
    !matches!(
        lookup("PAPERRUST_BULK_UPDATE_COALESCE").map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "false" | "0" | "no")
    )
}

/// The ids of a `bulk_update_documents` kwarg (plain integer list only; otherwise `None`).
pub fn document_ids(kwargs: &serde_json::Map<String, Value>) -> Option<Vec<i64>> {
    if kwargs.len() != 1 {
        return None;
    }
    kwargs.get("document_ids")?.as_array()?.iter().map(Value::as_i64).collect()
}

const MERGE_SQL: &str = "UPDATE paperrust_task_queue q \
    SET kwargs = jsonb_set(q.kwargs, '{document_ids}', (q.kwargs -> 'document_ids') || $2::jsonb) \
    FROM (SELECT id FROM paperrust_task_queue \
           WHERE task_name = $1 AND state = 'queued' AND attempts = 0 AND eta <= now() \
             AND expires IS NULL AND NOT revoke_requested AND NOT terminate_requested AND group_id IS NULL \
             AND args = '[]'::jsonb AND link = '[]'::jsonb AND link_error = '[]'::jsonb \
             AND jsonb_typeof(kwargs -> 'document_ids') = 'array' \
             AND (SELECT count(*) FROM jsonb_object_keys(kwargs)) = 1 \
             AND jsonb_array_length(kwargs -> 'document_ids') + $3 <= $4 \
           ORDER BY id DESC LIMIT 1 FOR UPDATE SKIP LOCKED) c \
    WHERE q.id = c.id RETURNING q.task_id";

/// Append ids to a waiting `bulk_update_documents` task (in the caller's
/// transaction). `Some(task_id)` = appended (no new task needed), `None` = enqueue a new
/// task.
pub async fn try_merge(conn: &mut PgConnection, ids: &[i64]) -> Result<Option<String>, ApiError> {
    if ids.is_empty() || ids.len() > MAX_IDS || !enabled(pl_core::config::var) {
        return Ok(None);
    }
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('paperrust_task_queue') IS NOT NULL").fetch_one(&mut *conn).await?;
    if !exists {
        return Ok(None);
    }
    let add = Value::from(ids.to_vec());
    let task_id: Option<String> = sqlx::query_scalar(MERGE_SQL)
        .bind(BULK_UPDATE_DOCUMENTS)
        .bind(add)
        .bind(ids.len() as i64)
        .bind(MAX_IDS as i64)
        .fetch_optional(&mut *conn)
        .await?;
    if let Some(t) = &task_id {
        tracing::debug!(target: "paperless.bulk_edit", task_id = %t, documents = ids.len(), "bulk_update_documents: ids appended to pending task");
    }
    Ok(task_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn switch() {
        assert!(enabled(|_| None));
        assert!(enabled(|_| Some("on".into())));
        assert!(!enabled(|_| Some(" OFF ".into())));
        assert!(!enabled(|_| Some("0".into())));
        assert!(!enabled(|_| Some("false".into())));
    }

    #[test]
    fn ids_only_plain_kwargs() {
        let m = |v: Value| v.as_object().cloned().unwrap_or_default();
        assert_eq!(document_ids(&m(json!({"document_ids": [3, 1]}))), Some(vec![3, 1]));
        assert_eq!(document_ids(&m(json!({"document_ids": [3, "x"]}))), None);
        assert_eq!(document_ids(&m(json!({"document_ids": [3], "other": 1}))), None);
        assert_eq!(document_ids(&m(json!({}))), None);
    }
}
