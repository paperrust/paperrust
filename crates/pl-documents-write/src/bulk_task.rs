// upstream idea: paperless-ngx dev 60a35edee (#14711, "make bulk metadata operations run async"),
//                src/documents/bulk_operations.py (execute_bulk_operation), views.py
//                (BulkEditView._queue_bulk_operation). Own implementation, no code taken over.
//
//! Large bulk metadata changes in the background.
//!
//! `POST /api/documents/bulk_edit/` normally changes the metadata immediately within the request (as
//! v3.4.2) and only enqueues the follow-up work (`bulk_update_documents`). For large selections even
//! the writing takes long (every row change to `documents_document` updates all
//! indexes); the request would then hang for seconds to minutes. Therefore:
//!
//! * Selection with at least [`async_min_docs`] documents (default 500,
//!   `PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS`, `0` = always immediately) **and** a method from
//!   [`Method::queueable`] **and** PaperRust queue active → enqueue task
//!   [`TASK_NAME`], response `{"result": "OK", "task_id": "<uuid>"}`.
//! * otherwise immediately as before, response `{"result": "OK"}` (old UIs reload the list
//!   afterwards and see the change immediately).
//!
//! The task checks user (active?), permissions and parameters **again** (both can change between
//! enqueueing and execution) and then runs the same path as the request:
//! snapshot old → method → audit entries → commit → enqueue follow-up work.

use serde_json::{Map, Value, json};
use uuid::Uuid;

use pl_core::AppState;
use pl_core::tasks::TaskRequest;

use crate::audit::RequestMeta;
use crate::bulk_edit::{self, Method, Validated};
use crate::error::ApiError;
use crate::perms::has_document_permissions;

/// Name of the task (same as upstream `dev`, so that a later Django version knows it).
pub const TASK_NAME: &str = "documents.bulk_operations.execute_bulk_operation";

/// Default threshold for the background path.
pub const DEFAULT_ASYNC_MIN_DOCS: usize = 500;

/// From how many documents a metadata change runs in the background (`0` = never).
pub fn async_min_docs(lookup: impl Fn(&str) -> Option<String>) -> usize {
    parse_min_docs(lookup("PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS").as_deref())
}

fn parse_min_docs(v: Option<&str>) -> usize {
    match v.map(str::trim) {
        None | Some("") => DEFAULT_ASYNC_MIN_DOCS,
        Some(s) => s.parse().unwrap_or(DEFAULT_ASYNC_MIN_DOCS),
    }
}

/// Should this change run in the background?
pub fn should_queue(method: Method, doc_count: usize, min_docs: usize, has_pg_queue: bool, kwargs_ok: bool) -> bool {
    has_pg_queue && kwargs_ok && method.queueable() && min_docs > 0 && doc_count >= min_docs
}

/// Enqueue task (kwargs as upstream: `operation`, `document_ids`, `parameters`, `user_id`;
/// PaperRust additionally `audit_meta` and `actor`, so that the audit entries keep address/cid and the
/// actor as in the request).
#[allow(clippy::too_many_arguments)]
pub async fn enqueue(
    state: &AppState,
    user_id: i64,
    session_actor: bool,
    meta: &RequestMeta,
    ids: &[i64],
    method: Method,
    raw_parameters: &Map<String, Value>,
    from_webui: bool,
) -> Result<Uuid, ApiError> {
    let source = if from_webui {
        pl_core::tasks::trigger_source::WEB_UI
    } else {
        pl_core::tasks::trigger_source::API_UPLOAD
    };
    let mut req = TaskRequest::new(TASK_NAME).trigger_source(source);
    req.kwargs = kwargs(user_id, session_actor, meta, ids, method, raw_parameters);
    Ok(pl_core::tasks::send_task(state, req).await?)
}

fn kwargs(
    user_id: i64,
    session_actor: bool,
    meta: &RequestMeta,
    ids: &[i64],
    method: Method,
    raw_parameters: &Map<String, Value>,
) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("operation".into(), Value::String(method.name().into()));
    m.insert("document_ids".into(), json!(ids));
    m.insert("parameters".into(), Value::Object(raw_parameters.clone()));
    m.insert("user_id".into(), json!(user_id));
    m.insert("actor".into(), Value::Bool(session_actor));
    m.insert(
        "audit_meta".into(),
        json!({"remote_addr": meta.remote_addr, "remote_port": meta.remote_port, "cid": meta.cid}),
    );
    m
}

/// Task error: `(exception type, text)` for `TaskError::fail`.
pub type TaskFailure = (String, String);

fn fail(t: &str, m: impl Into<String>) -> TaskFailure {
    (t.to_string(), m.into())
}

/// Execute task. Result `{"operation": …, "document_count": n}`.
///
/// If it fails, the task ends with a failure status (`PaperlessTask` FAILURE with reason, via
/// pl-queue) **and** the triggering user gets a message via the WebSocket
/// (`status_update` FAILED, visible like a failed upload) – the request had already answered
/// 200 (FINDINGS B5: the background path used to fail silently).
pub async fn execute(state: &AppState, kwargs: &Map<String, Value>) -> Result<Value, TaskFailure> {
    let r = execute_inner(state, kwargs).await;
    if let Err((kind, message)) = &r {
        let operation = kwargs.get("operation").and_then(Value::as_str).unwrap_or("");
        let count = kwargs.get("document_ids").and_then(Value::as_array).map_or(0, Vec::len);
        tracing::error!(target: "paperless.bulk_edit", operation, documents = count, "execute_bulk_operation failed: {kind}: {message}");
        notify_failure(state, kwargs, operation, count);
    }
    r
}

/// Error message to the trigger (only they and superusers see it: `owner_id` = user). Text
/// without internals – details are in the task and in the log.
fn notify_failure(state: &AppState, kwargs: &Map<String, Value>, operation: &str, count: usize) {
    let Some(bus) = state.service::<pl_queue::EventBus>() else { return };
    let Some(user_id) = kwargs.get("user_id").and_then(Value::as_i64) else { return };
    let label = format!("Bulk edit: {operation} ({count} documents)");
    pl_queue::events::ProgressManager::new(bus, Some(label), None).send_progress(
        pl_queue::events::ProgressStatus::Failed,
        "Bulk edit failed, check the task list or logs for details.",
        100,
        100,
        None,
        pl_queue::events::ViewPermissions { owner_id: Some(user_id), users_can_view: Vec::new(), groups_can_view: Vec::new() },
    );
}

async fn execute_inner(state: &AppState, kwargs: &Map<String, Value>) -> Result<Value, TaskFailure> {
    let operation = kwargs.get("operation").and_then(Value::as_str).unwrap_or("");
    let method = Method::from_choice(operation)
        .filter(|m| m.queueable())
        .ok_or_else(|| fail("ValueError", format!("Unsupported bulk operation: {operation}")))?;
    let ids: Vec<i64> = match kwargs.get("document_ids") {
        Some(Value::Array(a)) => a.iter().map(Value::as_i64).collect::<Option<_>>().ok_or_else(|| fail("TypeError", "document_ids must be a list of integers"))?,
        _ => return Err(fail("TypeError", "execute_bulk_operation() missing 'document_ids'")),
    };
    let mut parameters = match kwargs.get("parameters") {
        Some(Value::Object(m)) => m.clone(),
        None | Some(Value::Null) => Map::new(),
        Some(_) => return Err(fail("TypeError", "parameters must be a dict")),
    };
    let user_id = kwargs
        .get("user_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| fail("TypeError", "execute_bulk_operation() missing 'user_id'"))?;
    let session_actor = kwargs.get("actor").and_then(Value::as_bool).unwrap_or(true);
    let am = kwargs.get("audit_meta");
    let meta = RequestMeta {
        remote_addr: am.and_then(|m| m.get("remote_addr")).and_then(Value::as_str).map(String::from),
        remote_port: am.and_then(|m| m.get("remote_port")).and_then(Value::as_i64).and_then(|p| i32::try_from(p).ok()),
        cid: am.and_then(|m| m.get("cid")).and_then(Value::as_str).map(String::from),
    };

    let db_err = |e: sqlx::Error| fail("OperationalError", e.to_string());
    let user = pl_authz::user::load_user_by_id(state.db(), user_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| fail("DoesNotExist", "User matching query does not exist."))?;
    let denied = || fail("PermissionDenied", "Insufficient permissions to execute bulk operation");
    if !user.is_active {
        return Err(denied());
    }
    let mut conn = state.db().acquire().await.map_err(db_err)?;
    let api = |e: ApiError| fail("OperationalError", format!("{e:?}"));
    if !has_document_permissions(&mut conn, &user, &ids, method.needs()).await.map_err(api)? {
        return Err(denied());
    }
    let mut call_kwargs = parameters.clone();
    if method == Method::SetPermissions {
        call_kwargs.entry("merge").or_insert(Value::Bool(false));
    }
    let params = match bulk_edit::validate_parameters(&mut conn, &user, method, &mut parameters).await.map_err(api)? {
        Validated::Ok(p) => p,
        Validated::Invalid(v) => return Err(fail("ValidationError", v.to_string())),
        Validated::Forbidden(d) => return Err(fail("PermissionDenied", d)),
    };
    let kwargs_ok = bulk_edit::kwargs_accepted(method, &call_kwargs);
    drop(conn);
    crate::views::run_method(state, &user, session_actor, &meta, &ids, method, &params, kwargs_ok, true)
        .await
        .map_err(|e| fail("Exception", e))?;
    Ok(json!({"operation": method.name(), "document_count": ids.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_parsing() {
        assert_eq!(parse_min_docs(None), 500);
        assert_eq!(parse_min_docs(Some(" ")), 500);
        assert_eq!(parse_min_docs(Some("0")), 0);
        assert_eq!(parse_min_docs(Some("1200")), 1200);
        assert_eq!(parse_min_docs(Some("viele")), 500);
    }

    #[test]
    fn queue_decision() {
        assert!(should_queue(Method::AddTag, 500, 500, true, true));
        assert!(!should_queue(Method::AddTag, 499, 500, true, true));
        // without the PaperRust queue the Django worker v3.4.2 does not know the task
        assert!(!should_queue(Method::AddTag, 5000, 500, false, true));
        // wrong keyword arguments → the immediate path returns the 400 error
        assert!(!should_queue(Method::AddTag, 5000, 500, true, false));
        // 0 = abgeschaltet
        assert!(!should_queue(Method::AddTag, 5000, 0, true, true));
        // Delete/reprocess stay as before
        assert!(!should_queue(Method::Delete, 5000, 500, true, true));
        assert!(!should_queue(Method::Reprocess, 5000, 500, true, true));
        for m in [
            Method::SetCorrespondent,
            Method::SetDocumentType,
            Method::SetStoragePath,
            Method::RemoveTag,
            Method::ModifyTags,
            Method::ModifyCustomFields,
            Method::SetPermissions,
        ] {
            assert!(should_queue(m, 500, 500, true, true), "{m:?}");
        }
    }

    #[test]
    fn kwargs_shape() {
        let meta = RequestMeta { remote_addr: Some("127.0.0.1".into()), remote_port: Some(5), cid: None };
        let mut p = Map::new();
        p.insert("tag".into(), json!(3));
        let k = kwargs(7, true, &meta, &[1, 2], Method::AddTag, &p);
        let mut keys: Vec<&str> = k.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["actor", "audit_meta", "document_ids", "operation", "parameters", "user_id"]);
        assert_eq!(k["operation"], "add_tag");
        assert_eq!(k["document_ids"], json!([1, 2]));
        assert_eq!(k["user_id"], 7);
    }
}
