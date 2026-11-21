// Manual workflow run on selected documents (task `run_workflows_for_documents`).
//
// Own implementation after the idea from paperless-ngx dev `3dbeca603` (#14704,
// `documents.tasks.run_workflows_for_documents`): the web UI enqueues this task via
// `bulk_edit` (`method=run_workflow`), the worker applies the workflow to each
// root document. Order like `Document.objects` (`-created`), deleted documents and
// versions are skipped. An error on one document does not abort the run.

use crate::consumer::Consumer;
use crate::db;
use crate::docops::DocInstance;
use crate::workflows;

/// Celery/PgQueue name of the task (same name as in paperless-ngx dev, so that a later
/// shared worker recognizes it).
pub const RUN_WORKFLOWS_FOR_DOCUMENTS: &str = "documents.tasks.run_workflows_for_documents";

/// Result of a manual run (for tests and log).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ManualRunReport {
    /// Documents to which the workflow was applied (filter matching or skipped).
    pub attempted: Vec<i64>,
    /// Documents with an error (logged, the run continued).
    pub failed: Vec<i64>,
    /// `false` if the workflow no longer exists (warning, no exception).
    pub workflow_found: bool,
}

/// Root documents from `ids` (not deleted) in the order `-created, id`.
pub const ROOT_DOCS_SQL: &str = "SELECT id FROM documents_document WHERE id = ANY($1) AND root_document_id IS NULL \
     AND deleted_at IS NULL ORDER BY created DESC, id ASC";

impl Consumer {
    /// `run_workflows_for_documents(document_ids, workflow_id, *, filter_documents=True)`.
    ///
    /// `filter_documents = false` applies the actions without checking the trigger filters.
    /// Autocommit like upstream (each save on its own).
    pub async fn run_workflows_for_documents(
        &self,
        document_ids: &[i64],
        workflow_id: i64,
        filter_documents: bool,
    ) -> Result<ManualRunReport, sqlx::Error> {
        self.run_workflows_for_documents_as(document_ids, workflow_id, filter_documents, None).await
    }

    /// PaperRust: like [`Self::run_workflows_for_documents`]; `match_trigger` checks the filters of the
    /// triggers of this type instead of "manual" (caught-up runs with an AI condition, "apply
    /// retroactively" from the test run). The AI condition is checked immediately, non-matching
    /// conditions end up in the history.
    pub async fn run_workflows_for_documents_as(
        &self,
        document_ids: &[i64],
        workflow_id: i64,
        filter_documents: bool,
        match_trigger: Option<i64>,
    ) -> Result<ManualRunReport, sqlx::Error> {
        let mut report = ManualRunReport::default();
        let mut conn = self.pool.acquire().await?;
        let Some(workflow) = workflows::workflow_by_id(&mut conn, workflow_id).await? else {
            tracing::warn!(target: "paperless.tasks", "Manual workflow run requested for missing workflow {workflow_id}");
            return Ok(report);
        };
        report.workflow_found = true;
        let ids32: Vec<i32> = document_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
        let docs: Vec<i32> = sqlx::query_scalar(ROOT_DOCS_SQL).bind(&ids32).fetch_all(&mut *conn).await?;
        let audit = crate::audit::AuditLog { enabled: self.settings.audit_log_enabled, actor_id: None };
        let trigger = match_trigger.unwrap_or(pl_matching::workflow::TRIGGER_MANUAL);
        for id in docs.into_iter().map(i64::from) {
            report.attempted.push(id);
            let result: Result<(), sqlx::Error> = async {
                let Some(row) = db::load_document(&mut conn, id).await? else { return Ok(()) };
                let mut doc = DocInstance::from_db(row);
                let mut env = self.run_env(&audit);
                env.ext = crate::workflow_ext::ExtOptions { ai_mode: crate::workflow_ext::AiMode::Inline, log_skips: true };
                let outcome = workflows::run_workflow_as(&mut conn, &env, &mut doc, &workflow, !filter_documents, trigger).await?;
                workflows::enqueue_ai_suggestions(self.queue.as_deref(), &outcome).await;
                // Manual runs send no document_updated signal: clients update here.
                self.send_websocket_document_updated(&mut conn, id).await
            }
            .await;
            if let Err(e) = result {
                report.failed.push(id);
                tracing::error!(target: "paperless.tasks", "Error running workflow {} on document {id}: {e}", workflow.name);
            }
        }
        Ok(report)
    }
}

/// kwargs of the task incl. PaperRust extension `match_trigger` (trigger type 1–5, missing/`null` →
/// manual).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualKwargs {
    pub document_ids: Vec<i64>,
    pub workflow_id: i64,
    pub filter_documents: bool,
    pub match_trigger: Option<i64>,
}

/// [`parse_kwargs`] + `match_trigger`.
pub fn parse_kwargs_ext(kwargs: &serde_json::Map<String, serde_json::Value>) -> Result<ManualKwargs, String> {
    let (document_ids, workflow_id, filter_documents) = parse_kwargs(kwargs)?;
    let match_trigger = match kwargs.get("match_trigger") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_i64() {
            Some(t @ 2..=5) => Some(t),
            _ => return Err("match_trigger must be a trigger type of an existing document (2-5)".into()),
        },
    };
    Ok(ManualKwargs { document_ids, workflow_id, filter_documents, match_trigger })
}

/// Read the kwargs of the task: (`document_ids`, `workflow_id`, `filter_documents`).
/// `filter_documents` missing → `true` (default as upstream).
pub fn parse_kwargs(kwargs: &serde_json::Map<String, serde_json::Value>) -> Result<(Vec<i64>, i64, bool), String> {
    let ids = kwargs
        .get("document_ids")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "document_ids missing".to_string())?
        .iter()
        .map(|v| v.as_i64().ok_or_else(|| "document_ids must be integers".to_string()))
        .collect::<Result<Vec<i64>, String>>()?;
    let workflow_id = kwargs.get("workflow_id").and_then(serde_json::Value::as_i64).ok_or_else(|| "workflow_id missing".to_string())?;
    let filter_documents = match kwargs.get("filter_documents") {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(_) => return Err("filter_documents must be a boolean".into()),
    };
    Ok((ids, workflow_id, filter_documents))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kw(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn kwargs_defaults_and_errors() {
        assert_eq!(parse_kwargs(&kw(json!({"document_ids": [3, 4], "workflow_id": 5}))), Ok((vec![3, 4], 5, true)));
        assert_eq!(parse_kwargs(&kw(json!({"document_ids": [], "workflow_id": 5, "filter_documents": false}))), Ok((vec![], 5, false)));
        assert!(parse_kwargs(&kw(json!({"workflow_id": 5}))).is_err());
        assert!(parse_kwargs(&kw(json!({"document_ids": ["x"], "workflow_id": 5}))).is_err());
        assert!(parse_kwargs(&kw(json!({"document_ids": [1], "workflow_id": 5, "filter_documents": "no"}))).is_err());
    }

    #[test]
    fn kwargs_match_trigger() {
        let k = parse_kwargs_ext(&kw(json!({"document_ids": [1], "workflow_id": 2, "match_trigger": 2}))).unwrap();
        assert_eq!(k.match_trigger, Some(2));
        assert_eq!(parse_kwargs_ext(&kw(json!({"document_ids": [1], "workflow_id": 2}))).unwrap().match_trigger, None);
        assert!(parse_kwargs_ext(&kw(json!({"document_ids": [1], "workflow_id": 2, "match_trigger": 1}))).is_err());
        assert!(parse_kwargs_ext(&kw(json!({"document_ids": [1], "workflow_id": 2, "match_trigger": "x"}))).is_err());
    }

    #[test]
    fn root_docs_query_skips_versions_and_trash() {
        assert!(ROOT_DOCS_SQL.contains("root_document_id IS NULL"));
        assert!(ROOT_DOCS_SQL.contains("deleted_at IS NULL"));
        assert!(ROOT_DOCS_SQL.contains("ORDER BY created DESC"));
    }
}
