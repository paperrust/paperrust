// upstream: src/documents/views.py@v3.4.2 (DocumentOperationPermissionMixin._has_document_permissions/
//           _execute_document_action, DeleteDocumentsView, RotateDocumentsView, MergeDocumentsView,
//           MergeDocumentsAsVersionsView, EditPdfDocumentsView, RemovePasswordDocumentsView),
//           src/documents/serialisers.py@v3.4.2 (DocumentListSerializer, DocumentSelectionSerializer,
//           SourceModeValidationMixin, RotateDocumentsSerializer, MergeDocumentsSerializer,
//           MergeDocumentsAsVersionsSerializer, EditPdfDocumentsSerializer, RemovePasswordDocumentsSerializer,
//           DeleteDocumentsSerializer), src/documents/bulk_edit.py@v3.4.2 (delete, merge_as_versions),
//           rest_framework/fields.py (CharField, IntegerField, BooleanField)
//
//! Own document action routes `POST /api/documents/{delete,rotate,merge,merge_as_versions,
//! edit_pdf,remove_password}/`. Validation via the respective serializers (field errors under the
//! field name, not as `non_field_errors` like in `bulk_edit`), execution via the existing
//! building blocks: `delete` = [`crate::tasks::bulk_delete_logged`], PDF methods = [`pdf_edit::Exec`],
//! `merge_as_versions` = [`merge_as_versions`] (here, no counterpart in bulk_edit).

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use pl_core::AppState;
use pl_pdfops::bulk::SourceMode;

use crate::audit::RequestMeta;
use crate::auditlog::{self, LogCtx};
use crate::docs;
use crate::drf::{self, Errors};
use crate::error::{ApiError, ApiResult};
use crate::files::MediaCfg;
use crate::pdf_edit::{self, Exec, PdfParams};
use crate::perms::{OperationNeeds, has_document_permissions};
use crate::selection::{self, Selection};
use crate::user::CurrentUser;
use crate::views::{Authed, Peer, not_a_dict_message, ok_result, parse_json_body};

const LOG: &str = "paperless.bulk_edit";

// ------------------------------------------------------------------ DRF fields

/// `CharField(allow_blank=…, max_length=…)` with `trim_whitespace=True` for a present,
/// non-null value. `Ok(s)` = cleaned value.
pub fn char_field(v: &Value, allow_blank: bool, max_length: Option<usize>) -> Result<String, Value> {
    // run_validation: '' or whitespace only → blank
    let raw = match v {
        Value::Null => return Err(drf::msg(drf::NULL)),
        Value::String(s) => s.clone(),
        Value::Number(_) => drf::py_str(v),
        // bool / list / dict: to_internal_value → invalid
        _ => {
            return Err(drf::msg("Not a valid string."));
        }
    };
    let s = raw.trim().to_string();
    if s.is_empty() {
        return if allow_blank { Ok(String::new()) } else { Err(drf::msg("This field may not be blank.")) };
    }
    // run_validators: collect all messages (MaxLength, ProhibitNull, ProhibitSurrogate)
    let mut msgs: Vec<Value> = Vec::new();
    if let Some(max) = max_length
        && s.chars().count() > max
    {
        msgs.push(Value::String(format!("Ensure this field has no more than {max} characters.")));
    }
    if s.contains('\0') {
        msgs.push(Value::String("Null characters are not allowed.".into()));
    }
    if msgs.is_empty() { Ok(s) } else { Err(Value::Array(msgs)) }
}

/// `IntegerField(required=…, allow_null=…)`. `Ok(None)` = missing (not required) or null
/// (with `allow_null`).
fn int_field(data: &Map<String, Value>, key: &str, required: bool, allow_null: bool, errors: &mut Errors) -> Option<Option<i64>> {
    match data.get(key) {
        None if required => {
            errors.add(key, drf::msg(drf::REQUIRED));
            None
        }
        None => Some(None),
        Some(Value::Null) if allow_null => Some(None),
        Some(Value::Null) => {
            errors.add(key, drf::msg(drf::NULL));
            None
        }
        Some(v) => match drf::drf_int(v) {
            Ok(i) => Some(Some(i)),
            Err(e) => {
                errors.add(key, drf::msg(e));
                None
            }
        },
    }
}

/// `BooleanField(required=False, default=…)`.
fn bool_field(data: &Map<String, Value>, key: &str, default: bool, errors: &mut Errors) -> bool {
    match data.get(key) {
        None => default,
        Some(v) => match drf::boolean(v) {
            Ok(b) => b,
            Err(e) => {
                errors.add(key, e);
                default
            }
        },
    }
}

/// `source_mode = CharField(required=False, default=LATEST_VERSION)` + `validate_source_mode`.
fn source_mode_field(data: &Map<String, Value>, errors: &mut Errors) -> SourceMode {
    let Some(v) = data.get("source_mode") else { return SourceMode::LatestVersion };
    match char_field(v, false, None) {
        Err(e) => {
            errors.add("source_mode", e);
            SourceMode::LatestVersion
        }
        Ok(s) => match pdf_edit::parse_source_mode(Some(&Value::String(s))) {
            Some(m) => m,
            None => {
                errors.add("source_mode", drf::msg("Invalid source_mode"));
                SourceMode::LatestVersion
            }
        },
    }
}

/// Rotationswinkel (`_validate_rotation_degrees`).
fn multiple_of_90(d: i64, field: &str) -> Result<i64, Value> {
    if d % 90 != 0 { Err(drf::msg(format!("{field} must be a multiple of 90"))) } else { Ok(d) }
}

// ------------------------------------------------------------------ shared flow

/// Body → dict (`Serializer.is_valid` for non-dicts: `non_field_errors`).
fn body_dict(headers: &HeaderMap, body: &Bytes) -> ApiResult<Map<String, Value>> {
    match parse_json_body(headers, body)? {
        Value::Object(m) => Ok(m),
        other => Err(ApiError::Validation(drf::non_field(not_a_dict_message(&other)))),
    }
}

fn invalid(errors: Errors) -> ApiError {
    ApiError::Validation(Value::Object(errors.0))
}

/// `HttpResponseBadRequest(f"Error performing {operation_label}, check logs for more detail.")`.
fn failed(label: &str, e: impl std::fmt::Display) -> ApiError {
    tracing::warn!("An error occurred performing {label}: {e}");
    ApiError::BadRequestText(format!("Error performing {label}, check logs for more detail."))
}

/// auditlog context of the middleware for this request.
fn request_log(state: &AppState, user: &CurrentUser, session: bool, meta: &RequestMeta) -> LogCtx {
    LogCtx::middleware(
        state.config().audit_log_enabled,
        if session { Some((&user.id, Some(user.email.as_str()))) } else { None },
        meta.remote_addr.clone(),
        meta.remote_port,
        meta.cid.clone(),
    )
}

/// `LogEntry.objects.log_create(..., actor=user)`: actor explicit (no e-mail), address/cid
/// from the middleware.
pub fn explicit_actor(state: &AppState, user_id: i64, meta: &RequestMeta) -> LogCtx {
    LogCtx {
        enabled: state.config().audit_log_enabled,
        actor_id: Some(user_id),
        actor_email: None,
        remote_addr: meta.remote_addr.clone(),
        remote_port: meta.remote_port,
        cid: meta.cid.clone(),
    }
}

/// `DocumentSelectionSerializer`: fields + `validate()`.
async fn selection_fields(conn: &mut PgConnection, data: &Map<String, Value>, errors: &mut Errors) -> ApiResult<Selection> {
    selection::parse_fields(conn, data, false, true, errors).await
}

/// `DocumentListSerializer.documents` (Pflichtfeld, `validate_documents`).
async fn list_field(conn: &mut PgConnection, data: &Map<String, Value>, errors: &mut Errors) -> ApiResult<Vec<i64>> {
    let sel = selection::parse_fields(conn, data, true, false, errors).await?;
    Ok(sel.documents.unwrap_or_default())
}

macro_rules! handler {
    ($name:ident, $inner:ident) => {
        pub async fn $name(
            State(state): State<AppState>,
            Authed(user, session): Authed,
            Peer(peer): Peer,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            match $inner(&state, &user, session, &headers, peer, &body).await {
                Ok(r) => r,
                Err(e) => e.into_response(),
            }
        }
    };
}

handler!(delete_handler, delete_inner);
handler!(rotate_handler, rotate_inner);
handler!(merge_handler, merge_inner);
handler!(merge_as_versions_handler, merge_as_versions_inner);
handler!(edit_pdf_handler, edit_pdf_inner);
handler!(remove_password_handler, remove_password_inner);

// ------------------------------------------------------------------ delete

/// POST /api/documents/delete/ (`DeleteDocumentsView`, `bulk_edit.delete`).
async fn delete_inner(
    state: &AppState,
    user: &CurrentUser,
    session: bool,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let data = body_dict(headers, body)?;
    let mut conn = state.db().acquire().await?;
    let mut errors = Errors::default();
    let mut sel = selection_fields(&mut conn, &data, &mut errors).await?;
    if !errors.is_empty() {
        return Err(invalid(errors));
    }
    if let Err(m) = selection::validate(&mut conn, &mut sel).await? {
        return Err(ApiError::Validation(drf::non_field(m)));
    }
    let (mut conn, ids) = selection::resolve(state, conn, user, &sel).await?;
    let needs = OperationNeeds { ownership: true, delete_document: true, ..Default::default() };
    if !has_document_permissions(&mut conn, user, &ids, needs).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);
    let meta = RequestMeta::from_request(headers, peer);
    // bulk_edit.delete catches every exception itself and always returns "OK"
    crate::tasks::bulk_delete_logged(state, &ids, request_log(state, user, session, &meta)).await;
    Ok(ok_result())
}

// ------------------------------------------------------------------ PDF methods

/// Execution of a PDF method via [`Exec`] (`method(documents, **parameters)`).
#[allow(clippy::too_many_arguments)]
async fn run_pdf(
    state: &AppState,
    queue: std::sync::Arc<dyn pl_queue::TaskQueue>,
    user: &CurrentUser,
    ids: &[i64],
    params: &PdfParams,
    mode: SourceMode,
    from_webui: bool,
    label: &str,
) -> ApiResult<Response> {
    let ex = Exec {
        pool: state.db().clone(),
        queue,
        media: MediaCfg::from_config(&state.live_config()),
        scratch: state.config().scratch_dir.clone(),
        user_id: Some(user.id),
        trigger: if from_webui { pl_core::tasks::trigger_source::WEB_UI } else { pl_core::tasks::trigger_source::API_UPLOAD },
        mode,
    };
    match ex.run(ids, params).await {
        Ok(()) => Ok(ok_result()),
        Err(e) => Err(failed(label, e)),
    }
}

/// The PaperRust queue (pl-server always attaches it). Without it `consume_file` cannot be
/// sent: misconfigured server → 500 (upstream: broker unreachable → uncaught error).
fn queue_or_delegate(state: &AppState) -> ApiResult<std::sync::Arc<dyn pl_queue::TaskQueue>> {
    pdf_edit::pg_queue(state).ok_or_else(|| ApiError::Uncaught("PDF operation: PaperRust task queue not attached".into()))
}

/// POST /api/documents/rotate/
async fn rotate_inner(
    state: &AppState,
    user: &CurrentUser,
    _session: bool,
    headers: &HeaderMap,
    _peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let queue = queue_or_delegate(state)?;
    let data = body_dict(headers, body)?;
    let mut conn = state.db().acquire().await?;
    let mut errors = Errors::default();
    let mut sel = selection_fields(&mut conn, &data, &mut errors).await?;
    let degrees = match int_field(&data, "degrees", true, false, &mut errors) {
        Some(Some(d)) => match multiple_of_90(d, "degrees") {
            Ok(d) => d,
            Err(e) => {
                errors.add("degrees", e);
                0
            }
        },
        _ => 0,
    };
    let mode = source_mode_field(&data, &mut errors);
    let from_webui = bool_field(&data, "from_webui", false, &mut errors);
    if !errors.is_empty() {
        return Err(invalid(errors));
    }
    if let Err(m) = selection::validate(&mut conn, &mut sel).await? {
        return Err(ApiError::Validation(drf::non_field(m)));
    }
    let (mut conn, ids) = selection::resolve(state, conn, user, &sel).await?;
    let params = PdfParams::Rotate { degrees };
    if !has_document_permissions(&mut conn, user, &ids, params.needs()).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);
    run_pdf(state, queue, user, &ids, &params, mode, from_webui, "document rotate").await
}

/// POST /api/documents/merge/
async fn merge_inner(
    state: &AppState,
    user: &CurrentUser,
    _session: bool,
    headers: &HeaderMap,
    _peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let queue = queue_or_delegate(state)?;
    let data = body_dict(headers, body)?;
    let mut conn = state.db().acquire().await?;
    let mut errors = Errors::default();
    let ids = list_field(&mut conn, &data, &mut errors).await?;
    let metadata_document_id = match int_field(&data, "metadata_document_id", false, true, &mut errors) {
        Some(Some(i)) => Some(Value::from(i)),
        // present and null → `metadata_document_id=None` (falsy like missing)
        _ => None,
    };
    let delete_originals = bool_field(&data, "delete_originals", false, &mut errors);
    let archive_fallback = bool_field(&data, "archive_fallback", false, &mut errors);
    let mode = source_mode_field(&data, &mut errors);
    let from_webui = bool_field(&data, "from_webui", false, &mut errors);
    if !errors.is_empty() {
        return Err(invalid(errors));
    }
    let params = PdfParams::Merge { metadata_document_id, delete_originals, archive_fallback };
    if !has_document_permissions(&mut conn, user, &ids, params.needs()).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);
    run_pdf(state, queue, user, &ids, &params, mode, from_webui, "document merge").await
}

/// POST /api/documents/edit_pdf/
async fn edit_pdf_inner(
    state: &AppState,
    user: &CurrentUser,
    _session: bool,
    headers: &HeaderMap,
    _peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let queue = queue_or_delegate(state)?;
    let data = body_dict(headers, body)?;
    let mut conn = state.db().acquire().await?;
    let mut errors = Errors::default();
    let ids = list_field(&mut conn, &data, &mut errors).await?;
    let operations = match data.get("operations") {
        None => {
            errors.add("operations", drf::msg(drf::REQUIRED));
            Vec::new()
        }
        Some(v) => match pdf_edit::validate_operations(v) {
            Ok(o) => o,
            Err(e) => {
                errors.add("operations", e);
                Vec::new()
            }
        },
    };
    let delete_original = bool_field(&data, "delete_original", false, &mut errors);
    let update_document = bool_field(&data, "update_document", false, &mut errors);
    let include_metadata = bool_field(&data, "include_metadata", true, &mut errors);
    let mode = source_mode_field(&data, &mut errors);
    let from_webui = bool_field(&data, "from_webui", false, &mut errors);
    if !errors.is_empty() {
        return Err(invalid(errors));
    }
    // EditPdfDocumentsSerializer.validate
    if ids.len() > 1 {
        return Err(ApiError::Validation(drf::non_field("Edit PDF method only supports one document")));
    }
    let n = operations.len() as i64;
    if operations.iter().any(|o| o.doc.unwrap_or(0) >= n) {
        return Err(ApiError::Validation(drf::non_field("doc index is out of bounds")));
    }
    if update_document && operations.iter().map(|o| o.doc.unwrap_or(0)).max().unwrap_or(0) > 0 {
        return Err(ApiError::Validation(drf::non_field("update_document only allowed with a single output document")));
    }
    // Document.objects.get(id=documents[0]) – empty list → IndexError (500)
    let Some(first) = ids.first().copied() else {
        return Err(ApiError::Uncaught("IndexError: list index out of range".into()));
    };
    let Some(doc) = docs::load_live(&mut conn, first).await? else {
        return Err(ApiError::Uncaught("Document matching query does not exist.".into()));
    };
    if let Some(pc) = doc.page_count.filter(|n| *n != 0)
        && let Some(op) = operations.iter().find(|o| o.page > pc)
    {
        return Err(ApiError::Validation(drf::non_field(format!("Page {} is out of bounds for document with {pc} pages.", op.page))));
    }
    let params = PdfParams::EditPdf { operations, delete_original, update_document, include_metadata };
    if !has_document_permissions(&mut conn, user, &ids, params.needs()).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);
    run_pdf(state, queue, user, &ids, &params, mode, from_webui, "PDF edit").await
}

/// POST /api/documents/remove_password/
async fn remove_password_inner(
    state: &AppState,
    user: &CurrentUser,
    _session: bool,
    headers: &HeaderMap,
    _peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let queue = queue_or_delegate(state)?;
    let data = body_dict(headers, body)?;
    let mut conn = state.db().acquire().await?;
    let mut errors = Errors::default();
    let ids = list_field(&mut conn, &data, &mut errors).await?;
    let password = match data.get("password") {
        None => {
            errors.add("password", drf::msg(drf::REQUIRED));
            String::new()
        }
        Some(v) => char_field(v, false, None).unwrap_or_else(|e| {
            errors.add("password", e);
            String::new()
        }),
    };
    let update_document = bool_field(&data, "update_document", false, &mut errors);
    let delete_original = bool_field(&data, "delete_original", false, &mut errors);
    let include_metadata = bool_field(&data, "include_metadata", true, &mut errors);
    let mode = source_mode_field(&data, &mut errors);
    let from_webui = bool_field(&data, "from_webui", false, &mut errors);
    if !errors.is_empty() {
        return Err(invalid(errors));
    }
    let params = PdfParams::RemovePassword { password, update_document, delete_original, include_metadata, source_paths_by_id: None };
    if !has_document_permissions(&mut conn, user, &ids, params.needs()).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);
    run_pdf(state, queue, user, &ids, &params, mode, from_webui, "password removal").await
}

// ------------------------------------------------------------------ merge_as_versions

/// `MergeDocumentsAsVersionsSerializer.validate_version_label`.
fn normalize_label(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// POST /api/documents/merge_as_versions/
async fn merge_as_versions_inner(
    state: &AppState,
    user: &CurrentUser,
    _session: bool,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    body: &Bytes,
) -> ApiResult<Response> {
    let data = body_dict(headers, body)?;
    let mut conn = state.db().acquire().await?;
    let mut errors = Errors::default();
    let ids = list_field(&mut conn, &data, &mut errors).await?;
    let root_id = int_field(&data, "root_document_id", true, false, &mut errors).flatten();
    // version_label: CharField(required=False, allow_blank, allow_null, max_length=64)
    let (label_given, version_label) = match data.get("version_label") {
        None => (false, None),
        Some(Value::Null) => (true, None),
        Some(v) => match char_field(v, true, Some(64)) {
            Ok(s) => (true, normalize_label(Some(s))),
            Err(e) => {
                errors.add("version_label", e);
                (true, None)
            }
        },
    };
    if !errors.is_empty() {
        return Err(invalid(errors));
    }
    let Some(root_id) = root_id else {
        return Err(ApiError::Uncaught("root_document_id missing after validation".into()));
    };
    // validate()
    if ids.len() < 2 {
        return Err(ApiError::Validation(drf::non_field("At least two documents are required.")));
    }
    if version_label.is_some() && ids.len() != 2 {
        return Err(ApiError::Validation(drf::non_field("version_label can only be used when merging one source document.")));
    }
    if !ids.contains(&root_id) {
        return Err(ApiError::Validation(drf::non_field("root_document_id must be one of the selected documents.")));
    }
    let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let has_versions_selected: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL AND root_document_id IS NOT NULL)",
    )
    .bind(&ids32)
    .fetch_one(&mut *conn)
    .await?;
    if has_versions_selected {
        return Err(ApiError::Validation(drf::non_field("Only top-level documents can be merged as versions.")));
    }
    let sources32: Vec<i32> = ids32.iter().copied().filter(|i| i64::from(*i) != root_id).collect();
    // Document.global_objects (incl. trash)
    let sources_have_versions: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM documents_document WHERE root_document_id = ANY($1))")
            .bind(&sources32)
            .fetch_one(&mut *conn)
            .await?;
    if sources_have_versions {
        return Err(ApiError::Validation(drf::non_field("Documents with existing versions cannot be merged into another document.")));
    }
    let needs = OperationNeeds { ownership: true, delete_document: true, ..Default::default() };
    if !has_document_permissions(&mut conn, user, &ids, needs).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);
    let meta = RequestMeta::from_request(headers, peer);
    let _ = label_given; // `version_label=None` and missing behave the same in merge_as_versions
    match merge_as_versions(state, &explicit_actor(state, user.id, &meta), &ids, root_id, version_label.as_deref()).await {
        Ok(()) => Ok(ok_result()),
        Err(e) => Err(failed("document merge as versions", e)),
    }
}

/// `bulk_edit.merge_as_versions(doc_ids, root_document_id=…, version_label=…, user=…)`.
pub async fn merge_as_versions(
    state: &AppState,
    log: &LogCtx,
    doc_ids: &[i64],
    root_document_id: i64,
    version_label: Option<&str>,
) -> Result<(), String> {
    let pool = state.db();
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let ids32: Vec<i32> = doc_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    // select_for_update().filter(id__in=…).order_by("id"). Since data model stage 1
    // (docs/DATAMODEL-V2.md §6) documents_document is a view with outer joins, on which
    // FOR UPDATE does not work → lock the rows of the native table.
    let native: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace           WHERE n.nspname = current_schema() AND c.relname = 'paperrust_document' AND c.relkind = 'r')",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let table = if native { "paperrust_document" } else { "documents_document" };
    let locked: Vec<i32> =
        sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY id FOR UPDATE"))
            .bind(&ids32)
            .fetch_all(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
    let mut by_id: HashMap<i64, pl_consumer::db::DocRow> = HashMap::new();
    for id in locked {
        if let Some(d) = docs::load_global(&mut tx, i64::from(id)).await.map_err(|e| e.to_string())? {
            by_id.insert(d.id, d);
        }
    }
    let source_ids: Vec<i64> = doc_ids.iter().copied().filter(|i| *i != root_document_id).collect();
    let Some(root) = by_id.get(&root_document_id).cloned() else {
        return Err(format!("KeyError: {root_document_id}"));
    };
    let max_index: Option<i32> = sqlx::query_scalar("SELECT MAX(version_index) FROM documents_document WHERE root_document_id = $1")
        .bind(root_document_id as i32)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let mut next_index = i64::from(max_index.unwrap_or(0));

    let mut source_asns: Vec<i64> = Vec::new();
    for sid in &source_ids {
        let d = by_id.get(sid).ok_or_else(|| format!("KeyError: {sid}"))?;
        if let Some(a) = d.archive_serial_number {
            source_asns.push(a);
        }
    }
    let mut fields: Vec<&str> = vec!["root_document", "version_index", "archive_serial_number"];
    if version_label.is_some() {
        fields.push("version_label");
    }
    // bulk_update (without signals, without auto_now)
    for sid in &source_ids {
        next_index += 1;
        let Some(d) = by_id.get_mut(sid) else { continue };
        d.root_document_id = Some(root.id);
        d.version_index = Some(next_index);
        d.archive_serial_number = None;
        if let Some(l) = version_label {
            d.version_label = Some(l.to_string());
        }
        docs::update_columns(&mut tx, d, &fields).await.map_err(|e| e.to_string())?;
    }
    let mut take_asn: Option<i64> = None;
    if !source_asns.is_empty() && root.archive_serial_number.is_none() {
        let a = source_asns.remove(0);
        tracing::info!(target: LOG, "Document {} took archive serial number {a} from a document merged into it", root.id);
        take_asn = Some(a);
    }
    if !source_asns.is_empty() {
        tracing::warn!(
            target: LOG,
            "Archive serial number(s) {} were removed by merging those documents as versions of document {}",
            drf::py_repr(&json!(source_asns)),
            root.id
        );
    }
    let now = docs::now_micros();
    match take_asn {
        Some(a) => {
            sqlx::query("UPDATE documents_document SET modified = $2, archive_serial_number = $3 WHERE id = $1 AND deleted_at IS NULL")
                .bind(root.id as i32)
                .bind(now)
                .bind(a)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?
        }
        None => sqlx::query("UPDATE documents_document SET modified = $2 WHERE id = $1 AND deleted_at IS NULL")
            .bind(root.id as i32)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?,
    };
    if log.enabled {
        let r = docs::repr(&mut tx, &root).await.map_err(|e| e.to_string())?;
        let mut ch = Map::new();
        ch.insert("Merged As Versions".into(), json!(["None", source_ids]));
        auditlog::write(
            &mut tx,
            log,
            "document",
            root.id,
            &r,
            auditlog::UPDATE,
            ch,
            Some(json!({"reason": "Merged as versions", "version_ids": source_ids})),
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;

    crate::index::remove(state, &source_ids).await;
    let req = crate::enqueue::Task::new(
        crate::enqueue::BULK_UPDATE_DOCUMENTS,
        json!({"document_ids": [root_document_id]}),
        crate::enqueue::TriggerSource::System,
    )
    .request();
    pl_core::tasks::send_task(state, req).await.map_err(|e| format!("{e:?}"))?;
    if let Some(bus) = state.service::<pl_queue::EventBus>() {
        pl_queue::events::DocumentsStatusManager::new(bus).send_documents_deleted(source_ids.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_field_like_drf() {
        assert_eq!(char_field(&json!("  a "), false, None), Ok("a".into()));
        assert_eq!(char_field(&json!(" "), false, None), Err(drf::msg("This field may not be blank.")));
        assert_eq!(char_field(&json!(""), true, None), Ok(String::new()));
        assert_eq!(char_field(&json!(5), false, None), Ok("5".into()));
        assert_eq!(char_field(&json!(true), false, None), Err(drf::msg("Not a valid string.")));
        assert_eq!(char_field(&json!([1]), false, None), Err(drf::msg("Not a valid string.")));
        assert_eq!(char_field(&Value::Null, true, None), Err(drf::msg(drf::NULL)));
        assert_eq!(char_field(&json!("x".repeat(65)), true, Some(64)), Err(drf::msg("Ensure this field has no more than 64 characters.")));
        assert_eq!(char_field(&json!("a\u{0}b"), false, None), Err(drf::msg("Null characters are not allowed.")));
    }

    #[test]
    fn fields_in_order() {
        let data = json!({"degrees": "x", "from_webui": "nope", "source_mode": "bad"});
        let Value::Object(m) = data else { unreachable!() };
        let mut e = Errors::default();
        assert_eq!(int_field(&m, "degrees", true, false, &mut e), None);
        assert_eq!(source_mode_field(&m, &mut e), SourceMode::LatestVersion);
        assert!(!bool_field(&m, "from_webui", false, &mut e));
        assert_eq!(
            Value::Object(e.0),
            json!({"degrees": [drf::INVALID_INT], "source_mode": ["Invalid source_mode"], "from_webui": [drf::INVALID_BOOL]})
        );
        assert_eq!(multiple_of_90(45, "degrees"), Err(drf::msg("degrees must be a multiple of 90")));
        assert_eq!(multiple_of_90(-270, "degrees"), Ok(-270));
    }

    #[test]
    fn source_mode_trimmed_and_choices() {
        let m: Map<String, Value> = serde_json::from_value(json!({"source_mode": " explicit_selection "})).unwrap();
        let mut e = Errors::default();
        assert_eq!(source_mode_field(&m, &mut e), SourceMode::ExplicitSelection);
        let m: Map<String, Value> = serde_json::from_value(json!({"source_mode": null})).unwrap();
        source_mode_field(&m, &mut e);
        assert_eq!(Value::Object(e.0), json!({"source_mode": [drf::NULL]}));
    }

    #[test]
    fn label_normalized() {
        assert_eq!(normalize_label(Some("  v2 ".into())), Some("v2".into()));
        assert_eq!(normalize_label(Some("".into())), None);
        assert_eq!(normalize_label(None), None);
    }
}
