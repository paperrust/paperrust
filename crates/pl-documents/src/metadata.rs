// upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.metadata, get_metadata, get_filesize,
//           _resolve_request_and_root_doc, _get_effective_file_doc, _resolve_file_doc),
//           src/documents/conditionals.py@v3.4.2 (metadata_etag, metadata_last_modified),
//           src/documents/serialisers.py@v3.4.2 (DocumentBarcodeSerializer),
//           src/documents/caching.py@v3.4.2 (get/set/refresh_metadata_cache – not used here),
//           Django 5.2 django/views/decorators/http.py (condition), cache.py (cache_control)
//
//! `GET /api/documents/{id}/metadata/`.
//!
//! File resolution (versions, `?version=`), paths, permissions and `condition()` come from pl-files
//! (the same functions as `preview`/`thumb`); metadata from pl-parsers
//! (`ParserRegistry::get_parser_for_file` + `extract_metadata`), language via [`crate::langdetect`].
//! The metadata cache of the Django cache is neither read nor written (Known difference).

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use pl_authz::AuthUser;
use pl_authz::models::DOCUMENT;
use pl_core::{ApiError, AppState, Drf, RequestContext};
use pl_files::db::DocRow;
use pl_files::docfile::{self, Ctx};
use pl_files::resolve::{self, Params, VersionError};
use serde_json::{Value, json};
use sqlx::Row;

type ApiResult<T> = Result<T, ApiError>;

/// `Model.objects.filter(pk=...)`: `int(pk)` – invalid → ValueError → 500; outside the
/// value range → no row (`None`).
pub(crate) fn lookup_pk(pk: &str) -> ApiResult<Option<i64>> {
    resolve::py_int(pk).map_err(|_| ApiError::internal(format!("Field 'id' expected a number but got '{pk}'.")))
}

/// Python `datetime.isoformat()` of a UTC timestamp from Django (`+00:00`, microseconds only if ≠ 0).
pub(crate) fn py_isoformat(t: &DateTime<Utc>) -> String {
    if t.timestamp_subsec_micros() == 0 {
        t.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        t.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

/// `get_metadata(file, mime_type)`: `None` without a file, `[]` without a parser.
fn get_metadata(path: Option<&FsPath>, mime_type: &str, tz: chrono_tz::Tz) -> Value {
    let Some(path) = path.filter(|p| p.is_file()) else {
        return Value::Null;
    };
    let registry = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match registry.get_parser_for_file(mime_type, &name, Some(path), true) {
        Some(p) => {
            let ctx = pl_parsers::ParseContext { time_zone: tz, ..Default::default() };
            guarded_extract(path, || serde_json::to_value(p.extract_metadata(path, mime_type, &ctx)).unwrap_or_else(|_| json!([])))
        }
        None => {
            tracing::warn!(target: "paperless.api", "No parser for {mime_type}");
            json!([])
        }
    }
}

/// `except Exception: logger.exception("Issue getting metadata for …"); return []` – a panic in the
/// parser (e.g. corrupted file) yields `[]` and an error message in the log instead of a 500
/// (final security review B-N4).
fn guarded_extract(path: &FsPath, f: impl FnOnce() -> Value) -> Value {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            tracing::error!(target: "paperless.api", "Issue getting metadata for {}", path.display());
            json!([])
        }
    }
}

/// `get_filesize(filename)`.
fn get_filesize(path: Option<&FsPath>) -> Option<u64> {
    let p = path?;
    let m = std::fs::metadata(p).ok()?;
    m.is_file().then_some(m.len())
}

/// `metadata_etag` / `metadata_last_modified`.
async fn etag_and_last_modified(state: &AppState, pk: Option<i64>, params: &Params) -> ApiResult<(Option<String>, Option<i64>)> {
    let Some(pk) = pk else { return Ok((None, None)) };
    let eff = resolve::effective_by_pk(state.db(), pk, params).await?;
    Ok(match eff {
        Some(d) => (Some(format!("{}:{}", d.checksum, py_isoformat(&d.modified))), Some(d.modified.timestamp())),
        None => (None, None),
    })
}

/// Result of a `condition()`-protected view: returned (runs through `condition` and
/// `cache_control`) or raised (DRF exception handler, without post-processing).
pub(crate) type ViewResult = Result<Response, ApiError>;

/// `cache_control(no_cache=True)(condition(etag, last_modified)(view))`.
pub(crate) async fn conditional<F, Fut>(
    method: &Method,
    headers: &HeaderMap,
    etag: Option<String>,
    last_modified: Option<i64>,
    view: F,
) -> Response
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ViewResult>,
{
    let etag = etag.map(|e| pl_files::conditional::quote_etag(&e));
    let pre = pl_files::conditional::evaluate(method, headers, etag.as_deref(), last_modified);
    let returned = match pl_files::conditional::precondition_response(&pre) {
        Some(r) => r,
        None => match view().await {
            Ok(r) => r,
            Err(e) => return e.into_response(),
        },
    };
    pl_files::conditional::finish(returned, method, etag.as_deref(), last_modified)
}

fn forbidden() -> Response {
    ApiError::django_text(StatusCode::FORBIDDEN, "Insufficient permissions").into_response()
}

/// GET /api/documents/{id}/metadata/
pub async fn metadata(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
    RawQuery(raw): RawQuery,
    method: Method,
    headers: HeaderMap,
) -> ApiResult<Response> {
    // initial(): IsAuthenticated + PaperlessObjectPermissions (GET → view_document)
    let user = auth.require_model_perms(&DOCUMENT)?.clone();
    let params = Params::parse(raw.as_deref());
    let pk = lookup_pk(&pk)?;
    let (etag, lm) = etag_and_last_modified(&state, pk, &params).await?;
    let indent = ctx.negotiation.indent;
    Ok(conditional(&method, &headers, etag, lm, || async move {
        let cx = Ctx::new(&state);
        let db = state.db();
        // _resolve_request_and_root_doc(pk, request) – Document.objects (without trash)
        let Some(pk) = pk else { return Err(ApiError::not_found()) };
        let Some(request_doc) = pl_files::db::load_doc(db, pk, false).await? else {
            return Err(ApiError::not_found());
        };
        let root = resolve::root_of(db, &request_doc).await?;
        if !pl_files::perms::has_view_on_root(db, &user, root.id, root.owner_id).await? {
            return Ok(forbidden());
        }
        let doc = match resolve::effective_file_doc(db, &request_doc, &root, &params).await? {
            Ok(d) => d,
            Err(VersionError::Invalid) => {
                return Err(ApiError::with_detail(StatusCode::NOT_FOUND, "Invalid version parameter"));
            }
            Err(VersionError::NotFound) => return Err(ApiError::not_found()),
        };
        let body = build(&state, cx, &doc).await?;
        Ok(Drf::ok(body).indent(indent).into_response())
    })
    .await)
}

async fn build(state: &AppState, cx: Ctx<'_>, doc: &DocRow) -> ApiResult<Value> {
    let db = state.db();
    let source = docfile::source_path(cx, doc).await?;
    let archive: Option<PathBuf> = if doc.has_archive_version() { docfile::archive_path(cx, doc) } else { None };
    let row =
        sqlx::query("SELECT original_filename, content FROM documents_document WHERE id = $1").bind(doc.id as i32).fetch_one(db).await?;
    let original_filename: Option<String> = row.try_get("original_filename")?;
    let content: String = row.try_get("content")?;
    let barcodes = sqlx::query("SELECT page, value, format FROM documents_documentbarcode WHERE document_id = $1 ORDER BY page, id")
        .bind(doc.id as i32)
        .fetch_all(db)
        .await?
        .iter()
        .map(|r| {
            Ok(json!({
                "page": r.try_get::<i32, _>("page")?,
                "value": r.try_get::<String, _>("value")?,
                "format": r.try_get::<String, _>("format")?,
            }))
        })
        .collect::<Result<Vec<Value>, sqlx::Error>>()?;

    let tz = *state.tz();
    let mime = doc.mime_type.clone();
    let has_archive = doc.has_archive_version();
    let (src, arc) = (source.clone(), archive.clone());
    let (original_metadata, archive_metadata, lang) = tokio::task::spawn_blocking(move || {
        let original = get_metadata(src.as_deref(), &mime, tz);
        let archive = if has_archive { get_metadata(arc.as_deref(), "application/pdf", tz) } else { Value::Null };
        // lang = detect(doc.content), "en" on error
        let lang = std::panic::catch_unwind(|| crate::langdetect::detect(&content)).ok().flatten().unwrap_or_else(|| "en".to_string());
        (original, archive, lang)
    })
    .await
    .map_err(|e| ApiError::internal(format!("metadata task: {e}")))?;

    Ok(json!({
        "original_checksum": doc.checksum,
        "original_size": get_filesize(source.as_deref()),
        "original_mime_type": doc.mime_type,
        "media_filename": doc.filename,
        "has_archive_version": has_archive,
        "original_metadata": original_metadata,
        "archive_checksum": doc.archive_checksum,
        "archive_media_filename": doc.archive_filename,
        "original_filename": original_filename,
        "archive_size": if has_archive { get_filesize(archive.as_deref()).map(Value::from).unwrap_or(Value::Null) } else { Value::Null },
        "archive_metadata": archive_metadata,
        "barcodes": barcodes,
        "lang": lang,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B-N4: parser panic during metadata retrieval → `[]` like upstream `except Exception`.
    #[test]
    fn parser_panic_yields_empty_list() {
        let p = FsPath::new("/x/broken.pdf");
        assert_eq!(guarded_extract(p, || panic!("broken parser")), json!([]));
        assert_eq!(guarded_extract(p, || json!([{"key": "a"}])), json!([{"key": "a"}]));
    }
    use chrono::TimeZone;

    #[test]
    fn isoformat_like_python() {
        let t = Utc.with_ymd_and_hms(2026, 1, 21, 8, 0, 0).unwrap();
        assert_eq!(py_isoformat(&t), "2026-01-21T08:00:00+00:00");
        let t = t + chrono::Duration::microseconds(1500);
        assert_eq!(py_isoformat(&t), "2026-01-21T08:00:00.001500+00:00");
    }

    #[test]
    fn pk_lookup() {
        assert_eq!(lookup_pk("12").unwrap(), Some(12));
        assert_eq!(lookup_pk(" 7").unwrap(), Some(7));
        assert_eq!(lookup_pk("99999999999999999999").unwrap(), None);
        assert!(lookup_pk("abc").is_err());
    }

    #[test]
    fn metadata_without_file_is_null() {
        assert_eq!(get_metadata(None, "application/pdf", chrono_tz::UTC), Value::Null);
        assert_eq!(get_metadata(Some(FsPath::new("/nonexistent/x.pdf")), "application/pdf", chrono_tz::UTC), Value::Null);
        assert_eq!(get_filesize(None), None);
    }
}
