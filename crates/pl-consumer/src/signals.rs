// Signal receivers for existing documents that the worker triggers (not ingestion):
// `document_updated` ("document updated" workflows, WebSocket) + `post_save`
// (`update_filename_and_move_files`) from `bulk_update_documents`.
//
// upstream: src/documents/tasks.py@v3.4.2 (bulk_update_documents, signal part),
//           src/documents/signals/handlers.py@v3.4.2 (run_workflows_updated,
//           send_websocket_document_updated, update_filename_and_move_files),
//           src/documents/apps.py@v3.4.2 (receiver order of document_updated),
//           src/documents/caching.py@v3.4.2 (clear_document_caches),
//           src/documents/data_models.py@v3.4.2 (DocumentMetadataOverrides.from_document),
//           src/documents/permissions.py@v3.4.2 (get_permission_grantees).

use sqlx::PgConnection;

use crate::audit::AuditLog;
use crate::consumer::Consumer;
use crate::db;
use crate::docops::DocInstance;
use crate::stages::BoxFuture;
use crate::workflows::{self, RunEnv, RunOutcome};

/// `clear_document_caches(document_id)` (Django cache in Redis: suggestions, metadata,
/// thumbnail timestamp, AI suggestions). The worker plugs in a Redis implementation.
pub trait DocumentCaches: Send + Sync {
    fn clear_document_caches(&self, document_id: i64) -> BoxFuture<'_, ()>;

    /// `clear_document_caches` for many documents (bundled follow-up). Default: one by one.
    fn clear_many<'a>(&'a self, document_ids: &'a [i64]) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            for id in document_ids {
                self.clear_document_caches(*id).await;
            }
        })
    }
}

/// No cache (tests, operation without Redis).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCaches;

impl DocumentCaches for NoCaches {
    fn clear_document_caches(&self, _document_id: i64) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// Django cache keys of `clear_document_caches` without the AI generation
/// (`get_suggestion_cache_key`, `get_metadata_cache_key`, `get_thumbnail_modified_key`) in
/// Django's `KEY_PREFIX:VERSION:` form (`<prefix>:1:doc_<id>_…`).
pub fn document_cache_keys(prefix: &str, document_id: i64) -> [String; 3] {
    ["suggest", "metadata", "thumbnail_modified"].map(|k| format!("{prefix}:1:doc_{document_id}_{k}"))
}

/// `get_permission_grantees(Document, [pk])[pk]["view"]`: (users, groups) with the direct
/// `view_document` object permission.
pub async fn view_grantees(conn: &mut PgConnection, document_id: i64) -> Result<(Vec<i64>, Vec<i64>), sqlx::Error> {
    let mut out = Vec::new();
    for (table, col) in [("guardian_userobjectpermission", "user_id"), ("guardian_groupobjectpermission", "group_id")] {
        let sql = format!(
            "SELECT DISTINCT o.{col} FROM {table} o \
             JOIN auth_permission p ON p.id = o.permission_id \
             JOIN django_content_type c ON c.id = o.content_type_id \
             WHERE o.object_pk = $1 AND p.codename = 'view_document' \
               AND c.app_label = 'documents' AND c.model = 'document' ORDER BY 1"
        );
        let ids: Vec<i32> = sqlx::query_scalar(&sql).bind(document_id.to_string()).fetch_all(&mut *conn).await?;
        out.push(ids.into_iter().map(i64::from).collect::<Vec<_>>());
    }
    let groups = out.pop().unwrap_or_default();
    let users = out.pop().unwrap_or_default();
    Ok((users, groups))
}

impl Consumer {
    fn worker_audit(&self) -> AuditLog {
        AuditLog { enabled: self.settings.audit_log_enabled, actor_id: None }
    }

    /// Environment for workflows on existing documents (with file renaming after `save()`).
    pub fn run_env<'a>(&'a self, audit: &'a AuditLog) -> RunEnv<'a> {
        RunEnv {
            audit,
            effects: self.effects.as_ref(),
            time_zone: self.settings.time_zone,
            original_file: None,
            settings: Some(self.settings.as_ref()),
            deleter: self.deleter.as_ref(),
            // Worker: check the AI condition immediately (document is committed)
            ext: crate::workflow_ext::ExtOptions::default(),
        }
    }

    /// `send_websocket_document_updated(document)`: `refresh_from_db()`, owner and
    /// `view` grantees, `modified` like `DRF_DATETIME_FIELD.to_representation`.
    pub async fn send_websocket_document_updated(&self, conn: &mut PgConnection, document_id: i64) -> Result<(), sqlx::Error> {
        let Some(bus) = &self.events else { return Ok(()) };
        let Some(d) = db::load_document(conn, document_id).await? else { return Ok(()) };
        let (users, groups) = view_grantees(conn, document_id).await?;
        let modified = pl_core::datetime::format_datetime(&d.modified, &self.settings.time_zone);
        pl_queue::events::DocumentsStatusManager::new(bus.clone()).send_document_updated(
            document_id,
            modified,
            pl_queue::events::ViewPermissions { owner_id: d.owner_id, users_can_view: users, groups_can_view: groups },
        );
        Ok(())
    }

    /// `document_updated.send(document=doc)` without the AI index receiver (`skip_ai_index=True`):
    /// `run_workflows_updated` → `send_websocket_document_updated`.
    pub async fn document_updated(&self, conn: &mut PgConnection, document_id: i64) -> Result<RunOutcome, sqlx::Error> {
        let Some(row) = db::load_document(conn, document_id).await? else { return Ok(RunOutcome::default()) };
        let audit = self.worker_audit();
        let mut doc = DocInstance::from_db(row);
        let outcome =
            workflows::run_workflows(conn, &self.run_env(&audit), pl_matching::workflow::TRIGGER_DOCUMENT_UPDATED, &mut doc, None)
                .await?;
        self.send_websocket_document_updated(conn, document_id).await?;
        Ok(outcome)
    }

    /// Signal part of `bulk_update_documents(document_ids)`: for each document from
    /// `Document.objects.filter(id__in=…)` (without trash, order `-created`):
    /// `clear_document_caches`, `document_updated`, `post_save` (`update_filename_and_move_files`).
    /// The index (`batch.add_or_update_ids`) and the AI index stay with the caller.
    /// Autocommit like upstream (Celery task without `atomic`); `apply_ai_suggestions` from the
    /// workflows is enqueued directly (`delay_on_commit` without a transaction = immediately).
    ///
    /// Single path (everything individually per document, like upstream); the default is the bundled path
    /// [`Consumer::bulk_update_signals`] (crate::bulk) with the same effect.
    pub async fn bulk_update_signals_single(&self, document_ids: &[i64]) -> Result<(), sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        let ids32: Vec<i32> = document_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
        let docs: Vec<i32> = sqlx::query_scalar(
            "SELECT id FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY created DESC, id",
        )
        .bind(&ids32)
        .fetch_all(&mut *conn)
        .await?;
        for id in docs {
            let id = i64::from(id);
            self.caches.clear_document_caches(id).await;
            let outcome = self.document_updated(&mut conn, id).await?;
            workflows::enqueue_ai_suggestions(self.queue.as_deref(), &outcome).await;
            crate::files::update_filename_and_move_files(&mut conn, &self.settings, id).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_like_django() {
        assert_eq!(
            document_cache_keys("pngx", 7),
            ["pngx:1:doc_7_suggest".to_string(), "pngx:1:doc_7_metadata".into(), "pngx:1:doc_7_thumbnail_modified".into()]
        );
    }
}
