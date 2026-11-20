// upstream: src/documents/views.py@v3.4.2 (BulkEditView.post, ReprocessDocumentsView.post,
//           DocumentOperationPermissionMixin._execute_document_action)
//           rest_framework/request.py (Request._parse), rest_framework/parsers.py (JSONParser)
//
//! HTTP handlers for bulk_edit and reprocess.

use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::{HeaderMap, header, request::Parts};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use pl_core::AppState;

use crate::audit::{self, RequestMeta};
use crate::bulk_edit::{self, METHOD_CHOICES, Method, Params, Validated};
use crate::enqueue::{self, Task};
use crate::drf::{self, Errors};
use crate::error::{ApiError, ApiResult};
use crate::perms::{OperationNeeds, has_document_permissions};
use crate::selection;
use crate::user::CurrentUser;

/// Logged-in user (`IsAuthenticated`) from pl-authz; second field = the middleware sets the
/// auditlog actor ([`crate::user::middleware_sets_actor`]).
pub struct Authed(pub CurrentUser, pub bool);

impl<S: Send + Sync> FromRequestParts<S> for Authed {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match pl_authz::AuthUser::from_request_parts(parts, state).await {
            Ok(u) => {
                // auditlog actor like AuditlogMiddleware (session/AutoLogin/RemoteUser)
                let actor = crate::user::middleware_sets_actor(u.context().method);
                Ok(Authed(u.user().clone(), actor))
            }
            Err(e) => Err(e.into_response()),
        }
    }
}

/// Peer address (`REMOTE_ADDR`), if the server provides `ConnectInfo`.
pub struct Peer(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Peer(parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0)))
    }
}

/// `request.data` with `parser_classes = (JSONParser,)`.
/// DRF `media_type_matches("application/json", content_type)` (`_MediaType.match`): wildcards
/// in the client type match (`application/*`, `*/*`), Review 1 N2. Same rule as pl-files.
pub fn is_json_content_type(ct: &str) -> bool {
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let (t, s) = main.split_once('/').unwrap_or((main.as_str(), ""));
    (t == "application" || t == "*") && (s == "json" || s == "*")
}

pub fn parse_json_body(headers: &HeaderMap, body: &Bytes) -> ApiResult<Value> {
    if body.is_empty() {
        // Request._parse: no stream → empty dict
        return Ok(Value::Object(Map::new()));
    }
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !is_json_content_type(ct) {
        return Err(pl_core::ApiError::unsupported_media_type(ct).into());
    }
    serde_json::from_slice(body).map_err(|e| pl_core::ApiError::json_body_parse_error(body, e).into())
}

/// `Serializer.is_valid` for a non-dict body. DRF `Serializer.errors` rewrites a
/// single `null` error to `No data provided` (review 1 N3).
pub fn not_a_dict_message(data: &Value) -> String {
    if data.is_null() {
        "No data provided".to_string()
    } else {
        format!("Invalid data. Expected a dictionary, but got {}.", drf::py_type_name(data))
    }
}

/// Error of one attempt of [`run_method_once`]: text for the log and whether a retry of the
/// whole transaction is permissible (deadlock/serialization failure, FINDINGS B4).
struct Attempt {
    message: String,
    retryable: bool,
}

impl From<ApiError> for Attempt {
    fn from(e: ApiError) -> Self {
        Attempt { retryable: e.is_retryable(), message: format!("{e:?}") }
    }
}

impl From<sqlx::Error> for Attempt {
    fn from(e: sqlx::Error) -> Self {
        Attempt { retryable: pl_consumer::db::is_retryable(&e), message: e.to_string() }
    }
}

impl From<pl_core::ApiError> for Attempt {
    fn from(e: pl_core::ApiError) -> Self {
        Attempt { retryable: false, message: e.to_string() }
    }
}

/// Execution in a transaction: snapshot old → method → snapshot new +
/// LogEntries → PaperlessTask rows → commit → enqueue tasks.
///
/// PaperRust: if PostgreSQL aborts the transaction because of a deadlock or a
/// serialization failure (`40P01`/`40001`), it is fully retried up to
/// [`pl_consumer::db::MAX_ATTEMPTS`] times (until the commit it has no
/// external effect). The document rows are locked beforehand in id order
/// ([`bulk_edit::lock_documents`]); so a concurrent change to the same documents waits,
/// and the "old" snapshot shows the committed state (FINDINGS B4/B5).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_method(
    state: &AppState,
    user: &CurrentUser,
    session: bool,
    meta: &RequestMeta,
    ids: &[i64],
    method: Method,
    params: &Params,
    kwargs_ok: bool,
    with_audit: bool,
) -> Result<(), String> {
    let mut attempt = 1;
    loop {
        match run_method_once(state, user, session, meta, ids, method, params, kwargs_ok, with_audit).await {
            Ok(()) => return Ok(()),
            Err(e) if e.retryable && attempt < pl_consumer::db::MAX_ATTEMPTS => {
                tracing::warn!(target: "paperless.bulk_edit", attempt, method = method.name(), "bulk edit retried after: {}", e.message);
                tokio::time::sleep(pl_consumer::db::retry_backoff(attempt)).await;
                attempt += 1;
            }
            Err(e) => return Err(e.message),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_method_once(
    state: &AppState,
    user: &CurrentUser,
    session: bool,
    meta: &RequestMeta,
    ids: &[i64],
    method: Method,
    params: &Params,
    kwargs_ok: bool,
    with_audit: bool,
) -> Result<(), Attempt> {
    let mut tx = state.inner.db.begin().await?;
    if kwargs_ok {
        bulk_edit::lock_documents(&mut tx, ids, params).await?;
    }
    let snap = if with_audit && state.config().audit_log_enabled { method.snapshot() } else { None };
    let old = match snap {
        Some(s) => Some(audit::snapshot(&mut tx, ids, s).await?),
        None => None,
    };
    if !kwargs_ok {
        // method(documents, **parameters) → TypeError: unexpected keyword argument
        return Err(Attempt { message: "unexpected keyword argument".into(), retryable: false });
    }
    let ex = bulk_edit::ExecCtx {
        log: crate::auditlog::LogCtx::middleware(
            state.config().audit_log_enabled,
            if session { Some((&user.id, Some(user.email.as_str()))) } else { None },
            meta.remote_addr.clone(),
            meta.remote_port,
            meta.cid.clone(),
        ),
        media: crate::files::MediaCfg::from_config(&state.live_config()),
    };
    let tasks: Vec<Task> = bulk_edit::execute(&mut tx, ids, params, &ex).await?;
    if let (Some(s), Some(old)) = (snap, old) {
        let new = audit::snapshot(&mut tx, ids, s).await?;
        audit::log_bulk_edit(&mut tx, user, meta, ids, s.field_name(), &old, &new, method.name()).await?;
    }
    // pl_core::tasks: fix the task id (without DB); PaperlessTask in this transaction only if
    // the dispatcher does not create it itself; enqueue after the commit. `bulk_update_documents`
    // attaches – where possible – to a still waiting identical task (crate::coalesce, B6);
    // as the last step before the commit, so that the queue row is locked only briefly.
    let mut prepared = Vec::with_capacity(tasks.len());
    for t in &tasks {
        if t.name == crate::enqueue::BULK_UPDATE_DOCUMENTS
            && let Some(doc_ids) = crate::coalesce::document_ids(&t.kwargs)
            && crate::coalesce::try_merge(&mut tx, &doc_ids).await?.is_some()
        {
            continue;
        }
        let p = pl_core::tasks::prepare_task(state, t.request())?;
        if p.records_in_caller() {
            enqueue::record(&mut tx, t).await?;
        }
        prepared.push(p);
    }
    tx.commit().await?;
    for p in prepared {
        // after the commit: do not retry an enqueue error (the change is saved)
        p.publish(state).await.map_err(|e| Attempt { message: e.to_string(), retryable: false })?;
    }
    Ok(())
}

/// bulk_edit methods that [`crate::hooks::PdfOps`] provides ([`crate::pdf_edit::NativePdfOps`]).
pub const PDF_METHODS: &[&str] = &["rotate", "merge", "split", "delete_pages", "edit_pdf", "remove_password"];

pub(crate) fn ok_result() -> Response {
    pl_core::json::drf_response(axum::http::StatusCode::OK, &json!({"result": "OK"}))
}

/// Response of the background path (upstream dev: `BulkEditDocumentsResult.task_id`, optional).
fn queued_result(task_id: uuid::Uuid) -> Response {
    pl_core::json::drf_response(axum::http::StatusCode::OK, &json!({"result": "OK", "task_id": task_id.to_string()}))
}

/// Fields of `BulkEditSerializer` (with a valid `method`) and `DocumentSelectionSerializer.validate`.
pub(crate) struct BulkFields {
    pub selection: selection::Selection,
    pub parameters: Map<String, Value>,
    pub from_webui: bool,
}

/// Field validation `documents, all, filters, excluded_documents, parameters, from_webui`
/// (errors collected in field order), then the selection validation.
///
/// `method_error`: error of the `method` field (`ChoiceField`, declared after the selection fields
/// and before `parameters`), so that it lands in DRF's field order next to the other field errors.
pub(crate) async fn parse_bulk_fields(
    conn: &mut sqlx::PgConnection,
    data: &Map<String, Value>,
    method_error: Option<drf::FieldError>,
) -> ApiResult<BulkFields> {
    let mut errors = Errors::default();
    let mut sel = selection::parse_fields(conn, data, false, true, &mut errors).await?;
    if let Some(e) = method_error {
        errors.add("method", e);
    }
    let parameters = match data.get("parameters") {
        None => Map::new(),
        Some(v) => match drf::dict(v) {
            Ok(m) => m,
            Err(e) => {
                errors.add("parameters", e);
                Map::new()
            }
        },
    };
    let mut from_webui = false;
    if let Some(v) = data.get("from_webui") {
        match drf::boolean(v) {
            Ok(b) => from_webui = b,
            Err(e) => errors.add("from_webui", e),
        }
    }
    if !errors.is_empty() {
        return Err(ApiError::Validation(Value::Object(errors.0)));
    }
    // validate()
    if let Err(m) = selection::validate(conn, &mut sel).await? {
        return Err(ApiError::Validation(drf::non_field(m)));
    }
    Ok(BulkFields { selection: sel, parameters, from_webui })
}

/// `BulkEditSerializer.method` (`ChoiceField(choices=METHOD_CHOICES)`, required, not nullable):
/// the valid choice or the field error like DRF (`Field.validate_empty_values`,
/// `ChoiceField.to_internal_value`: lookup by `str(data)`).
pub(crate) fn method_choice(v: Option<&Value>) -> Result<&str, drf::FieldError> {
    match v {
        None => Err(drf::msg(drf::REQUIRED)),
        Some(Value::Null) => Err(drf::msg(drf::NULL)),
        Some(Value::String(s)) if METHOD_CHOICES.contains(&s.as_str()) => Ok(s.as_str()),
        Some(other) => Err(drf::msg(format!("\"{}\" is not a valid choice.", drf::py_str(other)))),
    }
}

/// `{"non_field_errors": ["Invalid data. Expected a dictionary, but got <typ>."]}` (DRF-Wortlaut).
fn not_a_dict(v: &Value) -> ApiError {
    ApiError::Validation(drf::non_field(not_a_dict_message(v)))
}

/// POST /api/documents/bulk_edit/
pub async fn bulk_edit_handler(
    State(state): State<AppState>,
    Authed(user, session): Authed,
    Peer(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match bulk_edit_inner(&state, &user, session, &headers, peer, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn bulk_edit_inner(
    state: &AppState,
    user: &CurrentUser,
    session: bool,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let data = parse_json_body(headers, body)?;
    // upstream: `request.data.get("method")` – not a dict → AttributeError → 500. PaperRust: 400 like
    // DRF for non-objects (B-313, Known differences).
    let data = match data {
        Value::Object(m) => m,
        other => return Err(not_a_dict(&other)),
    };
    let method = match method_choice(data.get("method")) {
        Ok(name) => match Method::from_choice(name) {
            Some(m) => m,
            None => {
                // PDF methods: implementation of W5 (pl-pdfops + PaperRust queue, registered by
                // pl-server). Without it the server is misconfigured: 500 (no silent 404).
                if PDF_METHODS.contains(&name)
                    && let Some(ops) = crate::hooks::pdf_ops()
                {
                    let meta = RequestMeta::from_request(headers, peer);
                    if let Some(r) = ops.bulk_edit(state, user, name, &data, &meta).await {
                        return Ok(r);
                    }
                }
                return Err(ApiError::Uncaught(format!("bulk_edit method {name:?}: PDF backend or task queue not available")));
            }
        },
        // `method` missing/null/not a choice: 400 with every field error in declaration order.
        Err(e) => {
            let mut conn = state.inner.db.acquire().await?;
            parse_bulk_fields(&mut conn, &data, Some(e)).await?;
            return Err(ApiError::Uncaught("method error not reported".into()));
        }
    };
    debug_assert!(METHOD_CHOICES.contains(&method.name()));

    let mut conn = state.inner.db.acquire().await?;
    let BulkFields { selection: sel, mut parameters, from_webui } = parse_bulk_fields(&mut conn, &data, None).await?;
    // for the background path: the task validates the parameters again (passed on unchanged)
    let raw_parameters = parameters.clone();
    let params = match bulk_edit::validate_parameters(&mut conn, user, method, &mut parameters).await? {
        Validated::Ok(p) => p,
        Validated::Invalid(v) => return Err(ApiError::Validation(v)),
        Validated::Forbidden(d) => return Err(ApiError::Core(pl_core::ApiError::permission_denied_msg(d))),
    };
    // kwargs: default values added in validate (merge, remote_ocr) count as passed
    let mut call_kwargs = parameters.clone();
    match method {
        Method::SetPermissions => {
            call_kwargs.entry("merge").or_insert(Value::Bool(false));
        }
        Method::Reprocess => {
            call_kwargs.entry("remote_ocr").or_insert(Value::Bool(false));
        }
        Method::RunWorkflow => {
            call_kwargs.entry("filter_documents").or_insert(Value::Bool(true));
        }
        _ => {}
    }
    let kwargs_ok = bulk_edit::kwargs_accepted(method, &call_kwargs);

    let (mut conn, ids) = selection::resolve(state, conn, user, &sel).await?;
    if !has_document_permissions(&mut conn, user, &ids, method.needs()).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);

    let meta = RequestMeta::from_request(headers, peer);
    // PaperRust (upstream dev 60a35edee, own implementation): large metadata changes in the
    // background, small ones still immediately (crate::bulk_task).
    let min_docs = crate::bulk_task::async_min_docs(pl_core::config::var);
    let has_pg_queue = pl_core::tasks::dispatcher(state).is_some();
    if crate::bulk_task::should_queue(method, ids.len(), min_docs, has_pg_queue, kwargs_ok) {
        return match crate::bulk_task::enqueue(state, user.id, session, &meta, &ids, method, &raw_parameters, from_webui).await {
            Ok(task_id) => Ok(queued_result(task_id)),
            Err(e) => {
                tracing::warn!("An error occurred performing bulk edit: {e:?}");
                Err(ApiError::BadRequestText("Error performing bulk edit, check logs for more detail.".into()))
            }
        };
    }
    match run_method(state, user, session, &meta, &ids, method, &params, kwargs_ok, true).await {
        Ok(()) => Ok(ok_result()),
        // tests/chaos C1: database gone → 503 "unreachable" instead of 400 "check logs" (nothing changed,
        // the transaction is rolled back; the user can retry right away)
        Err(e) if pl_consumer::consumer::is_db_unavailable_text(&e) => {
            tracing::warn!("Database not reachable while performing bulk edit: {e}");
            pl_core::dbconn::mark_down();
            Err(ApiError::Core(pl_core::ApiError::db_unavailable()))
        }
        Err(e) => {
            tracing::warn!("An error occurred performing bulk edit: {e}");
            Err(ApiError::BadRequestText("Error performing bulk edit, check logs for more detail.".into()))
        }
    }
}

/// POST /api/documents/reprocess/
pub async fn reprocess_handler(
    State(state): State<AppState>,
    Authed(user, session): Authed,
    Peer(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match reprocess_inner(&state, &user, session, &headers, peer, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn reprocess_inner(
    state: &AppState,
    user: &CurrentUser,
    session: bool,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let data = parse_json_body(headers, body)?;
    let Value::Object(data) = data else {
        return Err(ApiError::Validation(drf::non_field(not_a_dict_message(&data))));
    };
    let mut conn = state.inner.db.acquire().await?;
    let mut errors = Errors::default();
    let mut sel = selection::parse_fields(&mut conn, &data, false, true, &mut errors).await?;
    let mut remote_ocr = false;
    if let Some(v) = data.get("remote_ocr") {
        match drf::boolean(v) {
            Ok(b) => remote_ocr = b,
            Err(e) => errors.add("remote_ocr", e),
        }
    }
    // PaperRust: AI text recognition when reprocessing
    let mut ai_ocr = false;
    if let Some(v) = data.get("ai_ocr") {
        match drf::boolean(v) {
            Ok(b) => ai_ocr = b,
            Err(e) => errors.add("ai_ocr", e),
        }
    }
    if !errors.is_empty() {
        return Err(ApiError::Validation(Value::Object(errors.0)));
    }
    if let Err(m) = selection::validate(&mut conn, &mut sel).await? {
        return Err(ApiError::Validation(drf::non_field(m)));
    }
    let (mut conn, ids) = selection::resolve(state, conn, user, &sel).await?;
    if !has_document_permissions(&mut conn, user, &ids, OperationNeeds::default()).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);
    let meta = RequestMeta::from_request(headers, peer);
    let params = Params::Reprocess { remote_ocr, ai_ocr };
    match run_method(state, user, session, &meta, &ids, Method::Reprocess, &params, true, false).await {
        Ok(()) => Ok(ok_result()),
        Err(e) => {
            tracing::warn!("An error occurred performing document reprocess: {e}");
            Err(ApiError::BadRequestText(
                "Error performing document reprocess, check logs for more detail.".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B-313: bulk_edit with a list/number as body → 400 instead of 500.
    #[test]
    fn bulk_edit_non_dict_is_400() {
        let r = not_a_dict(&json!([])).into_response();
        assert_eq!(r.status(), axum::http::StatusCode::BAD_REQUEST);
        let ApiError::Validation(v) = not_a_dict(&json!(5)) else { panic!("validation") };
        assert_eq!(v, json!({"non_field_errors": ["Invalid data. Expected a dictionary, but got int."]}));
    }

    /// Review 1 N2: DRF accepts wildcards in the client's content type.
    #[test]
    fn json_content_type_wildcards() {
        assert!(is_json_content_type("application/json; charset=utf-8"));
        assert!(is_json_content_type("application/*"));
        assert!(is_json_content_type("*/*"));
        assert!(is_json_content_type("Application/JSON"));
        assert!(!is_json_content_type("text/plain"));
        assert!(!is_json_content_type("application/xml"));
    }

    /// Review 1 N3: `null`-Body → `No data provided`.
    #[test]
    fn null_body_message() {
        assert_eq!(not_a_dict_message(&Value::Null), "No data provided");
        assert_eq!(not_a_dict_message(&serde_json::json!([1])), "Invalid data. Expected a dictionary, but got list.");
    }
}

#[cfg(test)]
mod method_choice_tests {
    use super::*;
    use serde_json::json;

    /// BulkEditSerializer.method: DRF's ChoiceField texts (parity run 2026-11-20, F1).
    #[test]
    fn method_errors_like_drf() {
        assert_eq!(method_choice(Some(&json!("add_tag"))), Ok("add_tag"));
        assert_eq!(method_choice(Some(&json!("rotate"))), Ok("rotate"));
        assert_eq!(method_choice(None), Err(json!(["This field is required."])));
        assert_eq!(method_choice(Some(&Value::Null)), Err(json!(["This field may not be null."])));
        assert_eq!(method_choice(Some(&json!("nope"))), Err(json!(["\"nope\" is not a valid choice."])));
        assert_eq!(method_choice(Some(&json!(""))), Err(json!(["\"\" is not a valid choice."])));
        assert_eq!(method_choice(Some(&json!(5))), Err(json!(["\"5\" is not a valid choice."])));
        assert_eq!(method_choice(Some(&json!(true))), Err(json!(["\"True\" is not a valid choice."])));
        assert_eq!(method_choice(Some(&json!(["add_tag"]))), Err(json!(["\"['add_tag']\" is not a valid choice."])));
    }
}
