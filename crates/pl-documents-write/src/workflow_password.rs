// upstream: src/documents/workflows/actions.py@v3.4.2 (execute_password_removal_action),
//           src/documents/bulk_edit.py@v3.4.2 (remove_password),
//           src/documents/signals/handlers.py@v3.4.2 (run_workflows → PASSWORD_REMOVAL)
//
//! Workflow action "remove password" (type 5, gap W1).
//!
//! Upstream tries the configured passwords in order with
//! `bulk_edit.remove_password([doc.id], password, update_document=True, user=document.owner)`;
//! the first one that fits produces a decrypted new version (task `consume_file` with
//! `root_document_id`). For the consumption trigger upstream hooks into
//! `document_consumption_finished` and runs with the finished document.
//!
//! PaperRust: the call comes synchronously from the workflow run (often inside the still open
//! ingestion transaction). The work therefore runs in the background and waits until the document
//! is committed (then the original file is also in its place; upstream passes the working copy
//! through as `source_paths_by_id` for that). For the consumption trigger the workflow
//! does not know the later document yet: the action registers with the input file and runs
//! when the consumer reports "ingestion finished" for exactly this file
//! (`WorkflowEffects::consumption_finished`, after the commit; upstream hooks into
//! the next `document_consumption_finished`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use sqlx::PgPool;

use pl_consumer::stages::ActionContext;
use pl_pdfops::bulk::SourceMode;
use pl_queue::TaskQueue;

use crate::files::MediaCfg;
use crate::pdf_edit::{Exec, PdfParams};

const LOG: &str = "paperless.workflows.actions";

/// Dependencies of the action (the worker builds them once at startup).
#[derive(Clone)]
pub struct PasswordRemovalDeps {
    pub pool: PgPool,
    pub queue: Arc<dyn TaskQueue>,
    pub media: MediaCfg,
    pub scratch: PathBuf,
    /// How long to wait for an existing document (commit of the transaction).
    pub commit_wait: Duration,
    /// Actions registered at the consumption trigger (input file → action), until "ingestion finished".
    pending: Arc<PendingActions>,
}

/// Registration at the consumption trigger.
#[derive(Debug, Clone)]
struct Pending {
    original_file: PathBuf,
    action_id: i64,
    at: Instant,
}

/// Discard registrations whose ingestion never finished (error, abort) after this time.
const PENDING_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

impl PasswordRemovalDeps {
    pub fn new(pool: PgPool, queue: Arc<dyn TaskQueue>, media: MediaCfg, scratch: PathBuf) -> Self {
        Self { pool, queue, media, scratch, commit_wait: Duration::from_secs(120), pending: Arc::default() }
    }
}

/// Actions registered at the consumption trigger.
#[derive(Debug, Default)]
struct PendingActions(Mutex<Vec<Pending>>);

impl PendingActions {
    fn register(&self, original_file: &Path, action_id: i64) {
        let now = Instant::now();
        let mut p = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        p.retain(|e| now.duration_since(e.at) < PENDING_MAX_AGE);
        p.push(Pending { original_file: original_file.to_path_buf(), action_id, at: now });
    }

    /// Take out the registered actions for this input file (in registration order).
    fn take(&self, original_file: &Path) -> Vec<i64> {
        let mut p = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::new();
        p.retain(|e| {
            if e.original_file == original_file {
                out.push(e.action_id);
                false
            } else {
                true
            }
        });
        out
    }
}

/// `action.passwords` → `[p.strip() for p in passwords if p.strip()]` (strings only; the serializer
/// does not allow other JSON values).
pub fn clean_passwords(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Entry from `WorkflowEffects::password_removal`: existing document → run in the background;
/// consumption trigger (no document yet) → register for "ingestion finished".
pub fn spawn(deps: Arc<PasswordRemovalDeps>, ctx: &ActionContext) {
    match ctx.document_id {
        Some(id) => run_in_background(deps, ctx.workflow_action_id, id),
        None => deps.pending.register(&ctx.original_file, ctx.workflow_action_id),
    }
}

/// Entry from `WorkflowEffects::consumption_finished`: run the actions registered for this input
/// file with the finished document.
pub fn consumption_finished(deps: Arc<PasswordRemovalDeps>, doc_id: i64, original_file: &Path) {
    for action_id in deps.pending.take(original_file) {
        run_in_background(deps.clone(), action_id, doc_id);
    }
}

fn run_in_background(deps: Arc<PasswordRemovalDeps>, action_id: i64, doc_id: i64) {
    let fut = async move {
        if let Err(e) = execute(&deps, action_id, doc_id).await {
            tracing::error!(target: LOG, "Workflow action {action_id} (password removal) failed: {e}");
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(fut);
        }
        Err(_) => tracing::warn!(target: LOG, "Workflow action {action_id} not executed (no async runtime)"),
    }
}

async fn load_passwords(pool: &PgPool, action_id: i64) -> Result<Vec<String>, String> {
    let v: Option<Option<Value>> = sqlx::query_scalar("SELECT passwords FROM documents_workflowaction WHERE id = $1")
        .bind(action_id as i32)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(v.flatten().map(|v| clean_passwords(&v)).unwrap_or_default())
}

async fn find_document(pool: &PgPool, id: i64) -> Result<Option<i64>, String> {
    let id: Option<i32> = sqlx::query_scalar("SELECT id FROM documents_document WHERE id = $1 AND deleted_at IS NULL")
        .bind(id as i32)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(id.map(i64::from))
}

/// Wait for the commit of the transaction in which the workflow ran.
async fn wait_for_document(deps: &PasswordRemovalDeps, id: i64) -> Result<Option<i64>, String> {
    let deadline = tokio::time::Instant::now() + deps.commit_wait;
    let step = Duration::from_millis(250);
    loop {
        if let Some(id) = find_document(&deps.pool, id).await? {
            return Ok(Some(id));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(step).await;
    }
}

/// `execute_password_removal_action(action, document, logging_group)`.
async fn execute(deps: &PasswordRemovalDeps, action_id: i64, target: i64) -> Result<(), String> {
    let passwords = load_passwords(&deps.pool, action_id).await?;
    if passwords.is_empty() {
        tracing::warn!(target: LOG, "Workflow action {action_id} has no configured unlock values");
        return Ok(());
    }
    let Some(doc_id) = wait_for_document(deps, target).await? else {
        tracing::warn!(target: LOG, "Workflow action {action_id}: document not available in time, password removal skipped");
        return Ok(());
    };
    let owner: Option<i32> = sqlx::query_scalar("SELECT owner_id FROM documents_document WHERE id = $1")
        .bind(doc_id as i32)
        .fetch_optional(&deps.pool)
        .await
        .map_err(|e| e.to_string())?
        .flatten();
    let ex = Exec {
        pool: deps.pool.clone(),
        queue: deps.queue.clone(),
        media: deps.media.clone(),
        scratch: deps.scratch.clone(),
        user_id: owner.map(i64::from),
        // remove_password(trigger_source=WEB_UI) – Standardwert upstream
        trigger: pl_core::tasks::trigger_source::WEB_UI,
        mode: SourceMode::default(),
    };
    for password in passwords {
        let params = PdfParams::RemovePassword {
            password,
            update_document: true,
            delete_original: false,
            include_metadata: true,
            source_paths_by_id: None,
        };
        match ex.run(&[doc_id], &params).await {
            Ok(()) => {
                tracing::info!(target: LOG, "Unlocked document {doc_id} using workflow action {action_id}");
                return Ok(());
            }
            Err(_) => {
                tracing::warn!(target: LOG, "Workflow action {action_id} could not unlock document {doc_id} with one configured value");
            }
        }
    }
    tracing::error!(target: LOG, "Workflow action {action_id} could not unlock document {doc_id} with any configured value");
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn passwords_stripped_like_upstream() {
        assert_eq!(clean_passwords(&json!(["", " a ", "b", "   "])), vec!["a".to_string(), "b".to_string()]);
        assert!(clean_passwords(&Value::Null).is_empty());
        assert!(clean_passwords(&json!([])).is_empty());
    }

    /// Consumption trigger: registration per input file, "ingestion finished" takes only its own.
    #[test]
    fn consumption_trigger_registers_until_finished() {
        let p = PendingActions::default();
        p.register(Path::new("a.pdf"), 1);
        p.register(Path::new("b.pdf"), 2);
        p.register(Path::new("a.pdf"), 3);
        assert_eq!(p.take(Path::new("a.pdf")), vec![1, 3]);
        assert!(p.take(Path::new("a.pdf")).is_empty());
        assert_eq!(p.take(Path::new("b.pdf")), vec![2]);
    }
}
