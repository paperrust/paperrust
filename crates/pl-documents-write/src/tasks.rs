// upstream: src/documents/bulk_edit.py@v3.4.2 (delete, restore_archive_serial_numbers_task),
//           src/documents/tasks.py@v3.4.2 (empty_trash), src/paperless/settings/__init__.py@v3.4.2
//           (EMPTY_TRASH_DELAY)
//
//! Worker tasks of this crate (pl-server registers them under the upstream names):
//!
//! * `documents.bulk_edit.delete(doc_ids)` → [`bulk_delete`]
//! * `documents.bulk_edit.restore_archive_serial_numbers_task(backup, *args)` →
//!   [`crate::pdf_edit::restore_asn_task`]
//! * `documents.tasks.empty_trash(doc_ids=None)` → [`empty_trash`]

use pl_core::AppState;
use serde_json::Value;

use crate::auditlog::LogCtx;
use crate::bulk_edit::{ExecCtx, Params};
use crate::files::MediaCfg;

pub use crate::pdf_edit::restore_asn_task;

/// LogEntries without a request (worker): no actor, no address.
fn worker_log(state: &AppState) -> LogCtx {
    LogCtx { enabled: state.config().audit_log_enabled, ..Default::default() }
}

/// `bulk_edit.delete(doc_ids)`: roots + their versions into the trash (one
/// `transaction_id`), remove from the search index, `documents_deleted` to the clients. Errors are only
/// logged like upstream; result always `"OK"`. Returns: the deleted ids (for the message).
pub async fn bulk_delete(state: &AppState, doc_ids: &[i64]) -> Vec<i64> {
    bulk_delete_logged(state, doc_ids, worker_log(state)).await
}

/// Like [`bulk_delete`] with the auditlog context of the caller (request: middleware actor,
/// address, cid – `POST /api/documents/delete/` calls `bulk_edit.delete` directly in the request).
pub async fn bulk_delete_logged(state: &AppState, doc_ids: &[i64], log: LogCtx) -> Vec<i64> {
    match bulk_delete_inner(state, doc_ids, log).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!(target: "paperless.bulk_edit", "Error deleting documents: {e}");
            Vec::new()
        }
    }
}

async fn bulk_delete_inner(state: &AppState, doc_ids: &[i64], log: LogCtx) -> Result<Vec<i64>, String> {
    let pool = state.db();
    let ids32: Vec<i32> = doc_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    // root_ids = filter(id__in, root_document IS NULL); version_ids = filter(root_document_id__in=root_ids)
    // .exclude(id__in=doc_ids); delete_ids = {*doc_ids, *version_ids}
    let versions: Vec<i32> = sqlx::query_scalar(
        "SELECT DISTINCT v.id FROM documents_document v WHERE v.deleted_at IS NULL AND NOT (v.id = ANY($1)) \
         AND v.root_document_id IN (SELECT r.id FROM documents_document r WHERE r.id = ANY($1) \
         AND r.root_document_id IS NULL AND r.deleted_at IS NULL)",
    )
    .bind(&ids32)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut delete_ids: Vec<i64> = doc_ids.to_vec();
    delete_ids.extend(versions.into_iter().map(i64::from));
    delete_ids.sort_unstable();
    delete_ids.dedup();

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let ex = ExecCtx { log, media: MediaCfg::from_config(&state.live_config()) };
    crate::bulk_edit::execute(&mut tx, doc_ids, &Params::Delete, &ex).await.map_err(|e| format!("{e:?}"))?;
    tx.commit().await.map_err(|e| e.to_string())?;
    crate::index::remove(state, &delete_ids).await;
    if let Some(bus) = state.service::<pl_queue::EventBus>() {
        pl_queue::events::DocumentsStatusManager::new(bus).send_documents_deleted(delete_ids.clone());
    }
    Ok(delete_ids)
}

/// `doc_ids` of a task (`args[0]` or kwarg): list of integers, `None` = missing/null.
pub fn doc_ids_arg(args: &[Value], kwargs: &serde_json::Map<String, Value>, name: &str) -> Result<Option<Vec<i64>>, String> {
    match kwargs.get(name).or_else(|| args.first()) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_i64().ok_or_else(|| format!("{name} must be a list of integers")))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("{name} must be a list of integers")),
    }
}

/// `documents.tasks.empty_trash(doc_ids=None)`: without `doc_ids` all documents that have been in the trash longer than
/// `EMPTY_TRASH_DELAY` days (`deleted_at < now - timedelta(days)`), otherwise
/// the given ones (deleted only). Hard deletion via [`crate::softdelete::empty_trash`]
/// (cascade, files, LogEntries). Errors are only logged like upstream. Returns: count.
pub async fn empty_trash(state: &AppState, doc_ids: Option<&[i64]>) -> usize {
    match empty_trash_inner(state, doc_ids).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(target: "paperless.tasks", "Error while emptying trash: {e}");
            0
        }
    }
}

async fn empty_trash_inner(state: &AppState, doc_ids: Option<&[i64]>) -> Result<usize, String> {
    let pool = state.db();
    let selected: Vec<i64> = match doc_ids {
        Some(ids) => ids.to_vec(),
        None => {
            tracing::info!(target: "paperless.tasks", "Emptying trash of all expired documents");
            // timedelta(days=N) = N·24 h (independent of daylight saving time)
            let rows: Vec<i32> = sqlx::query_scalar(
                "SELECT id FROM documents_document WHERE deleted_at IS NOT NULL \
                 AND deleted_at < now() - make_interval(hours => $1) ORDER BY id",
            )
            .bind((state.config().empty_trash_delay.max(1) * 24) as i32)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
            rows.into_iter().map(i64::from).collect()
        }
    };
    let ids32: Vec<i32> = selected.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM documents_document WHERE deleted_at IS NOT NULL AND id = ANY($1)")
        .bind(&ids32)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    crate::softdelete::empty_trash(&mut tx, &worker_log(state), &MediaCfg::from_config(&state.live_config()), &selected)
        .await
        .map_err(|e| format!("{e:?}"))?;
    tx.commit().await.map_err(|e| e.to_string())?;
    let n = usize::try_from(count).unwrap_or(0);
    tracing::info!(target: "paperless.tasks", "Deleted {n} documents from trash");
    Ok(n)
}

/// `documents.signals.handlers.process_cf_select_update(custom_field)` (docs/STANDALONE-GAPS.md
/// §4.3). Upstream pickles the `CustomField`; the PaperRust queue sends `custom_field_id` and the
/// field is read again (a deleted field → nothing to do).
///
/// 1. `custom_field.fields.exclude(value_select__in=<option ids>).update(value_select=None)`
///    (default manager: instances not soft-deleted; an empty option list clears all).
/// 2. `update_filename_and_move_files(CustomFieldInstance, i)` for every instance: rename only when
///    the document's template uses `custom_fields`. Move errors are logged, like the receiver.
pub async fn process_cf_select_update(state: &AppState, field_id: i64) -> Result<(), String> {
    let db = state.db();
    let extra: Option<Option<serde_json::Value>> = sqlx::query_scalar("SELECT extra_data FROM documents_customfield WHERE id = $1")
        .bind(field_id as i32)
        .fetch_optional(db)
        .await
        .map_err(|e| e.to_string())?;
    let Some(extra) = extra else { return Ok(()) };
    let keys: Vec<String> = extra
        .as_ref()
        .and_then(|e| e.get("select_options"))
        .and_then(serde_json::Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|o| o.get("id"))
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    sqlx::query(
        "UPDATE documents_customfieldinstance SET value_select = NULL \
         WHERE field_id = $1 AND deleted_at IS NULL AND NOT (value_select IS NOT NULL AND value_select = ANY($2::text[]))",
    )
    .bind(field_id as i32)
    .bind(&keys)
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    let docs: Vec<i64> = sqlx::query_scalar(
        "SELECT document_id::bigint FROM documents_customfieldinstance WHERE field_id = $1 AND deleted_at IS NULL ORDER BY id",
    )
    .bind(field_id as i32)
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;
    let cfg = crate::files::MediaCfg::from_config(&state.live_config());
    let mut conn = db.acquire().await.map_err(|e| e.to_string())?;
    for d in docs {
        if let Err(e) = crate::files::after_custom_field_saved(&mut conn, &cfg, d).await {
            tracing::warn!(target: "paperless.handlers", document = d, "process_cf_select_update: {e:?}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    #[test]
    fn doc_ids_argument() {
        let kw = Map::new();
        assert_eq!(doc_ids_arg(&[json!([1, 2])], &kw, "doc_ids"), Ok(Some(vec![1, 2])));
        assert_eq!(doc_ids_arg(&[], &kw, "doc_ids"), Ok(None));
        assert_eq!(doc_ids_arg(&[Value::Null], &kw, "doc_ids"), Ok(None));
        assert!(doc_ids_arg(&[json!("x")], &kw, "doc_ids").is_err());
        let mut kw = Map::new();
        kw.insert("doc_ids".into(), json!([7]));
        assert_eq!(doc_ids_arg(&[], &kw, "doc_ids"), Ok(Some(vec![7])));
    }
}
