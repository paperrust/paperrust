// upstream: src/documents/signals/handlers.py@v3.4.2 (before_task_publish_handler)
//
//! Tasks of the write paths: we create the `documents_paperlesstask` row **in the running
//! transaction** and only enqueue after the commit (`pl_core::tasks::prepare_task` +
//! `PreparedTask::publish`, PostgreSQL queue).

use chrono::Utc;
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use pl_core::tasks::{TaskRequest, tracked_task_type};

use crate::error::ApiError;

pub const BULK_UPDATE_DOCUMENTS: &str = "documents.tasks.bulk_update_documents";
pub const UPDATE_CONTENT_MAYBE_ARCHIVE: &str = "documents.tasks.update_document_content_maybe_archive_file";
/// Manual workflow run (PaperRust worker, `pl_consumer::manual`; name as paperless-ngx dev).
pub const RUN_WORKFLOWS_FOR_DOCUMENTS: &str = "documents.tasks.run_workflows_for_documents";

/// `PaperlessTask.TriggerSource`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerSource {
    System,
    Manual,
}

impl TriggerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerSource::System => pl_core::tasks::trigger_source::SYSTEM,
            TriggerSource::Manual => pl_core::tasks::trigger_source::MANUAL,
        }
    }
}

/// A task to be sent (kwargs only, like `apply_async(kwargs=…, headers={"trigger_source": …})`).
#[derive(Clone, Debug)]
pub struct Task {
    pub id: Uuid,
    pub name: &'static str,
    pub kwargs: serde_json::Map<String, Value>,
    pub trigger_source: TriggerSource,
}

impl Task {
    pub fn new(name: &'static str, kwargs: Value, trigger_source: TriggerSource) -> Self {
        let kwargs = match kwargs {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        Task { id: Uuid::new_v4(), name, kwargs, trigger_source }
    }

    pub fn request(&self) -> TaskRequest {
        let mut req = TaskRequest::new(self.name).trigger_source(self.trigger_source.as_str());
        req.kwargs = self.kwargs.clone();
        req.task_id = Some(self.id);
        req
    }
}

/// `before_task_publish_handler`: PaperlessTask row (get_or_create on task_id).
pub async fn record(conn: &mut PgConnection, task: &Task) -> Result<(), ApiError> {
    let Some(task_type) = tracked_task_type(task.name) else {
        return Ok(());
    };
    let mut q = Query::insert();
    q.into_table(Alias::new("documents_paperlesstask")).columns([
        Alias::new("task_id"),
        Alias::new("task_type"),
        Alias::new("trigger_source"),
        Alias::new("status"),
        Alias::new("date_created"),
        Alias::new("input_data"),
        Alias::new("acknowledged"),
        Alias::new("owner_id"),
    ]);
    q.values([
        task.id.to_string().into(),
        task_type.into(),
        task.trigger_source.as_str().into(),
        "pending".into(),
        Utc::now().into(),
        json!({}).into(),
        false.into(),
        Option::<i32>::None.into(),
    ])
    .map_err(|e| ApiError::Uncaught(format!("sea-query: {e}")))?;
    q.on_conflict(OnConflict::column(Alias::new("task_id")).do_nothing().to_owned());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *conn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked() {
        assert_eq!(tracked_task_type(UPDATE_CONTENT_MAYBE_ARCHIVE), Some("reprocess_document"));
        assert_eq!(tracked_task_type(BULK_UPDATE_DOCUMENTS), Some("bulk_update"));
    }

    #[test]
    fn request_has_trigger_header() {
        let t = Task::new(BULK_UPDATE_DOCUMENTS, json!({"document_ids": [3, 4]}), TriggerSource::System);
        let r = t.request();
        assert_eq!(r.headers.get("trigger_source"), Some(&json!("system")));
        assert_eq!(r.kwargs.get("document_ids"), Some(&json!([3, 4])));
        assert_eq!(r.task_id, Some(t.id));
    }
}
