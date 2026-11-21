// PaperRust-specific (no upstream counterpart): PDF tools redact and shrink based
// on PdfCraft (pl-pdftools, child process). Contract: docs/contract/pl-pdftools.md §4.
// Pattern: src/documents/views.py@v3.4.2 DocumentViewSet.update_version (permissions, new version via
// consume_file with root_document_id), bulk_edit.py edit_pdf (source = latest version).
//
//! * `POST /api/documents/{id}/redact/search/` – find search terms (hits with rectangles).
//! * `POST /api/documents/{id}/redact/` – redact areas/terms → new version.
//! * `POST /api/documents/{id}/shrink/` – shrink → new version (only if smaller).
//!
//! Permissions: `documents.change_document` (model) + `change_document` on the root document
//! (object/owner-based, `has_perms_owner_aware`) – like `update_version`. The PDF work
//! runs in `paperrust-pdftool` (time/memory limit, separate process); this process only reads
//! paths within ORIGINALS_DIR/ARCHIVE_DIR (`checked_*`).

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use pl_authz::AuthUser;
use pl_authz::models::DOCUMENT;
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use pl_consumer::db::DocRow;
use pl_core::{AppState, RequestContext};
use pl_pdftools::job::{Area, Hit, Op, Reply, ShrinkPreset};
use pl_pdftools::runner::RunError;

use crate::actions::char_field;
use crate::docs;
use crate::drf::{self, Errors};
use crate::error::{ApiError, ApiResult};
use crate::files::MediaCfg;
use crate::update::parse_pk;

const LOG: &str = "paperrust.pdftools";
/// Maximum number of areas / terms per request.
const MAX_AREAS: usize = 2000;
const MAX_TERMS: usize = 50;
const MAX_TERM_LEN: usize = 200;

pub(crate) fn not_found() -> ApiError {
    ApiError::Core(pl_core::ApiError::not_found())
}

pub(crate) fn detail(status: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError::Core(pl_core::ApiError::with_detail(status, msg))
}

pub(crate) fn run_error(e: RunError) -> ApiError {
    tracing::warn!(target: LOG, "{e}");
    let status = match e {
        RunError::Failed(_) => StatusCode::BAD_REQUEST,
        RunError::NotInstalled => StatusCode::SERVICE_UNAVAILABLE,
        RunError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    detail(status, e.user_message())
}

pub(crate) fn ok(ctx: &RequestContext, v: Value) -> Response {
    pl_core::Drf::ok(v).indent(ctx.negotiation.indent).into_response()
}

// ------------------------------------------------------------------ Inputs

/// Areas `[{"page": 1, "rect": [x0, y0, x1, y1]}]` (pages 1-based in the API).
pub fn parse_areas(v: Option<&Value>) -> Result<Vec<Area>, Value> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let Value::Array(items) = v else { return Err(drf::msg("Expected a list of areas.")) };
    if items.len() > MAX_AREAS {
        return Err(drf::msg(format!("At most {MAX_AREAS} areas are allowed.")));
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let bad = || drf::msg(format!("Area {}: expected {{\"page\": n, \"rect\": [x0, y0, x1, y1]}}.", i + 1));
        let Value::Object(m) = it else { return Err(bad()) };
        let page = m.get("page").and_then(Value::as_i64).filter(|p| *p >= 1).ok_or_else(bad)?;
        let rect = m.get("rect").and_then(Value::as_array).filter(|a| a.len() == 4).ok_or_else(bad)?;
        let mut r = [0.0f64; 4];
        for (k, n) in rect.iter().enumerate() {
            r[k] = n.as_f64().filter(|f| f.is_finite()).ok_or_else(bad)?;
        }
        if !pl_pdftools::matching::valid_rect(&r) {
            return Err(drf::msg(format!("Area {}: the rectangle has no area.", i + 1)));
        }
        out.push(Area { page: (page - 1) as usize, rect: r });
    }
    Ok(out)
}

/// Search terms (list of texts, empty ones are discarded).
pub fn parse_terms(v: Option<&Value>) -> Result<Vec<String>, Value> {
    let Some(v) = v else { return Ok(Vec::new()) };
    let Value::Array(items) = v else { return Err(drf::msg("Expected a list of search terms.")) };
    if items.len() > MAX_TERMS {
        return Err(drf::msg(format!("At most {MAX_TERMS} search terms are allowed.")));
    }
    let mut out = Vec::new();
    for it in items {
        let s = char_field(it, true, Some(MAX_TERM_LEN))?;
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(out)
}

pub(crate) fn bool_of(m: &Map<String, Value>, key: &str, errors: &mut Errors) -> bool {
    match m.get(key) {
        None => false,
        Some(v) => drf::boolean(v).unwrap_or_else(|e| {
            errors.add(key, e);
            false
        }),
    }
}

pub(crate) fn label_of(m: &Map<String, Value>, errors: &mut Errors) -> Option<String> {
    match m.get("version_label") {
        None | Some(Value::Null) => None,
        Some(v) => match char_field(v, true, Some(64)) {
            Ok(s) => Some(s).filter(|s| !s.is_empty()),
            Err(e) => {
                errors.add("version_label", e);
                None
            }
        },
    }
}

pub(crate) fn body_map(ctx: &RequestContext, body: &axum::body::Bytes) -> ApiResult<Map<String, Value>> {
    if body.is_empty() {
        return Ok(Map::new());
    }
    match crate::views::parse_json_body(&ctx.headers, body)? {
        Value::Object(m) => Ok(m),
        other => Err(ApiError::Validation(drf::non_field(crate::views::not_a_dict_message(&other)))),
    }
}

// ------------------------------------------------------------------ Document and permissions

pub(crate) struct Target {
    pub(crate) root: DocRow,
    pub(crate) source: DocRow,
}

/// Root + latest version (like `_resolve_root_and_source_doc` with `latest_version`), with
/// permission check `change_document`; the latest version must be a PDF.
async fn target(state: &AppState, auth: &AuthUser, pk: &str) -> ApiResult<Target> {
    let t = target_any(state, auth, pk).await?;
    if t.source.mime_type != "application/pdf" {
        return Err(detail(StatusCode::BAD_REQUEST, "This document is not a PDF."));
    }
    Ok(t)
}

/// Like [`target`], without format check (for "Convert to PDF").
pub(crate) async fn target_any(state: &AppState, auth: &AuthUser, pk: &str) -> ApiResult<Target> {
    auth.context().require_model_perms(&DOCUMENT).map_err(|e| ApiError::Core(e.into()))?;
    let Some(id) = parse_pk(pk) else { return Err(not_found()) };
    let mut conn = state.db().acquire().await?;
    let Some(doc) = docs::load_live(&mut conn, id).await? else { return Err(not_found()) };
    let root = match doc.root_document_id {
        None => doc,
        Some(r) => docs::load_global(&mut conn, r).await?.unwrap_or(doc),
    };
    // FINDINGS B2: read the latest version still with the held connection, then return the connection
    // – the permission check fetches its own from the pool (never hold two at the same time).
    let source = latest_version(&mut conn, &root).await?;
    drop(conn);
    let user = auth.user();
    let allowed = user.has_perm("documents.change_document")
        && has_perms_owner_aware(state.db(), Some(user), "change_document", &DOCUMENT, ObjRef { id: root.id, owner_id: root.owner_id })
            .await
            .map_err(|e| ApiError::Core(e.into()))?;
    if !allowed {
        return Err(ApiError::ForbiddenText("Insufficient permissions"));
    }
    Ok(Target { root, source })
}

async fn latest_version(conn: &mut PgConnection, root: &DocRow) -> ApiResult<DocRow> {
    let latest: Option<i32> = sqlx::query_scalar(
        "SELECT id FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL \
         ORDER BY version_index DESC NULLS LAST, id DESC LIMIT 1",
    )
    .bind(root.id as i32)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(match latest {
        Some(id) => docs::load_global(conn, i64::from(id)).await?.unwrap_or_else(|| root.clone()),
        None => root.clone(),
    })
}

pub(crate) fn paths(state: &AppState, d: &DocRow) -> ApiResult<(PathBuf, Option<PathBuf>)> {
    let media = MediaCfg::from_config(&state.live_config());
    let src = media.checked_source_path(d).map_err(ApiError::Uncaught)?;
    let archive = media.checked_archive_path(d).map_err(ApiError::Uncaught)?.filter(|p| p.is_file());
    Ok((src, archive))
}

pub(crate) fn s(p: &FsPath) -> String {
    p.to_string_lossy().into_owned()
}

fn hit_json(h: &Hit) -> Value {
    json!({"page": h.page + 1, "term": h.term, "context": h.context, "rects": h.rects})
}

/// Search: first in the original; if nothing is found there and there is an archive PDF (OCR text layer
/// of a scan), then there. Page geometry of original and archive is the same (OCRmyPDF).
async fn search_both(
    src: &FsPath,
    archive: Option<&FsPath>,
    terms: &[String],
    max_hits: usize,
) -> Result<(Vec<Hit>, bool, &'static str), RunError> {
    let r = pl_pdftools::runner::run(Op::Search { input: s(src), terms: terms.to_vec(), max_hits }).await?;
    let Reply::Hits { hits, truncated } = r else { return Err(RunError::Protocol("unexpected reply".into())) };
    if !hits.is_empty() || archive.is_none() {
        return Ok((hits, truncated, "original"));
    }
    let Some(a) = archive else { return Ok((hits, truncated, "original")) };
    let r = pl_pdftools::runner::run(Op::Search { input: s(a), terms: terms.to_vec(), max_hits }).await?;
    let Reply::Hits { hits, truncated } = r else { return Err(RunError::Protocol("unexpected reply".into())) };
    Ok((hits, truncated, "archive"))
}

/// `consume_file` with `root_document_id` (new version), returns the task id.
pub(crate) async fn queue_version(state: &AppState, user_id: i64, root_id: i64, file: &FsPath, label: Option<String>) -> ApiResult<String> {
    let Some(queue) = crate::pdf_edit::pg_queue(state) else {
        return Err(detail(StatusCode::SERVICE_UNAVAILABLE, "New versions need the PaperRust task queue."));
    };
    let r: Result<uuid::Uuid, String> = async {
        let mut input = pl_consumer::ConsumableDocument::new(pl_consumer::DocumentSource::ApiUpload, file).map_err(|e| e.to_string())?;
        input.root_document_id = Some(root_id);
        let overrides = pl_consumer::DocumentMetadataOverrides {
            version_label: label,
            owner_id: Some(user_id),
            actor_id: Some(user_id),
            ..Default::default()
        };
        let kwargs = pl_consumer::task::consume_kwargs(&input, &overrides)?;
        let opts =
            pl_queue::EnqueueOptions { trigger_source: Some(pl_core::tasks::trigger_source::WEB_UI.to_string()), ..Default::default() };
        queue.enqueue(crate::pdf_edit::CONSUME_FILE, kwargs, opts).await.map_err(|e| e.to_string())
    }
    .await;
    r.map(|u| u.to_string()).map_err(|e| {
        tracing::warn!(target: LOG, "queueing new version failed: {e}");
        ApiError::Uncaught(e)
    })
}

/// File name of the new version: `<stem>-<suffix>.pdf` (stem from the original name).
pub fn output_name(original: Option<&str>, id: i64, suffix: &str) -> String {
    let stem = original
        .and_then(|o| FsPath::new(o).file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .map(|s| pl_templating::sanitize_filename(&s, ""))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{id:07}"));
    format!("{stem}-{suffix}.pdf")
}

pub(crate) fn scratch_file(state: &AppState, name: &str) -> ApiResult<PathBuf> {
    pl_pdfops::bulk::scratch_path(&state.config().scratch_dir, name).map_err(|e| ApiError::Uncaught(e.to_string()))
}

pub(crate) fn cleanup(p: &FsPath) {
    let _ = std::fs::remove_file(p);
    if let Some(d) = p.parent() {
        let _ = std::fs::remove_dir(d);
    }
}

// ------------------------------------------------------------------ redact/search/

/// POST /api/documents/{id}/redact/search/ `{"terms": [...]}`
pub async fn redact_search_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    match redact_search(&state, &ctx, &auth, &pk, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn redact_search(state: &AppState, ctx: &RequestContext, auth: &AuthUser, pk: &str, body: &axum::body::Bytes) -> ApiResult<Response> {
    let m = body_map(ctx, body)?;
    let terms = parse_terms(m.get("terms")).map_err(|e| ApiError::Validation(json!({ "terms": e })))?;
    if terms.is_empty() {
        return Err(ApiError::Validation(json!({"terms": [drf::REQUIRED]})));
    }
    let t = target(state, auth, pk).await?;
    let (src, archive) = paths(state, &t.source)?;
    let (hits, truncated, from) = search_both(&src, archive.as_deref(), &terms, 500).await.map_err(run_error)?;
    Ok(ok(
        ctx,
        json!({"document": t.root.id, "version": t.source.id, "source": from, "truncated": truncated, "hits": hits.iter().map(hit_json).collect::<Vec<_>>()}),
    ))
}

// ------------------------------------------------------------------ redact/

/// POST /api/documents/{id}/redact/ `{"areas": [...], "terms": [...], "remove_metadata": bool, "version_label": str}`
pub async fn redact_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    match redact(&state, &ctx, &auth, &pk, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn redact(state: &AppState, ctx: &RequestContext, auth: &AuthUser, pk: &str, body: &axum::body::Bytes) -> ApiResult<Response> {
    let m = body_map(ctx, body)?;
    let mut errors = Errors::default();
    let mut areas = parse_areas(m.get("areas")).unwrap_or_else(|e| {
        errors.add("areas", e);
        Vec::new()
    });
    let terms = parse_terms(m.get("terms")).unwrap_or_else(|e| {
        errors.add("terms", e);
        Vec::new()
    });
    let remove_metadata = bool_of(&m, "remove_metadata", &mut errors);
    let label = label_of(&m, &mut errors).or_else(|| Some("Redacted".to_string()));
    if !errors.is_empty() {
        return Err(ApiError::Validation(Value::Object(errors.0)));
    }
    if areas.is_empty() && terms.is_empty() && !remove_metadata {
        return Err(ApiError::Validation(drf::non_field("Nothing to redact: give areas, search terms or remove_metadata.")));
    }
    let t = target(state, auth, pk).await?;
    let (src, archive) = paths(state, &t.source)?;
    let mut tool_terms = terms.clone();
    if !terms.is_empty() {
        // Terms only in the archive (scan with OCR text): hits there as areas on the original
        let (hits, truncated, from) = search_both(&src, archive.as_deref(), &terms, 10_000).await.map_err(run_error)?;
        if truncated {
            return Err(detail(StatusCode::BAD_REQUEST, "Too many matches for the search terms; nothing was changed."));
        }
        if from == "archive" {
            for h in &hits {
                areas.extend(h.rects.iter().map(|r| Area { page: h.page, rect: *r }));
            }
            tool_terms.clear();
            if areas.is_empty() && !remove_metadata {
                return Err(detail(StatusCode::BAD_REQUEST, "The search terms were not found; nothing was changed."));
            }
        }
    }
    let out = scratch_file(state, &output_name(t.source.original_filename.as_deref(), t.root.id, "redacted"))?;
    let op = Op::Redact { input: s(&src), output: s(&out), areas, terms: tool_terms, remove_metadata };
    let report = match pl_pdftools::runner::run(op).await {
        Ok(Reply::Redacted(r)) => r,
        Ok(_) => {
            cleanup(&out);
            return Err(run_error(RunError::Protocol("unexpected reply".into())));
        }
        Err(e) => {
            cleanup(&out);
            return Err(run_error(e));
        }
    };
    // Watch list (§4.1): checksum of the redacted file before it is ingested as a new version.
    // Errors here do not abort – without an entry the version counts as not redacted when sharing.
    let sum_path = out.clone();
    match tokio::task::spawn_blocking(move || pl_consumer::compute_checksum(&sum_path)).await.map_err(std::io::Error::other).and_then(|r| r)
    {
        Ok(sum) => {
            if let Err(e) = crate::redaction_marks::record(state.db(), t.root.id, t.source.id, &sum, auth.user().id).await {
                tracing::warn!(target: LOG, "recording redaction mark for document {} failed: {e}", t.root.id);
            }
        }
        Err(e) => tracing::warn!(target: LOG, "checksum of redacted file for document {} failed: {e}", t.root.id),
    }
    let task_id = match queue_version(state, auth.user().id, t.root.id, &out, label).await {
        Ok(id) => id,
        Err(e) => {
            cleanup(&out);
            return Err(e);
        }
    };
    tracing::info!(target: LOG, "Redacted document {} (version {}): {} marks, {} glyphs; new version queued", t.root.id, t.source.id, report.marks, report.glyphs);
    Ok(ok(ctx, json!({"task_id": task_id, "document": t.root.id, "source_version": t.source.id, "report": report})))
}

// ------------------------------------------------------------------ shrink/

/// POST /api/documents/{id}/shrink/ `{"preset": "lossless"|"standard"|"strong", "version_label": str}`
pub async fn shrink_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    match shrink(&state, &ctx, &auth, &pk, &body).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn shrink(state: &AppState, ctx: &RequestContext, auth: &AuthUser, pk: &str, body: &axum::body::Bytes) -> ApiResult<Response> {
    let m = body_map(ctx, body)?;
    let mut errors = Errors::default();
    let preset = match m.get("preset") {
        None | Some(Value::Null) => ShrinkPreset::Standard,
        Some(Value::String(p)) => ShrinkPreset::parse(p).unwrap_or_else(|| {
            errors.add("preset", drf::msg(format!("\"{p}\" is not a valid choice.")));
            ShrinkPreset::Standard
        }),
        Some(_) => {
            errors.add("preset", drf::msg("Not a valid string."));
            ShrinkPreset::Standard
        }
    };
    let label = label_of(&m, &mut errors).or_else(|| Some("Shrunk".to_string()));
    if !errors.is_empty() {
        return Err(ApiError::Validation(Value::Object(errors.0)));
    }
    let t = target(state, auth, pk).await?;
    let (src, _) = paths(state, &t.source)?;
    let out = scratch_file(state, &output_name(t.source.original_filename.as_deref(), t.root.id, "small"))?;
    let report = match pl_pdftools::runner::run(Op::Shrink { input: s(&src), output: s(&out), preset }).await {
        Ok(Reply::Shrunk(r)) => r,
        Ok(_) => {
            cleanup(&out);
            return Err(run_error(RunError::Protocol("unexpected reply".into())));
        }
        Err(e) => {
            cleanup(&out);
            return Err(run_error(e));
        }
    };
    if report.size_after >= report.size_before {
        cleanup(&out);
        return Ok(ok(ctx, json!({"task_id": null, "document": t.root.id, "new_version": false, "report": report})));
    }
    let task_id = match queue_version(state, auth.user().id, t.root.id, &out, label).await {
        Ok(id) => id,
        Err(e) => {
            cleanup(&out);
            return Err(e);
        }
    };
    tracing::info!(target: LOG, "Shrunk document {} from {} to {} bytes; new version queued", t.root.id, report.size_before, report.size_after);
    Ok(ok(ctx, json!({"task_id": task_id, "document": t.root.id, "new_version": true, "report": report})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_are_validated_and_one_based() {
        let a = parse_areas(Some(&json!([{"page": 2, "rect": [1, 2, 30.5, 40]}]))).unwrap();
        assert_eq!(a, vec![Area { page: 1, rect: [1.0, 2.0, 30.5, 40.0] }]);
        assert!(parse_areas(Some(&json!([{"page": 0, "rect": [1, 2, 3, 4]}]))).is_err());
        assert!(parse_areas(Some(&json!([{"page": 1, "rect": [1, 2, 3]}]))).is_err());
        assert!(parse_areas(Some(&json!([{"page": 1, "rect": [1, 2, 1, 4]}]))).is_err());
        assert!(parse_areas(Some(&json!({"page": 1}))).is_err());
        assert_eq!(parse_areas(None).unwrap(), vec![]);
    }

    #[test]
    fn terms_trimmed_deduplicated_limited() {
        assert_eq!(parse_terms(Some(&json!([" a ", "a", "", "b"]))).unwrap(), vec!["a", "b"]);
        assert!(parse_terms(Some(&json!("a"))).is_err());
        assert!(parse_terms(Some(&json!(["x".repeat(201)]))).is_err());
        let many: Vec<String> = (0..51).map(|i| i.to_string()).collect();
        assert!(parse_terms(Some(&json!(many))).is_err());
    }

    #[test]
    fn missing_tool_is_503_with_clear_message() {
        let ApiError::Core(e) = run_error(RunError::NotInstalled) else { panic!("expected core error") };
        assert_eq!(e.status(), StatusCode::SERVICE_UNAVAILABLE);
        let detail = e.body_json().and_then(|b| b.get("detail")).and_then(Value::as_str).unwrap_or_default().to_string();
        assert!(detail.contains("not installed") && detail.contains("paperrust-pdftool"), "{detail}");
        let ApiError::Core(e) = run_error(RunError::Timeout(5)) else { panic!("expected core error") };
        assert_eq!(e.status(), StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn output_names() {
        assert_eq!(output_name(Some("Rechnung März.pdf"), 7, "redacted"), "Rechnung März-redacted.pdf");
        assert_eq!(output_name(None, 7, "small"), "0000007-small.pdf");
        assert_eq!(output_name(Some("../../x.pdf"), 7, "small"), "x-small.pdf");
    }
}
