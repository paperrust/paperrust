// upstream: src/documents/views.py@v3.4.2 (BulkEditView.post, DocumentOperationPermissionMixin.
//           _has_document_permissions, METHOD_NAMES_REQUIRING_USER/_TRIGGER_SOURCE),
//           src/documents/serialisers.py@v3.4.2 (BulkEditSerializer.validate,
//           _validate_parameters_rotate/_split/_delete_pages/_merge/_edit_pdf,
//           validate_parameters_remove_password, PdfEditOperationSerializer, SourceModeValidationMixin),
//           src/documents/bulk_edit.py@v3.4.2 (rotate, merge, split, delete_pages, edit_pdf,
//           remove_password, release_archive_serial_numbers, restore_archive_serial_numbers,
//           _resolve_root_and_source_doc, _scratch_path), src/documents/data_models.py@v3.4.2
//           (DocumentMetadataOverrides.from_document), src/documents/versioning.py@v3.4.2
//           (get_root_document, get_latest_version_for_root), rest_framework/fields.py (ListField,
//           IntegerField), rest_framework/serializers.py (Serializer.run_validation)
//
//! PDF methods of `POST /api/documents/bulk_edit/` (`rotate`, `merge`, `split`, `delete_pages`,
//! `edit_pdf`, `remove_password`) as [`crate::hooks::PdfOps`] ([`NativePdfOps`]).
//!
//! The PDF work is done by pl-pdfops (`ops`, `bulk`); `consume_file` (with `ConsumableDocument` +
//! `DocumentMetadataOverrides` as JSON, docs/contract/pl-queue.md B5) goes into the
//! PaperRust queue (`state.service::<Arc<dyn TaskQueue>>()`), incl. `link`/`link_error`
//! and chords/groups. Without an attached queue (crate tests) → `None`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, PgPool};

use pl_consumer::data::{ConsumableDocument, DocumentMetadataOverrides, DocumentSource};
use pl_consumer::db::DocRow;
use pl_core::AppState;
use pl_pdfops::bulk::{self as pbulk, SourceMode, names};
use pl_pdfops::ops::{self as pops, PageOperation, PageSpec};
use pl_queue::{EnqueueOptions, Signature, TaskQueue};

use crate::drf::{self, Errors, py_truthy};
use crate::error::{ApiError, ApiResult};
use crate::files::MediaCfg;
use crate::perms::{OperationNeeds, has_document_permissions};
use crate::selection;
use crate::user::CurrentUser;

const LOG: &str = "paperless.bulk_edit";
pub const CONSUME_FILE: &str = "documents.tasks.consume_file";
pub const BULK_DELETE: &str = "documents.bulk_edit.delete";
pub const RESTORE_ASN: &str = "documents.bulk_edit.restore_archive_serial_numbers_task";

/// The PDF methods in `BulkEditSerializer.method`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfMethod {
    Rotate,
    Merge,
    Split,
    DeletePages,
    EditPdf,
    RemovePassword,
}

impl PdfMethod {
    pub fn from_choice(s: &str) -> Option<Self> {
        Some(match s {
            "rotate" => Self::Rotate,
            "merge" => Self::Merge,
            "split" => Self::Split,
            "delete_pages" => Self::DeletePages,
            "edit_pdf" => Self::EditPdf,
            "remove_password" => Self::RemovePassword,
            _ => return None,
        })
    }

    /// Keyword arguments of the Python function (otherwise `TypeError` → 400). `user` and
    /// `trigger_source` are set by the view itself (overrides inputs).
    fn accepted_kwargs(self) -> &'static [&'static str] {
        match self {
            Self::Rotate => &["degrees", "source_mode", "user", "trigger_source"],
            Self::Merge => &["metadata_document_id", "delete_originals", "archive_fallback", "source_mode", "user", "trigger_source"],
            Self::Split => &["pages", "delete_originals", "source_mode", "user", "trigger_source"],
            Self::DeletePages => &["pages", "source_mode", "user", "trigger_source"],
            Self::EditPdf => {
                &["operations", "delete_original", "update_document", "include_metadata", "source_mode", "user", "trigger_source"]
            }
            Self::RemovePassword => &[
                "password",
                "update_document",
                "delete_original",
                "include_metadata",
                "source_mode",
                "user",
                "trigger_source",
                "source_paths_by_id",
            ],
        }
    }

    /// `all=true` not allowed (`This method does not support all=true.`).
    fn rejects_all(self) -> bool {
        !matches!(self, Self::Rotate)
    }
}

/// Validated parameters (raw values, where upstream uses them unvalidated by truthiness).
#[derive(Clone, Debug)]
pub enum PdfParams {
    Rotate {
        degrees: i64,
    },
    Merge {
        metadata_document_id: Option<Value>,
        delete_originals: bool,
        archive_fallback: bool,
    },
    Split {
        pages: Vec<Vec<i64>>,
        delete_originals: bool,
    },
    DeletePages {
        pages: Vec<i64>,
    },
    EditPdf {
        operations: Vec<PageOperation>,
        delete_original: bool,
        update_document: bool,
        include_metadata: bool,
    },
    RemovePassword {
        password: String,
        update_document: bool,
        delete_original: bool,
        include_metadata: bool,
        source_paths_by_id: Option<Value>,
    },
}

impl PdfParams {
    /// Additional conditions of `_has_document_permissions` (all PDF methods change or replace
    /// the original or create documents).
    pub fn needs(&self) -> OperationNeeds {
        match self {
            Self::Rotate { .. } | Self::DeletePages { .. } => OperationNeeds { ownership: true, ..Default::default() },
            Self::Merge { delete_originals, .. } | Self::Split { delete_originals, .. } => {
                OperationNeeds { ownership: *delete_originals, add_document: true, delete_document: *delete_originals }
            }
            Self::EditPdf { delete_original, update_document, .. } | Self::RemovePassword { delete_original, update_document, .. } => {
                OperationNeeds { ownership: true, add_document: !*update_document, delete_document: *delete_original && !*update_document }
            }
        }
    }
}

/// `SourceModeChoices.__dict__.values()` (Python 3.12): also `"documents.bulk_edit"`
/// (`__module__`) and `None` (`__doc__`) count as valid and act like `latest_version`.
pub fn parse_source_mode(v: Option<&Value>) -> Option<SourceMode> {
    match v {
        None | Some(Value::Null) => Some(SourceMode::LatestVersion),
        Some(Value::String(s)) => match s.as_str() {
            "explicit_selection" => Some(SourceMode::ExplicitSelection),
            "latest_version" | "documents.bulk_edit" => Some(SourceMode::LatestVersion),
            _ => None,
        },
        Some(_) => None,
    }
}

// ------------------------------------------------------------------ Validation

/// `Ensure this value is greater than or equal to {n}.`
fn min_msg(n: i64) -> String {
    format!("Ensure this value is greater than or equal to {n}.")
}

/// A field of `PdfEditOperationSerializer` (`IntegerField(min_value=…)`).
fn op_int(m: &Map<String, Value>, key: &str, required: bool, min: Option<i64>) -> Result<Option<i64>, Value> {
    let Some(v) = m.get(key) else {
        return if required { Err(drf::msg(drf::REQUIRED)) } else { Ok(None) };
    };
    if v.is_null() {
        return Err(drf::msg(drf::NULL));
    }
    let i = drf::drf_int(v).map_err(drf::msg)?;
    if let Some(min) = min
        && i < min
    {
        return Err(drf::msg(min_msg(min)));
    }
    Ok(Some(i))
}

/// `PdfEditOperationSerializer().run_validation(item)`.
fn validate_operation(v: &Value) -> Result<PageOperation, Value> {
    let m = match v {
        Value::Null => return Err(drf::msg(drf::NULL)),
        Value::Object(m) => m,
        other => {
            return Err(drf::non_field(format!("Invalid data. Expected a dictionary, but got {}.", drf::py_type_name(other))));
        }
    };
    let mut errors = Errors::default();
    let page = op_int(m, "page", true, Some(1)).map_err(|e| errors.add("page", e)).ok().flatten();
    let rotate = match op_int(m, "rotate", false, None) {
        Ok(Some(r)) if r % 90 != 0 => {
            errors.add("rotate", drf::msg("rotate must be a multiple of 90"));
            None
        }
        Ok(r) => r,
        Err(e) => {
            errors.add("rotate", e);
            None
        }
    };
    let doc = op_int(m, "doc", false, Some(0)).map_err(|e| errors.add("doc", e)).ok().flatten();
    if !errors.is_empty() {
        return Err(Value::Object(errors.0));
    }
    Ok(PageOperation { page: page.unwrap_or(1), rotate, doc })
}

/// `ListField(child=PdfEditOperationSerializer(), allow_empty=False).run_validation(data)`;
/// error = `e.detail` (list or `{index: detail}`).
pub fn validate_operations(v: &Value) -> Result<Vec<PageOperation>, Value> {
    let items = match v {
        Value::Null => return Err(drf::msg(drf::NULL)),
        Value::Array(items) => items,
        other => {
            return Err(drf::msg(format!("Expected a list of items but got type \"{}\".", drf::py_type_name(other))));
        }
    };
    if items.is_empty() {
        return Err(drf::msg("This list may not be empty."));
    }
    let mut out = Vec::with_capacity(items.len());
    let mut errors = Map::new();
    for (i, it) in items.iter().enumerate() {
        match validate_operation(it) {
            Ok(o) => out.push(o),
            Err(e) => {
                errors.insert(i.to_string(), e);
            }
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(Value::Object(errors)) }
}

/// `parameters["pages"]` of `split`: `"1-2,3"` → `[[1, 2], [3]]` (bounds 1..=page_count).
pub fn parse_split_pages(s: &str, page_count: i64) -> Option<Vec<Vec<i64>>> {
    let mut out = Vec::new();
    for group in s.split(',') {
        let (start, is_range, end) = match group.split_once('-') {
            Some((a, b)) => (a, true, b),
            None => (group, false, ""),
        };
        let first = drf::py_int_from_str(start)?;
        let last = if is_range { drf::py_int_from_str(end)? } else { first };
        if !(1 <= first && first <= last && last <= page_count) {
            return None;
        }
        out.push((first..=last).collect());
    }
    Some(out)
}

/// `isinstance(i, int)` list (JSON bool counts as int).
fn int_items(items: &[Value]) -> Option<Vec<i64>> {
    items
        .iter()
        .map(|v| match v {
            Value::Bool(b) => Some(i64::from(*b)),
            Value::Number(n) if n.is_i64() => n.as_i64(),
            Value::Number(n) if n.is_u64() => Some(i64::MAX),
            _ => None,
        })
        .collect()
}

fn strict_bool(p: &Map<String, Value>, key: &str) -> Result<bool, String> {
    match p.get(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

/// Result of the method-specific validation.
pub enum Checked {
    Ok(PdfParams),
    /// 400 with a ready-made error body.
    Invalid(Value),
}

fn nf(m: impl Into<String>) -> ApiResult<Checked> {
    Ok(Checked::Invalid(drf::non_field(m)))
}

async fn page_count_of(conn: &mut PgConnection, id: i64) -> ApiResult<Option<i64>> {
    // Document.objects.get(id=…) – exists (validate_documents); otherwise DoesNotExist → 500
    match crate::docs::load_live(conn, id).await? {
        Some(d) => Ok(d.page_count),
        None => Err(ApiError::Uncaught("Document matching query does not exist.".into())),
    }
}

/// `BulkEditSerializer.validate` from `if "source_mode" in parameters` on, for the PDF methods.
/// Adds default values to `parameters` as upstream (for the kwargs check).
pub async fn validate_parameters(
    conn: &mut PgConnection,
    method: PdfMethod,
    documents: &[i64],
    parameters: &mut Map<String, Value>,
) -> ApiResult<Checked> {
    if parameters.contains_key("source_mode") && parse_source_mode(parameters.get("source_mode")).is_none() {
        return nf("Invalid source_mode");
    }
    // attrs["documents"][0] on an empty list → IndexError (not caught → 500)
    let first = || documents.first().copied().ok_or_else(|| ApiError::Uncaught("IndexError: list index out of range".into()));
    match method {
        PdfMethod::Rotate => {
            let Some(v) = parameters.get("degrees").filter(|v| !v.is_null()) else {
                return nf("invalid rotation degrees");
            };
            let Ok(d) = drf::drf_int(v) else {
                return nf("invalid rotation degrees");
            };
            if d % 90 != 0 {
                return nf("degrees must be a multiple of 90");
            }
            parameters.insert("degrees".into(), Value::from(d));
            Ok(Checked::Ok(PdfParams::Rotate { degrees: d }))
        }
        PdfMethod::Split => {
            if documents.len() > 1 {
                return nf("Split method only supports one document");
            }
            let doc_id = first()?;
            let Some(pages) = parameters.get("pages") else {
                return nf("pages not specified");
            };
            let Value::String(pages) = pages else {
                return nf("invalid pages specified");
            };
            let page_count = page_count_of(conn, doc_id).await?.filter(|n| *n != 0);
            let Some(page_count) = page_count else {
                return nf("document page count is unknown");
            };
            let Some(groups) = parse_split_pages(pages, page_count) else {
                return nf("invalid pages specified");
            };
            parameters.insert("pages".into(), json!(groups));
            let delete_originals = match strict_bool(parameters, "delete_originals") {
                Ok(b) => b,
                Err(m) => return nf(m),
            };
            parameters.insert("delete_originals".into(), Value::Bool(delete_originals));
            Ok(Checked::Ok(PdfParams::Split { pages: groups, delete_originals }))
        }
        PdfMethod::DeletePages => {
            if documents.len() > 1 {
                return nf("Delete pages method only supports one document");
            }
            let Some(pages) = parameters.get("pages") else {
                return nf("pages not specified");
            };
            let Value::Array(items) = pages else {
                return nf("pages must be a list");
            };
            let Some(pages) = int_items(items) else {
                return nf("pages must be a list of integers");
            };
            if pages.iter().any(|p| *p < 1) {
                return nf("pages must be positive integers");
            }
            Ok(Checked::Ok(PdfParams::DeletePages { pages }))
        }
        PdfMethod::Merge => {
            let delete_originals = match strict_bool(parameters, "delete_originals") {
                Ok(b) => b,
                Err(m) => return nf(m),
            };
            parameters.insert("delete_originals".into(), Value::Bool(delete_originals));
            let archive_fallback = match strict_bool(parameters, "archive_fallback") {
                Ok(b) => b,
                Err(m) => return nf(m),
            };
            parameters.insert("archive_fallback".into(), Value::Bool(archive_fallback));
            Ok(Checked::Ok(PdfParams::Merge {
                metadata_document_id: parameters.get("metadata_document_id").cloned(),
                delete_originals,
                archive_fallback,
            }))
        }
        PdfMethod::EditPdf => {
            if documents.len() > 1 {
                return nf("Edit PDF method only supports one document");
            }
            let doc_id = first()?;
            let Some(ops) = parameters.get("operations") else {
                return nf("operations not specified");
            };
            let operations = match validate_operations(ops) {
                Ok(o) => o,
                Err(detail) => return Ok(Checked::Invalid(json!({ "operations": detail }))),
            };
            let validated_ops: Vec<Value> = operations.iter().map(|o| serde_json::to_value(o).unwrap_or(Value::Null)).collect();
            parameters.insert("operations".into(), Value::Array(validated_ops));
            let update_document = match parameters.get("update_document") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return nf("update_document must be a boolean"),
            };
            parameters.insert("update_document".into(), Value::Bool(update_document));
            let include_metadata = match parameters.get("include_metadata") {
                None => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => return nf("include_metadata must be a boolean"),
            };
            parameters.insert("include_metadata".into(), Value::Bool(include_metadata));
            let n = operations.len() as i64;
            if operations.iter().any(|o| o.doc.unwrap_or(0) >= n) {
                return nf("doc index is out of bounds");
            }
            if update_document && operations.iter().map(|o| o.doc.unwrap_or(0)).max().unwrap_or(0) > 0 {
                return nf("update_document only allowed with a single output document");
            }
            if let Some(pc) = page_count_of(conn, doc_id).await?.filter(|n| *n != 0)
                && let Some(op) = operations.iter().find(|o| o.page > pc)
            {
                return nf(format!("Page {} is out of bounds for document with {pc} pages.", op.page));
            }
            Ok(Checked::Ok(PdfParams::EditPdf {
                delete_original: parameters.get("delete_original").is_some_and(py_truthy),
                operations,
                update_document,
                include_metadata,
            }))
        }
        PdfMethod::RemovePassword => {
            let Some(pw) = parameters.get("password") else {
                return nf("password not specified");
            };
            let Value::String(pw) = pw else {
                return nf("password must be a string");
            };
            Ok(Checked::Ok(PdfParams::RemovePassword {
                password: pw.clone(),
                update_document: parameters.get("update_document").is_some_and(py_truthy),
                delete_original: parameters.get("delete_original").is_some_and(py_truthy),
                include_metadata: parameters.get("include_metadata").is_none_or(py_truthy),
                source_paths_by_id: parameters.get("source_paths_by_id").cloned(),
            }))
        }
    }
}

// ------------------------------------------------------------------ Hook

/// Implementation of [`crate::hooks::PdfOps`] (pl-server registers it with `set_pdf_ops`).
#[derive(Debug, Default, Clone, Copy)]
pub struct NativePdfOps;

/// The attached PaperRust queue.
pub fn pg_queue(state: &AppState) -> Option<Arc<dyn TaskQueue>> {
    state.service::<Arc<dyn TaskQueue>>()
}

impl crate::hooks::PdfOps for NativePdfOps {
    fn bulk_edit<'a>(
        &'a self,
        state: &'a AppState,
        user: &'a pl_authz::User,
        method: &'a str,
        data: &'a Map<String, Value>,
        _meta: &'a crate::audit::RequestMeta,
    ) -> Pin<Box<dyn Future<Output = Option<Response>> + Send + 'a>> {
        Box::pin(async move {
            let method = PdfMethod::from_choice(method)?;
            let queue = pg_queue(state)?;
            Some(match bulk_edit_pdf(state, queue, user, method, data).await {
                Ok(r) => r,
                Err(e) => e.into_response(),
            })
        })
    }
}

/// `BulkEditView.post` for a PDF method (from the parsed JSON dict).
pub async fn bulk_edit_pdf(
    state: &AppState,
    queue: Arc<dyn TaskQueue>,
    user: &CurrentUser,
    method: PdfMethod,
    data: &Map<String, Value>,
) -> ApiResult<Response> {
    let mut conn = state.inner.db.acquire().await?;
    let common = crate::views::parse_bulk_fields(&mut conn, data, None).await?;
    let (sel, mut parameters, from_webui) = (common.selection, common.parameters, common.from_webui);
    if sel.all && method.rejects_all() {
        return Err(ApiError::Validation(drf::non_field("This method does not support all=true.")));
    }
    let documents = sel.documents.clone().unwrap_or_default();
    let params = match validate_parameters(&mut conn, method, &documents, &mut parameters).await? {
        Checked::Ok(p) => p,
        Checked::Invalid(v) => return Err(ApiError::Validation(v)),
    };
    let mode = parse_source_mode(parameters.get("source_mode")).unwrap_or_default();
    let accepted = method.accepted_kwargs();
    let kwargs_ok = parameters.keys().all(|k| accepted.contains(&k.as_str()));

    let (mut conn, ids) = selection::resolve(state, conn, user, &sel).await?;
    if !has_document_permissions(&mut conn, user, &ids, params.needs()).await? {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    drop(conn);

    let ex = Exec {
        pool: state.db().clone(),
        queue,
        media: MediaCfg::from_config(&state.live_config()),
        scratch: state.config().scratch_dir.clone(),
        user_id: Some(user.id),
        trigger: if from_webui { pl_core::tasks::trigger_source::WEB_UI } else { pl_core::tasks::trigger_source::API_UPLOAD },
        mode,
    };
    let result =
        if kwargs_ok { ex.run(&ids, &params).await } else { Err(format!("{}() got an unexpected keyword argument", method_name(method))) };
    match result {
        Ok(()) => Ok(crate::views::ok_result()),
        Err(e) => {
            tracing::warn!("An error occurred performing bulk edit: {e}");
            Err(ApiError::BadRequestText("Error performing bulk edit, check logs for more detail.".into()))
        }
    }
}

fn method_name(m: PdfMethod) -> &'static str {
    match m {
        PdfMethod::Rotate => "rotate",
        PdfMethod::Merge => "merge",
        PdfMethod::Split => "split",
        PdfMethod::DeletePages => "delete_pages",
        PdfMethod::EditPdf => "edit_pdf",
        PdfMethod::RemovePassword => "remove_password",
    }
}

// ------------------------------------------------------------------ Execution

/// Execution context of a PDF method (`user`, `trigger_source`, `source_mode`).
pub struct Exec {
    pub pool: PgPool,
    pub queue: Arc<dyn TaskQueue>,
    pub media: MediaCfg,
    pub scratch: PathBuf,
    pub user_id: Option<i64>,
    pub trigger: &'static str,
    pub mode: SourceMode,
}

/// `ResolvedDocPair`.
struct Pair {
    root: DocRow,
    source: DocRow,
}

type ExecResult = Result<(), String>;

fn db(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Python repr of an int list (`[1, 2]`).
fn py_list(v: &[i64]) -> String {
    format!("[{}]", v.iter().map(i64::to_string).collect::<Vec<_>>().join(", "))
}

/// PDF work outside the async runtime thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> pl_pdfops::Result<T> + Send + 'static) -> Result<T, String> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(e) => Err(format!("PDF task failed: {e}")),
    }
}

/// `Document.objects.select_related("root_document").filter(id__in=ids)` as a map.
async fn live_docs(conn: &mut PgConnection, ids: &[i64]) -> Result<std::collections::HashMap<i64, DocRow>, String> {
    let mut out = std::collections::HashMap::new();
    for id in ids {
        if let Some(d) = crate::docs::load_live(conn, *id).await.map_err(db)? {
            out.insert(*id, d);
        }
    }
    Ok(out)
}

/// `Document.objects.get(id=…)`.
async fn get_doc(conn: &mut PgConnection, id: i64) -> Result<DocRow, String> {
    crate::docs::load_live(conn, id).await.map_err(db)?.ok_or_else(|| "Document matching query does not exist.".to_string())
}

/// `_resolve_root_and_source_doc`.
async fn resolve_pair(conn: &mut PgConnection, doc: &DocRow, mode: SourceMode) -> Result<Pair, String> {
    // get_root_document: select_related("root_document") (also deleted), otherwise the document
    let root = match doc.root_document_id {
        None => doc.clone(),
        Some(r) => crate::docs::load_global(conn, r).await.map_err(db)?.unwrap_or_else(|| doc.clone()),
    };
    if mode == SourceMode::ExplicitSelection || doc.root_document_id.is_some() {
        return Ok(Pair { root, source: doc.clone() });
    }
    // get_latest_version_for_root: Document.objects (without deleted), version_index DESC NULLS LAST, id DESC
    let latest: Option<i32> = sqlx::query_scalar(
        "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL \
         ORDER BY version_index DESC NULLS LAST, id DESC LIMIT 1",
    )
    .bind(root.id as i32)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    let source = match latest {
        Some(id) => crate::docs::load_global(conn, i64::from(id)).await.map_err(db)?.unwrap_or_else(|| root.clone()),
        None => root.clone(),
    };
    Ok(Pair { root, source })
}

/// `DocumentMetadataOverrides.from_document(doc)`.
pub async fn overrides_from_document(pool: &PgPool, doc: &DocRow) -> Result<DocumentMetadataOverrides, String> {
    // FINDINGS B2: read the permitted ones before fetching the own connection (never two at the same time)
    let grantees = pl_authz::owned::get_permission_grantees(pool, &pl_authz::models::DOCUMENT, &[doc.id])
        .await
        .map_err(|e| format!("{e:?}"))?
        .remove(&doc.id)
        .unwrap_or_default();
    let mut conn = pool.acquire().await.map_err(db)?;
    // doc.tags.values_list("id", flat=True) – Tag.Meta.ordering = ("name",)
    let tags: Vec<i32> = sqlx::query_scalar(
        "SELECT t.id FROM documents_document_tags dt JOIN documents_tag t ON t.id = dt.tag_id \
         WHERE dt.document_id = $1 ORDER BY t.name",
    )
    .bind(doc.id as i32)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let mut custom_fields = std::collections::BTreeMap::new();
    for c in crate::cfi::live_for_document(&mut conn, doc.id).await.map_err(db)? {
        let data_type = crate::cfi::field_def(&mut conn, c.field_id).await.map_err(db)?.map(|f| f.data_type).unwrap_or_default();
        custom_fields.insert(c.field_id, crate::cfi::value_of(&c, &data_type));
    }
    Ok(DocumentMetadataOverrides {
        title: Some(doc.title.clone()),
        correspondent_id: doc.correspondent_id,
        document_type_id: doc.document_type_id,
        storage_path_id: doc.storage_path_id,
        owner_id: doc.owner_id,
        tag_ids: Some(tags.into_iter().map(i64::from).collect()),
        created: Some(doc.created),
        view_users: Some(grantees.view.users),
        view_groups: Some(grantees.view.groups),
        change_users: Some(grantees.change.users),
        change_groups: Some(grantees.change.groups),
        custom_fields: Some(custom_fields),
        ..Default::default()
    })
}

/// `release_archive_serial_numbers(doc_ids)` → backup `{id: asn}`.
pub async fn release_archive_serial_numbers(pool: &PgPool, doc_ids: &[i64]) -> Result<Vec<(i64, i64)>, String> {
    let ids32: Vec<i32> = doc_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let rows: Vec<(i32, i32)> = sqlx::query_as(
        "SELECT id, archive_serial_number FROM documents_document \
         WHERE id = ANY($1) AND archive_serial_number IS NOT NULL AND deleted_at IS NULL ORDER BY id",
    )
    .bind(&ids32)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    sqlx::query(
        "UPDATE documents_document SET archive_serial_number = NULL \
         WHERE id = ANY($1) AND archive_serial_number IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(&ids32)
    .execute(pool)
    .await
    .map_err(db)?;
    let backup: Vec<(i64, i64)> = rows.into_iter().map(|(i, a)| (i64::from(i), i64::from(a))).collect();
    tracing::info!(target: LOG, "Released archive serial numbers for documents {}", py_list(&backup.iter().map(|b| b.0).collect::<Vec<_>>()));
    Ok(backup)
}

/// `restore_archive_serial_numbers(backup)`.
pub async fn restore_archive_serial_numbers(pool: &PgPool, backup: &[(i64, Option<i64>)]) -> Result<(), String> {
    for (id, asn) in backup {
        // Document.objects.filter(pk=…).update(…) – default manager (without deleted)
        sqlx::query("UPDATE documents_document SET archive_serial_number = $2 WHERE id = $1 AND deleted_at IS NULL")
            .bind(*id as i32)
            .bind(asn.map(|a| a as i32))
            .execute(pool)
            .await
            .map_err(db)?;
    }
    tracing::info!(target: LOG, "Restored archive serial numbers for documents {}", py_list(&backup.iter().map(|b| b.0).collect::<Vec<_>>()));
    Ok(())
}

fn backup_json(backup: &[(i64, i64)]) -> Value {
    let mut m = Map::new();
    for (id, asn) in backup {
        m.insert(id.to_string(), Value::from(*asn));
    }
    Value::Object(m)
}

fn backup_restore_list(backup: &[(i64, i64)]) -> Vec<(i64, Option<i64>)> {
    backup.iter().map(|(i, a)| (*i, Some(*a))).collect()
}

/// `delete.si(doc_ids)`.
fn delete_sig(ids: &[i64]) -> Signature {
    Signature { args: vec![json!(ids)], ..Signature::immutable(BULK_DELETE, Map::new()) }
}

/// `restore_archive_serial_numbers_task.s(backup)`.
fn restore_sig(backup: &[(i64, i64)]) -> Signature {
    Signature { args: vec![backup_json(backup)], ..Signature::new(RESTORE_ASN, Map::new()) }
}

impl Exec {
    fn scratch_path(&self, name: &str) -> pl_pdfops::Result<PathBuf> {
        pbulk::scratch_path(&self.scratch, name)
    }

    /// `consume_file.s(input_doc=ConsumableDocument(ConsumeFolder, file, root_document_id), overrides).set(headers=…)`
    fn consume_sig(&self, file: &Path, root_document_id: Option<i64>, overrides: &DocumentMetadataOverrides) -> Result<Signature, String> {
        let mut doc = ConsumableDocument::new(DocumentSource::ConsumeFolder, file).map_err(|e| e.to_string())?;
        doc.root_document_id = root_document_id;
        let kwargs = pl_consumer::task::consume_kwargs(&doc, overrides)?;
        Ok(Signature::new(CONSUME_FILE, kwargs).with_trigger(self.trigger))
    }

    /// `sig.apply_async(**options)`.
    async fn apply_async(&self, sig: Signature, link: Vec<Signature>, link_error: Vec<Signature>) -> Result<(), String> {
        let mut opts: EnqueueOptions = sig.options;
        opts.link = link;
        opts.link_error = link_error;
        self.queue.enqueue(&sig.task, sig.kwargs, opts).await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn group(&self, sigs: Vec<Signature>) -> Result<(), String> {
        if sigs.is_empty() {
            // group([]).delay() → freeze(), nothing sent
            return Ok(());
        }
        self.queue.enqueue_group(sigs).await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn chord(&self, header: Vec<Signature>, body: Signature, on_error: Option<Signature>) -> Result<(), String> {
        self.queue.enqueue_chord(header, body, on_error).await.map(|_| ()).map_err(|e| e.to_string())
    }

    /// `chord(header, delete.si([id])).on_error(restore.s(backup)).apply_async()` with prior
    /// ASN release; error on sending → ASNs back, error passed on.
    async fn chord_with_release(&self, header: Vec<Signature>, doc_id: i64) -> Result<(), String> {
        let backup = release_archive_serial_numbers(&self.pool, &[doc_id]).await?;
        if let Err(e) = self.chord(header, delete_sig(&[doc_id]), Some(restore_sig(&backup))).await {
            restore_archive_serial_numbers(&self.pool, &backup_restore_list(&backup)).await?;
            return Err(e);
        }
        Ok(())
    }

    pub async fn run(&self, doc_ids: &[i64], params: &PdfParams) -> ExecResult {
        match params {
            PdfParams::Rotate { degrees } => self.rotate(doc_ids, *degrees).await,
            PdfParams::Merge { metadata_document_id, delete_originals, archive_fallback } => {
                self.merge(doc_ids, metadata_document_id.as_ref(), *delete_originals, *archive_fallback).await
            }
            PdfParams::Split { pages, delete_originals } => self.split(doc_ids, pages, *delete_originals).await,
            PdfParams::DeletePages { pages } => self.delete_pages(doc_ids, pages).await,
            PdfParams::EditPdf { operations, delete_original, update_document, include_metadata } => {
                self.edit_pdf(doc_ids, operations, *delete_original, *update_document, *include_metadata).await
            }
            PdfParams::RemovePassword { password, update_document, delete_original, include_metadata, source_paths_by_id } => {
                self.remove_password(doc_ids, password, *update_document, *delete_original, *include_metadata, source_paths_by_id.as_ref())
                    .await
            }
        }
    }

    /// `bulk_edit.rotate`
    async fn rotate(&self, doc_ids: &[i64], degrees: i64) -> ExecResult {
        tracing::info!(target: LOG, "Attempting to rotate {} documents by {degrees} degrees.", doc_ids.len());
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let docs = live_docs(&mut conn, doc_ids).await?;
        let mut pairs: Vec<Pair> = Vec::new();
        for id in doc_ids {
            let Some(doc) = docs.get(id) else { continue };
            let pair = resolve_pair(&mut conn, doc, self.mode).await?;
            if !pairs.iter().any(|p| p.root.id == pair.root.id) {
                pairs.push(pair);
            }
        }
        drop(conn);
        for pair in pairs {
            let root = pair.root.id;
            if pair.source.mime_type != "application/pdf" {
                tracing::warn!(target: LOG, "Document {root} is not a PDF, skipping rotation.");
                continue;
            }
            let r: ExecResult = async {
                let path = self.scratch_path(&names::rotated(root)).map_err(|e| e.to_string())?;
                let (src, dst) = (self.media.checked_source_path(&pair.source)?, path.clone());
                blocking(move || pops::rotate_pdf(&src, &dst, degrees)).await?;
                let mut o = overrides_from_document(&self.pool, &pair.root).await?;
                o.actor_id = self.user_id;
                self.apply_async(self.consume_sig(&path, Some(root), &o)?, Vec::new(), Vec::new()).await?;
                tracing::info!(target: LOG, "Queued new rotated version for document {root} by {degrees} degrees");
                Ok(())
            }
            .await;
            if let Err(e) = r {
                tracing::error!(target: LOG, "Error rotating document {root}: {e}");
            }
        }
        Ok(())
    }

    /// `bulk_edit.merge`
    async fn merge(
        &self,
        doc_ids: &[i64],
        metadata_document_id: Option<&Value>,
        delete_originals: bool,
        archive_fallback: bool,
    ) -> ExecResult {
        tracing::info!(target: LOG, "Attempting to merge {} documents into a single document.", doc_ids.len());
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let docs = live_docs(&mut conn, doc_ids).await?;
        let mut inputs: Vec<(i64, PathBuf)> = Vec::new();
        for id in doc_ids {
            let Some(doc) = docs.get(id) else { continue };
            let pair = resolve_pair(&mut conn, doc, self.mode).await?;
            // Review H1: read only files within ORIGINALS_DIR/ARCHIVE_DIR; otherwise skip like an
            // unreadable source.
            let path = if archive_fallback && pair.source.mime_type != "application/pdf" {
                match self.media.checked_archive_path(&pair.source) {
                    Ok(Some(a)) => Ok(a),
                    Ok(None) => self.media.checked_source_path(&pair.source),
                    Err(e) => Err(e),
                }
            } else {
                self.media.checked_source_path(&pair.source)
            };
            match path {
                Ok(p) => inputs.push((doc.id, p)),
                Err(e) => tracing::error!(target: LOG, "Error merging document {}, it will not be included in the merge: {e}", doc.id),
            }
        }
        drop(conn);
        let (added, merged_file) = {
            let scratch = self.scratch.clone();
            let inputs = inputs.clone();
            tokio::task::spawn_blocking(move || -> Result<(Vec<i64>, Option<PathBuf>), String> {
                let mut merger = pops::PdfMerger::new();
                let mut added = Vec::new();
                for (id, path) in &inputs {
                    match merger.add(path) {
                        Ok(()) => added.push(*id),
                        Err(e) => {
                            tracing::error!(target: LOG, "Error merging document {id}, it will not be included in the merge: {e}");
                        }
                    }
                }
                if added.is_empty() {
                    return Ok((added, None));
                }
                let path = pbulk::scratch_path(&scratch, &names::merged(&added)).map_err(|e| e.to_string())?;
                merger.save(&path).map_err(|e| e.to_string())?;
                Ok((added, Some(path)))
            })
            .await
            .map_err(|e| format!("PDF task failed: {e}"))??
        };
        let Some(merged_file) = merged_file else {
            tracing::warn!(target: LOG, "No documents were merged");
            return Ok(());
        };
        // handoff_asn: first ASN of the merged (selected) documents
        let mut handoff: Option<i64> = added.iter().find_map(|id| docs.get(id).and_then(|d| d.archive_serial_number));
        let mut overrides = DocumentMetadataOverrides::default();
        if let Some(mid) = metadata_document_id.filter(|v| py_truthy(v)) {
            // qs.get(id=metadata_document_id), qs = documents from doc_ids
            let id = drf::django_int_lookup(mid)
                .map_err(|_| format!("Field 'id' expected a number but got {}.", drf::py_repr(mid)))?
                .ok_or_else(|| "Document matching query does not exist.".to_string())?;
            let meta = docs.get(&id).ok_or_else(|| "Document matching query does not exist.".to_string())?;
            overrides = overrides_from_document(&self.pool, meta).await?;
            overrides.title = Some(format!("{} (merged)", meta.title));
            if meta.archive_serial_number.is_some() {
                handoff = meta.archive_serial_number;
            }
        }
        overrides.owner_id = self.user_id;
        if !delete_originals {
            overrides.skip_asn_if_exists = true;
        }
        if delete_originals && handoff.is_some() {
            overrides.asn = handoff;
        }
        tracing::info!(target: LOG, "Adding merged document to the task queue.");
        let sig = self.consume_sig(&merged_file, None, &overrides)?;
        if delete_originals {
            let backup = release_archive_serial_numbers(&self.pool, &added).await?;
            tracing::info!(target: LOG, "Queueing removal of original documents after consumption of merged document");
            if let Err(e) = self.apply_async(sig, vec![delete_sig(&added)], vec![restore_sig(&backup)]).await {
                restore_archive_serial_numbers(&self.pool, &backup_restore_list(&backup)).await?;
                return Err(e);
            }
        } else {
            self.apply_async(sig, Vec::new(), Vec::new()).await?;
        }
        Ok(())
    }

    /// `bulk_edit.split`
    async fn split(&self, doc_ids: &[i64], pages: &[Vec<i64>], delete_originals: bool) -> ExecResult {
        let first = *doc_ids.first().ok_or("IndexError: list index out of range")?;
        tracing::info!(target: LOG, "Attempting to split document {first} into {} documents", pages.len());
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let doc = get_doc(&mut conn, first).await?;
        let pair = resolve_pair(&mut conn, &doc, self.mode).await?;
        drop(conn);
        let r: ExecResult = async {
            let file_names: Vec<String> =
                pages.iter().map(|p| names::split(doc.id, p.first().copied().unwrap_or(0), p.last().copied().unwrap_or(0))).collect();
            let outputs: Vec<Vec<PageSpec>> = pages.iter().map(|p| p.iter().map(|n| PageSpec::new(*n)).collect()).collect();
            let (src, scratch) = (self.media.checked_source_path(&pair.source)?, self.scratch.clone());
            let files = blocking(move || pops::build_pdfs(&src, &outputs, |i| pbulk::scratch_path(&scratch, &file_names[i]))).await?;
            let mut tasks = Vec::new();
            for (idx, (split_doc, file)) in pages.iter().zip(files.iter()).enumerate() {
                let mut o = overrides_from_document(&self.pool, &doc).await?;
                o.title = Some(format!("{} (split {})", doc.title, idx + 1));
                o.owner_id = self.user_id;
                if !delete_originals {
                    o.skip_asn_if_exists = true;
                }
                tracing::info!(target: LOG, "Adding split document with pages {} to the task queue.", py_list(split_doc));
                tasks.push(self.consume_sig(file, None, &o)?);
            }
            if delete_originals {
                tracing::info!(target: LOG, "Queueing removal of original document after consumption of the split documents");
                self.chord_with_release(tasks, doc.id).await
            } else {
                self.group(tasks).await
            }
        }
        .await;
        if let Err(e) = r {
            tracing::error!(target: LOG, "Error splitting document {}: {e}", doc.id);
        }
        Ok(())
    }

    /// `bulk_edit.delete_pages`
    async fn delete_pages(&self, doc_ids: &[i64], pages: &[i64]) -> ExecResult {
        tracing::info!(target: LOG, "Attempting to delete pages {} from {} documents", py_list(pages), doc_ids.len());
        let first = *doc_ids.first().ok_or("IndexError: list index out of range")?;
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let doc = get_doc(&mut conn, first).await?;
        let pair = resolve_pair(&mut conn, &doc, self.mode).await?;
        drop(conn);
        let root = pair.root.id;
        let r: ExecResult = async {
            let path = self.scratch_path(&names::pages_deleted(root)).map_err(|e| e.to_string())?;
            let (src, dst, pages_v) = (self.media.checked_source_path(&pair.source)?, path.clone(), pages.to_vec());
            blocking(move || pops::remove_pages(&src, &dst, &pages_v)).await?;
            let mut o = overrides_from_document(&self.pool, &pair.root).await?;
            o.actor_id = self.user_id;
            self.apply_async(self.consume_sig(&path, Some(root), &o)?, Vec::new(), Vec::new()).await?;
            tracing::info!(target: LOG, "Queued new version for document {root} after deleting pages {}", py_list(pages));
            Ok(())
        }
        .await;
        if let Err(e) = r {
            tracing::error!(target: LOG, "Error deleting pages from document {root}: {e}");
        }
        Ok(())
    }

    async fn metadata_overrides(&self, root: &DocRow, include_metadata: bool) -> Result<DocumentMetadataOverrides, String> {
        let mut o = if include_metadata { overrides_from_document(&self.pool, root).await? } else { DocumentMetadataOverrides::default() };
        o.owner_id = self.user_id;
        o.actor_id = self.user_id;
        Ok(o)
    }

    /// `bulk_edit.edit_pdf`
    async fn edit_pdf(
        &self,
        doc_ids: &[i64],
        operations: &[PageOperation],
        delete_original: bool,
        update_document: bool,
        include_metadata: bool,
    ) -> ExecResult {
        let first = *doc_ids.first().ok_or("IndexError: list index out of range")?;
        tracing::info!(target: LOG, "Editing PDF of document {first} with {} operations", operations.len());
        let mut conn = self.pool.acquire().await.map_err(db)?;
        let doc = get_doc(&mut conn, first).await?;
        let pair = resolve_pair(&mut conn, &doc, self.mode).await?;
        drop(conn);
        let root = pair.root.id;
        let r: ExecResult = async {
            let output_count = pops::validate_page_operations(operations, update_document).map_err(|e| e.to_string())?;
            let specs = pops::page_specs_for(operations, output_count);
            let (src, scratch) = (self.media.checked_source_path(&pair.source)?, self.scratch.clone());
            if update_document {
                let one = vec![specs.into_iter().next().unwrap_or_default()];
                let files = blocking(move || pops::build_pdfs(&src, &one, |_| pbulk::scratch_path(&scratch, &names::edited(root)))).await?;
                let file = files.first().ok_or("ValueError: not enough values to unpack")?;
                let o = self.metadata_overrides(&pair.root, include_metadata).await?;
                self.apply_async(self.consume_sig(file, Some(root), &o)?, Vec::new(), Vec::new()).await
            } else {
                let files =
                    blocking(move || pops::build_pdfs(&src, &specs, |i| pbulk::scratch_path(&scratch, &names::edit_part(root, i + 1))))
                        .await?;
                let mut o = self.metadata_overrides(&pair.root, include_metadata).await?;
                if !delete_original {
                    o.skip_asn_if_exists = true;
                }
                if delete_original && output_count == 1 {
                    o.asn = pair.root.archive_serial_number;
                }
                let mut tasks = Vec::new();
                for f in &files {
                    tasks.push(self.consume_sig(f, None, &o)?);
                }
                if delete_original { self.chord_with_release(tasks, doc.id).await } else { self.group(tasks).await }
            }
        }
        .await;
        r.map_err(|e| {
            tracing::error!(target: LOG, "Error editing document {root}: {e}");
            format!("An error occurred while editing the document: {e}")
        })
    }

    /// `bulk_edit.remove_password`
    async fn remove_password(
        &self,
        doc_ids: &[i64],
        password: &str,
        update_document: bool,
        delete_original: bool,
        include_metadata: bool,
        source_paths_by_id: Option<&Value>,
    ) -> ExecResult {
        for id in doc_ids {
            let mut conn = self.pool.acquire().await.map_err(db)?;
            let doc = get_doc(&mut conn, *id).await?;
            let pair = resolve_pair(&mut conn, &doc, self.mode).await?;
            drop(conn);
            let root = pair.root.id;
            let r: ExecResult = async {
                tracing::info!(target: LOG, "Attempting password removal from document {root}");
                // (source_paths_by_id or {}).get(doc.id, …): JSON keys are strings → never a hit
                match source_paths_by_id {
                    Some(v) if py_truthy(v) && !v.is_object() => {
                        return Err(format!("'{}' object has no attribute 'get'", drf::py_type_name(v)));
                    }
                    _ => {}
                }
                let src = self.media.checked_source_path(&pair.source)?;
                let s2 = src.clone();
                if !blocking(move || pops::needs_decrypt(&s2)).await? {
                    tracing::info!(target: LOG, "Skipping password removal for document {root} because the source PDF is not encrypted");
                    return Ok(());
                }
                let (scratch, pw) = (self.scratch.clone(), password.to_string());
                let file =
                    blocking(move || pops::decrypt_pdf(&src, || pbulk::scratch_path(&scratch, &names::unprotected(root)), &pw)).await?;
                let o = self.metadata_overrides(&pair.root, include_metadata).await?;
                if update_document {
                    self.apply_async(self.consume_sig(&file, Some(root), &o)?, Vec::new(), Vec::new()).await
                } else {
                    let tasks = vec![self.consume_sig(&file, None, &o)?];
                    if delete_original { self.chord(tasks, delete_sig(&[doc.id]), None).await } else { self.group(tasks).await }
                }
            }
            .await;
            if let Err(e) = r {
                tracing::error!(target: LOG, "Error removing password from document {root}: {e}");
                return Err(format!("An error occurred while removing the password: {e}"));
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ Worker tasks

/// `restore_archive_serial_numbers_task(self, backup, *args, **kwargs)`: `backup` = first
/// positional argument (or kwarg). As `link_error`/`on_error` Celery puts `(request, exc,
/// traceback)` **before** its own arguments – upstream then gets the request as `backup`
/// and fails (`AttributeError`); we reproduce that: a `backup` whose keys are not
/// document ids is an error.
pub async fn restore_asn_task(pool: &PgPool, args: &[Value], kwargs: &Map<String, Value>) -> Result<(), (String, String)> {
    let Some(backup) = kwargs.get("backup").or_else(|| args.first()) else {
        return Err(("TypeError".into(), "restore_archive_serial_numbers_task() missing 1 required positional argument: 'backup'".into()));
    };
    let Value::Object(m) = backup else {
        return Err(("AttributeError".into(), format!("'{}' object has no attribute 'items'", drf::py_type_name(backup))));
    };
    let mut list = Vec::new();
    for (k, v) in m {
        let Some(id) = drf::py_int_from_str(k) else {
            // upstream: request object (Celery Context) has no .items()
            return Err(("AttributeError".into(), "'Context' object has no attribute 'items'".into()));
        };
        let asn = match v {
            Value::Null => None,
            other => Some(drf::drf_int(other).map_err(|e| ("ValueError".to_string(), e.to_string()))?),
        };
        list.push((id, asn));
    }
    restore_archive_serial_numbers(pool, &list).await.map_err(|e| ("OperationalError".into(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pages() {
        assert_eq!(parse_split_pages("1-2,3", 3), Some(vec![vec![1, 2], vec![3]]));
        assert_eq!(parse_split_pages(" 2 ", 3), Some(vec![vec![2]]));
        assert_eq!(parse_split_pages("1-4", 3), None);
        assert_eq!(parse_split_pages("2-1", 3), None);
        assert_eq!(parse_split_pages("0", 3), None);
        assert_eq!(parse_split_pages("", 3), None);
        assert_eq!(parse_split_pages("1-", 3), None);
        assert_eq!(parse_split_pages("1-2-3", 3), None);
        assert_eq!(parse_split_pages("a", 3), None);
    }

    #[test]
    fn operations_like_drf() {
        let ok = validate_operations(&json!([{"page": 1}, {"page": "2", "rotate": 90, "doc": 1}])).unwrap();
        assert_eq!(ok[1], PageOperation { page: 2, rotate: Some(90), doc: Some(1) });
        assert_eq!(validate_operations(&json!([])).unwrap_err(), json!(["This list may not be empty."]));
        assert_eq!(validate_operations(&json!("x")).unwrap_err(), json!(["Expected a list of items but got type \"str\"."]));
        assert_eq!(validate_operations(&json!({"a": 1})).unwrap_err(), json!(["Expected a list of items but got type \"dict\"."]));
        assert_eq!(validate_operations(&Value::Null).unwrap_err(), json!(["This field may not be null."]));
        let e = validate_operations(&json!([{"page": 1}, {"page": 0, "rotate": 45, "doc": -1}, 5, null, {}])).unwrap_err();
        assert_eq!(
            e,
            json!({
                "1": {"page": ["Ensure this value is greater than or equal to 1."], "rotate": ["rotate must be a multiple of 90"],
                      "doc": ["Ensure this value is greater than or equal to 0."]},
                "2": {"non_field_errors": ["Invalid data. Expected a dictionary, but got int."]},
                "3": ["This field may not be null."],
                "4": {"page": ["This field is required."]}
            })
        );
        let e = validate_operations(&json!([{"page": true, "rotate": null}])).unwrap_err();
        assert_eq!(e, json!({"0": {"page": ["A valid integer is required."], "rotate": ["This field may not be null."]}}));
    }

    #[test]
    fn source_modes() {
        assert_eq!(parse_source_mode(Some(&json!("explicit_selection"))), Some(SourceMode::ExplicitSelection));
        assert_eq!(parse_source_mode(Some(&json!("documents.bulk_edit"))), Some(SourceMode::LatestVersion));
        assert_eq!(parse_source_mode(Some(&Value::Null)), Some(SourceMode::LatestVersion));
        assert!(parse_source_mode(Some(&json!("x"))).is_none());
        assert!(parse_source_mode(Some(&json!(1))).is_none());
    }

    #[test]
    fn permission_needs() {
        let n = PdfParams::Merge { metadata_document_id: None, delete_originals: true, archive_fallback: false }.needs();
        assert!(n.ownership && n.add_document && n.delete_document);
        let n = PdfParams::Split { pages: vec![], delete_originals: false }.needs();
        assert!(!n.ownership && n.add_document && !n.delete_document);
        let n = PdfParams::EditPdf { operations: vec![], delete_original: true, update_document: true, include_metadata: true }.needs();
        assert!(n.ownership && !n.add_document && !n.delete_document);
        let n = PdfParams::RemovePassword {
            password: String::new(),
            update_document: false,
            delete_original: true,
            include_metadata: true,
            source_paths_by_id: None,
        }
        .needs();
        assert!(n.ownership && n.add_document && n.delete_document);
        assert!(PdfParams::Rotate { degrees: 90 }.needs().ownership);
    }

    #[test]
    fn signatures() {
        let d = delete_sig(&[4, 5]);
        assert!(d.immutable);
        assert_eq!(d.args, vec![json!([4, 5])]);
        let r = restore_sig(&[(4, 1004)]);
        assert!(!r.immutable);
        assert_eq!(r.args, vec![json!({"4": 1004})]);
    }

    #[tokio::test]
    async fn restore_task_rejects_request_as_backup() {
        // without DB: error before any query
        let pool = PgPool::connect_lazy("postgres://x:y@127.0.0.1:1/none").unwrap();
        let req = json!({"id": "t", "task": "documents.tasks.consume_file", "args": [], "kwargs": {}, "group": null, "retries": 0});
        let e = restore_asn_task(&pool, &[req, json!({"exc_type": "X"}), Value::Null, json!({"4": 1004})], &Map::new()).await.unwrap_err();
        assert_eq!(e.0, "AttributeError");
        let e = restore_asn_task(&pool, &[], &Map::new()).await.unwrap_err();
        assert_eq!(e.0, "TypeError");
    }
}
