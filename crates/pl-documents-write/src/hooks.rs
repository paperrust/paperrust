// upstream: src/documents/workflows/actions.py@v3.4.2 (execute_email_action, execute_webhook_action,
//           execute_password_removal_action), src/documents/bulk_edit.py@v3.4.2 (rotate, merge, split,
//           delete_pages, edit_pdf, remove_password), src/documents/views.py@v3.4.2 (document_updated →
//           send_websocket_document_updated, DocumentsStatusManager.send_documents_deleted),
//           src/documents/apps.py@v3.4.2 (document_updated.connect(add_or_update_document_in_llm_index))
//
//! Interfaces to other crates that are populated at runtime:
//!
//! * [`set_workflow_effects`] – e-mail/webhook/password actions of the workflows (**W6**, pl-mail).
//!   Same trait as in the consumer (`pl_consumer::stages::WorkflowEffects`), so that one
//!   implementation serves both triggers. Default: [`DefaultEffects`] (log only, like upstream without
//!   `EMAIL_ENABLED`; webhook → Celery `send_webhook` without attachment).
//! * [`set_pdf_ops`] – PDF methods of bulk_edit. Implementation: [`crate::pdf_edit::NativePdfOps`]
//!   (pl-pdfops + PaperRust queue; pl-server registers it). Default: none → the
//!   handler delegates to Django.
//! * [`set_document_updated_hook`] – further receivers of the `document_updated` signal outside
//!   this crate (pl-ai: `add_or_update_document_in_llm_index`). Default: none.

use std::sync::{Arc, OnceLock, RwLock};

use serde_json::{Map, Value};

pub use pl_consumer::stages::{ActionContext, WorkflowEffects};

/// Default side effects without network.
#[derive(Debug, Default)]
pub struct DefaultEffects;

impl WorkflowEffects for DefaultEffects {
    fn email(&self, ctx: &ActionContext) {
        tracing::error!(
            target: "paperless.workflows.actions",
            "Email backend has not been configured, cannot send email notifications (action {})",
            ctx.workflow_action_id
        );
    }
    fn webhook(&self, ctx: &ActionContext) {
        tracing::info!(
            target: "paperless.workflows.actions",
            "Webhook action {} for document {:?} not sent (no webhook sender registered)",
            ctx.workflow_action_id,
            ctx.document_id
        );
    }
    fn password_removal(&self, ctx: &ActionContext) {
        tracing::warn!(
            target: "paperless.workflows.actions",
            "Password removal action {} not available without PDF backend",
            ctx.workflow_action_id
        );
    }
}

fn effects_slot() -> &'static RwLock<Arc<dyn WorkflowEffects>> {
    static SLOT: OnceLock<RwLock<Arc<dyn WorkflowEffects>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(Arc::new(DefaultEffects)))
}

/// Registers the implementation for e-mail/webhook/password (W6 calls this at startup).
pub fn set_workflow_effects(e: Arc<dyn WorkflowEffects>) {
    if let Ok(mut g) = effects_slot().write() {
        *g = e;
    }
}

pub fn workflow_effects() -> Arc<dyn WorkflowEffects> {
    match effects_slot().read() {
        Ok(g) => g.clone(),
        Err(_) => Arc::new(DefaultEffects),
    }
}

/// PDF methods of `POST /api/documents/bulk_edit/` (`rotate`, `merge`, `split`, `delete_pages`,
/// `edit_pdf`, `remove_password`). W5 takes over the whole request from the parsed JSON body
/// (validation `BulkEditSerializer` incl. selection, permissions, execution, response), because the
/// validation of these methods needs PDF knowledge (page counts, operations).
///
/// `None` = not handled → the handler delegates to Django.
pub trait PdfOps: Send + Sync {
    fn bulk_edit<'a>(
        &'a self,
        state: &'a pl_core::AppState,
        user: &'a pl_authz::User,
        method: &'a str,
        data: &'a Map<String, Value>,
        meta: &'a crate::audit::RequestMeta,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<axum::response::Response>> + Send + 'a>>;
}

fn pdf_slot() -> &'static RwLock<Option<Arc<dyn PdfOps>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn PdfOps>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Registers the PDF implementation (W5 calls this at startup).
pub fn set_pdf_ops(p: Arc<dyn PdfOps>) {
    if let Ok(mut g) = pdf_slot().write() {
        *g = Some(p);
    }
}

pub fn pdf_ops() -> Option<Arc<dyn PdfOps>> {
    pdf_slot().read().ok().and_then(|g| g.clone())
}

/// Receiver of `document_updated` outside this crate (after `run_workflows_updated`, like the
/// order in `documents/apps.py`). The implementation logs errors itself (upstream: signal
/// receivers do not raise into the request).
pub trait DocumentUpdatedHook: Send + Sync {
    fn document_updated<'a>(
        &'a self,
        state: &'a pl_core::AppState,
        document_id: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

fn updated_slot() -> &'static RwLock<Option<Arc<dyn DocumentUpdatedHook>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn DocumentUpdatedHook>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Registers the receiver (pl-server at startup, e.g. LLM index of pl-ai).
pub fn set_document_updated_hook(h: Arc<dyn DocumentUpdatedHook>) {
    if let Ok(mut g) = updated_slot().write() {
        *g = Some(h);
    }
}

/// `document_updated.send(document=…)` to the registered receiver.
pub async fn document_updated(state: &pl_core::AppState, document_id: i64) {
    let hook = updated_slot().read().ok().and_then(|g| g.clone());
    if let Some(h) = hook {
        h.document_updated(state, document_id).await;
    }
}

/// `send_websocket_document_updated(document)` (W3): `refresh_from_db()`, owner and direct
/// `view` grantees, `modified` like `DRF_DATETIME_FIELD.to_representation`. Without an EventBus
/// (tests, operation without WebSocket) nothing. Errors only logged (signal receiver).
pub async fn send_websocket_document_updated(state: &pl_core::AppState, conn: &mut sqlx::PgConnection, document_id: i64) {
    let Some(bus) = state.service::<pl_queue::EventBus>() else { return };
    let r: Result<(), sqlx::Error> = async {
        let Some(d) = pl_consumer::db::load_document(conn, document_id).await? else { return Ok(()) };
        let (users, groups) = pl_consumer::signals::view_grantees(conn, document_id).await?;
        let modified = pl_core::datetime::format_datetime(&d.modified, state.tz());
        pl_queue::events::DocumentsStatusManager::new(bus).send_document_updated(
            document_id,
            modified,
            pl_queue::events::ViewPermissions { owner_id: d.owner_id, users_can_view: users, groups_can_view: groups },
        );
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::warn!(target: "paperless.handlers", "document_updated websocket message for {document_id} failed: {e}");
    }
}
