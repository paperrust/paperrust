// upstream: src/documents/views.py@v3.4.2 (BulkDownloadView.post, DocumentSelectionMixin._resolve_document_ids)
//           src/documents/serialisers.py@v3.4.2 (DocumentListSerializer, DocumentSelectionSerializer,
//           BulkDownloadSerializer)
//           src/documents/bulk_download.py@v3.4.2 (strategies, _filename_only, _formatted_filepath)
//           rest_framework 3.18 (fields.py: ListField, IntegerField, BooleanField, ChoiceField,
//           DictField; serializers.py: to_internal_value / errors; parsers.py: JSONParser)

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_authz::perms::VIEW_DOCUMENTS_PERMISSIONS;
use pl_authz::{AuthError, AuthUser, User};
use pl_core::AppState;
use serde_json::{Map, Value, json};

use crate::compat;
use crate::db::{self, DocRow};
use crate::docfile::{self, Ctx};
use crate::perms;
use crate::resolve;
use crate::serve::{self, TempGuard};
use crate::zipw::{self, Method, ZipWriter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    Archive,
    Originals,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    None,
    Deflated,
    Bzip2,
    Lzma,
}

/// Compression choices PaperRust cannot produce (`ZIP_BZIP2`, `ZIP_LZMA`): the choice name.
pub fn unsupported_compression(c: Compression) -> Option<&'static str> {
    match c {
        Compression::Bzip2 => Some("bzip2"),
        Compression::Lzma => Some("lzma"),
        _ => None,
    }
}

/// 400 body for [`unsupported_compression`] (DRF field-error shape).
pub fn unsupported_compression_error(name: &str) -> serde_json::Value {
    serde_json::json!({"compression": [format!("\"{name}\" is not supported by PaperRust. Use \"none\" or \"deflated\".")]})
}

/// `DocumentSelectionMixin._resolve_document_ids` for `all=true` (set by pl-server: pl-files
/// cannot depend on pl-documents).
#[derive(Clone)]
pub struct SelectionResolver(pub std::sync::Arc<SelectionResolveFn>);

/// `(state, user, filters, excluded_documents) -> ids`.
pub type SelectionResolveFn = dyn Fn(
        AppState,
        User,
        Option<serde_json::Map<String, Value>>,
        Vec<i64>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<i64>, pl_core::ApiError>> + Send>>
    + Send
    + Sync;

#[derive(Debug)]
pub struct Validated {
    pub documents: Option<Vec<i64>>,
    pub all: bool,
    pub excluded: Vec<i64>,
    /// `filters` (dict) for `all=true`.
    pub filters: Option<serde_json::Map<String, Value>>,
    pub content: Content,
    pub compression: Compression,
    pub follow_formatting: bool,
}

/// Python `str(value)` / `type(value).__name__` for JSON input (for DRF messages).
fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                if f.fract() == 0.0 && f.abs() < 1e16 { format!("{f:.1}") } else { format!("{f}") }
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

/// DRF `IntegerField.to_internal_value`.
fn drf_int(v: &Value) -> Result<i64, &'static str> {
    if v.is_null() {
        return Err("This field may not be null.");
    }
    if let Value::String(s) = v
        && s.chars().count() > 1000
    {
        return Err("String value too large.");
    }
    let s = py_str(v);
    // re_decimal = r'\.0*\s*$'
    let stripped = match s.rfind('.') {
        Some(i) if s[i + 1..].trim_end().chars().all(|c| c == '0') => s[..i].to_string(),
        _ => s.clone(),
    };
    match resolve::py_int(&stripped) {
        Ok(Some(i)) => Ok(i),
        _ => Err("A valid integer is required."),
    }
}

/// DRF `ListField(child=IntegerField())`. Errors as DRF renders them.
pub fn drf_int_list(v: &Value) -> Result<Vec<i64>, Value> {
    if v.is_null() {
        return Err(json!(["This field may not be null."]));
    }
    let Value::Array(items) = v else {
        return Err(json!([format!("Expected a list of items but got type \"{}\".", py_type(v))]));
    };
    let mut out = Vec::with_capacity(items.len());
    let mut errors = Map::new();
    for (i, item) in items.iter().enumerate() {
        match drf_int(item) {
            Ok(n) => out.push(n),
            Err(e) => {
                errors.insert(i.to_string(), json!([e]));
            }
        }
    }
    if errors.is_empty() { Ok(out) } else { Err(Value::Object(errors)) }
}

/// DRF `BooleanField.to_internal_value`.
pub fn drf_bool(v: &Value) -> Result<bool, Value> {
    match v {
        Value::Null => Err(json!(["This field may not be null."])),
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => match n.as_f64() {
            Some(1.0) => Ok(true),
            Some(0.0) => Ok(false),
            _ => Err(json!(["Must be a valid boolean."])),
        },
        Value::String(s) => match s.to_lowercase().as_str() {
            "t" | "y" | "yes" | "true" | "on" | "1" => Ok(true),
            "f" | "n" | "no" | "false" | "off" | "0" => Ok(false),
            _ => Err(json!(["Must be a valid boolean."])),
        },
        _ => Err(json!(["Must be a valid boolean."])),
    }
}

/// DRF `ChoiceField.to_internal_value` (choices compared via `str(data)`).
fn drf_choice<'a>(v: &Value, choices: &[&'a str]) -> Result<&'a str, Value> {
    if v.is_null() {
        return Err(json!(["This field may not be null."]));
    }
    let s = py_str(v);
    match choices.iter().find(|c| **c == s) {
        Some(c) => Ok(c),
        None => Err(json!([format!("\"{s}\" is not a valid choice.")])),
    }
}

pub enum ValidationOutcome {
    Ok(Validated),
    Errors(Value),
}

/// `BulkDownloadSerializer(data=...).is_valid()` – field order: documents, all, filters,
/// excluded_documents, content, compression, follow_formatting; then `validate()`.
pub async fn validate(db: &sqlx::PgPool, data: &Value) -> Result<ValidationOutcome, sqlx::Error> {
    let Value::Object(obj) = data else {
        let msg = if data.is_null() {
            "No data provided".to_string()
        } else {
            format!("Invalid data. Expected a dictionary, but got {}.", py_type(data))
        };
        return Ok(ValidationOutcome::Errors(json!({ "non_field_errors": [msg] })));
    };
    let mut errors = Map::new();

    let mut documents = None;
    if let Some(v) = obj.get("documents") {
        match drf_int_list(v) {
            Ok(ids) => {
                // validate_documents -> _validate_document_id_list
                let n = db::count_existing(db, &ids).await?;
                if n as usize != ids.len() {
                    errors.insert("documents".into(), json!(["Some documents in documents don't exist or were specified twice."]));
                } else {
                    documents = Some(ids);
                }
            }
            Err(e) => {
                errors.insert("documents".into(), e);
            }
        }
    }
    let mut all = false;
    if let Some(v) = obj.get("all") {
        match drf_bool(v) {
            Ok(b) => all = b,
            Err(e) => {
                errors.insert("all".into(), e);
            }
        }
    }
    let mut filters = None;
    if let Some(v) = obj.get("filters") {
        match v {
            Value::Object(m) => filters = Some(m.clone()),
            Value::Null => {
                errors.insert("filters".into(), json!(["This field may not be null."]));
            }
            other => {
                errors.insert("filters".into(), json!([format!("Expected a dictionary of items but got type \"{}\".", py_type(other))]));
            }
        }
    }
    let mut excluded = Vec::new();
    if let Some(v) = obj.get("excluded_documents") {
        match drf_int_list(v) {
            Ok(ids) => excluded = ids,
            Err(e) => {
                errors.insert("excluded_documents".into(), e);
            }
        }
    }
    let mut content = Content::Archive;
    if let Some(v) = obj.get("content") {
        match drf_choice(v, &["archive", "originals", "both"]) {
            Ok("originals") => content = Content::Originals,
            Ok("both") => content = Content::Both,
            Ok(_) => content = Content::Archive,
            Err(e) => {
                errors.insert("content".into(), e);
            }
        }
    }
    let mut compression = Compression::None;
    if let Some(v) = obj.get("compression") {
        match drf_choice(v, &["none", "deflated", "bzip2", "lzma"]) {
            Ok("deflated") => compression = Compression::Deflated,
            Ok("bzip2") => compression = Compression::Bzip2,
            Ok("lzma") => compression = Compression::Lzma,
            Ok(_) => compression = Compression::None,
            Err(e) => {
                errors.insert("compression".into(), e);
            }
        }
    }
    let mut follow_formatting = false;
    if let Some(v) = obj.get("follow_formatting") {
        match drf_bool(v) {
            Ok(b) => follow_formatting = b,
            Err(e) => {
                errors.insert("follow_formatting".into(), e);
            }
        }
    }
    if !errors.is_empty() {
        return Ok(ValidationOutcome::Errors(Value::Object(errors)));
    }

    // DocumentSelectionSerializer.validate
    if !all {
        if !excluded.is_empty() {
            return Ok(ValidationOutcome::Errors(json!({
                "non_field_errors": ["excluded_documents is only supported when all is true."]
            })));
        }
        if documents.is_none() {
            return Ok(ValidationOutcome::Errors(json!({
                "non_field_errors": ["documents is required unless all is true."]
            })));
        }
    }
    Ok(ValidationOutcome::Ok(Validated { documents, all, excluded, filters, content, compression, follow_formatting }))
}

/// DRF media type check for `JSONParser` (`media_type_matches("application/json", ct)`).
fn is_json_content_type(ct: &str) -> bool {
    let main = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let (t, s) = main.split_once('/').unwrap_or((main.as_str(), ""));
    (t == "application" || t == "*") && (s == "json" || s == "*")
}

/// One entry to add: source path, archive name.
struct Planned {
    src: Option<PathBuf>,
    name: String,
}

/// `BulkArchiveStrategy.make_unique_filename` + `add_document` for all docs, in order.
async fn plan(cx: Ctx<'_>, docs: &[DocRow], content: Content, follow_formatting: bool) -> Result<Vec<Planned>, sqlx::Error> {
    let mut out: Vec<Planned> = Vec::new();
    let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for doc in docs {
        let context = resolve::root_of(cx.db, doc).await?;
        // (archive?, folder)
        let mut parts: Vec<(bool, &str)> = Vec::new();
        match content {
            Content::Originals => parts.push((false, "")),
            Content::Archive => parts.push((doc.has_archive_version(), "")),
            Content::Both => {
                if doc.has_archive_version() {
                    parts.push((true, "archive/"));
                }
                parts.push((false, "originals/"));
            }
        }
        for (archive, folder) in parts {
            let src = if archive { docfile::archive_path(cx, doc) } else { docfile::source_path(cx, doc).await? };
            let name = if follow_formatting {
                // _formatted_filepath: Path(folder) / (archive_filename | filename)
                let f = if archive && doc.has_archive_version() {
                    doc.archive_filename.clone().unwrap_or_default()
                } else {
                    doc.filename.clone().unwrap_or_default()
                };
                let joined = if folder.is_empty() { f } else { format!("{folder}{f}") };
                zipw::normalize_arcname(&joined)
            } else {
                let mut counter = 0u32;
                loop {
                    let candidate = format!("{folder}{}", docfile::public_filename(cx, doc, &context, archive, counter).await?);
                    // Review 1 N10: `filename in zipf.namelist()` compares normalized names
                    if name_taken(&names, &candidate) {
                        counter += 1;
                    } else {
                        break candidate;
                    }
                }
            };
            names.insert(zipw::normalize_arcname(&name));
            out.push(Planned { src, name });
        }
    }
    Ok(out)
}

/// `filename in zipf.namelist()`: the stored names are normalized, so is the candidate.
fn name_taken(names: &std::collections::HashSet<String>, candidate: &str) -> bool {
    names.contains(&zipw::normalize_arcname(candidate))
}

/// POST /api/documents/bulk_download/
pub async fn bulk_download(State(state): State<AppState>, headers: HeaderMap, auth: Result<AuthUser, AuthError>, body: Bytes) -> Response {
    // initial(): IsAuthenticated + ViewDocumentsPermissions (POST -> documents.view_document)
    let auth = match auth {
        Ok(a) => a,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = auth.require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS) {
        return e.into_response();
    }
    handle(&state, &headers, auth.user(), &body).await
}

async fn handle(state: &AppState, headers: &HeaderMap, user: &User, body: &[u8]) -> Response {
    let cx = Ctx::new(state);
    let db = cx.db;

    // request.data (JSONParser only)
    let data: Value = if body.is_empty() {
        json!({})
    } else {
        let ct = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
        if !is_json_content_type(ct) {
            return compat::drf_detail(StatusCode::UNSUPPORTED_MEDIA_TYPE, &format!("Unsupported media type \"{ct}\" in request."));
        }
        match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(e) => {
                return compat::drf_detail(StatusCode::BAD_REQUEST, &format!("JSON parse error - {e}"));
            }
        }
    };

    let v = match validate(db, &data).await {
        Ok(ValidationOutcome::Ok(v)) => v,
        Ok(ValidationOutcome::Errors(e)) => return compat::drf_json(StatusCode::BAD_REQUEST, &e),
        Err(e) => return compat::internal("bulk validate", e),
    };
    // bzip2/lzma need codecs PaperRust does not ship: a clear 400 instead of a zip
    // (docs/contract/pl-files.md, Known differences; neither web UI offers them).
    if let Some(name) = unsupported_compression(v.compression) {
        return compat::drf_json(StatusCode::BAD_REQUEST, &unsupported_compression_error(name));
    }
    // `all=true`: `_resolve_document_ids` over the filter dict (resolver from pl-documents,
    // installed by pl-server; docs/STANDALONE-GAPS.md §4.1).
    let ids = if v.all {
        // pl-server always installs the resolver; without it the server is misconfigured (500).
        let Some(SelectionResolver(r)) = state.service::<SelectionResolver>() else {
            return compat::internal("bulk download", "selection resolver for all=true not installed");
        };
        match r(state.clone(), user.clone(), v.filters.clone(), v.excluded.clone()).await {
            Ok(ids) => ids,
            Err(e) => return e.into_response(),
        }
    } else {
        v.documents.clone().unwrap_or_default()
    };

    let result = async {
        let docs = db::load_docs_ordered(db, &ids).await?;
        let doc_ids: Vec<i64> = docs.iter().map(|d| d.id).collect();
        if perms::any_not_permitted(db, user, &doc_ids).await? {
            return Ok::<_, sqlx::Error>(Err(compat::insufficient_permissions()));
        }
        let mut versioned = Vec::with_capacity(docs.len());
        for d in &docs {
            let root = resolve::root_of(db, d).await?;
            versioned.push(db::latest_version(db, &root, false).await?);
        }
        Ok(Ok(plan(cx, &versioned, v.content, v.follow_formatting).await?))
    }
    .await;
    let planned = match result {
        Ok(Ok(p)) => p,
        Ok(Err(r)) => return r,
        Err(e) => return compat::internal("bulk plan", e),
    };

    let method = if v.compression == Compression::Deflated { Method::Deflated } else { Method::Stored };
    let scratch = cx.cfg.scratch_dir.clone();
    let tz = cx.cfg.tz;
    let built = tokio::task::spawn_blocking(move || build_zip(planned, method, scratch, tz)).await;
    let (file, path, len) = match built {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return compat::internal("bulk zip", e),
        Err(e) => return compat::internal("bulk zip task", e),
    };
    let file = tokio::fs::File::from_std(file);
    let mut r = Response::new(serve::file_body(file, Some(TempGuard(Some(path)))));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"documents.zip\""));
    r
}

/// Write the archive into a temp file in SCRATCH_DIR, rewind it and hand it back.
fn build_zip(
    planned: Vec<Planned>,
    method: Method,
    scratch_dir: PathBuf,
    tz: chrono_tz::Tz,
) -> std::io::Result<(std::fs::File, PathBuf, u64)> {
    use std::io::{Seek, SeekFrom};
    std::fs::create_dir_all(&scratch_dir)?;
    let path = scratch_dir.join(format!("tmp{}-compressed-archive", uuid::Uuid::new_v4().simple()));
    let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
    let guard = TempGuard(Some(path.clone()));
    let mut z = ZipWriter::new(file, method, tz);
    for p in planned {
        let Some(src) = p.src else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "path rejected"));
        };
        z.write_file(&src, &p.name)?;
    }
    let mut file = z.finish()?;
    let len = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    // Unix: unlink now like upstream (the open handle keeps the data); elsewhere the guard on
    // the response body removes it after streaming.
    let mut guard = guard;
    #[cfg(unix)]
    {
        if std::fs::remove_file(&path).is_ok() {
            guard.0 = None;
        }
    }
    let path = guard.0.take().unwrap_or(path);
    Ok((file, path, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 1 N4 (checked against DRF 3.18.3 of the reference): `BooleanField` compares lower-case,
    /// `"tRuE"`/`"yES"` are valid, unknown values are not.
    #[test]
    fn name_taken_normalizes_candidate() {
        let mut names = std::collections::HashSet::new();
        names.insert(zipw::normalize_arcname("a/./b.pdf"));
        assert!(name_taken(&names, "a/./b.pdf"));
        assert!(name_taken(&names, "a/b.pdf"));
        assert!(!name_taken(&names, "a/c.pdf"));
    }

    #[test]
    fn drf_bool_is_case_insensitive_like_drf_3_18() {
        assert_eq!(drf_bool(&json!("tRuE")), Ok(true));
        assert_eq!(drf_bool(&json!("yES")), Ok(true));
        assert!(drf_bool(&json!("maybe")).is_err());
    }

    #[test]
    fn ints_like_drf() {
        assert_eq!(drf_int(&json!(5)), Ok(5));
        assert_eq!(drf_int(&json!("5")), Ok(5));
        assert_eq!(drf_int(&json!(5.0)), Ok(5));
        assert_eq!(drf_int(&json!("5.00 ")), Ok(5));
        assert!(drf_int(&json!(5.5)).is_err());
        assert!(drf_int(&json!(true)).is_err());
        assert!(drf_int(&json!("x")).is_err());
        assert_eq!(drf_int_list(&json!([1, "a"])).unwrap_err(), json!({"1": ["A valid integer is required."]}));
        assert_eq!(drf_int_list(&json!("1")).unwrap_err(), json!(["Expected a list of items but got type \"str\"."]));
    }

    #[test]
    fn choices_and_bools() {
        assert_eq!(drf_choice(&json!("both"), &["archive", "both"]), Ok("both"));
        assert_eq!(drf_choice(&json!("x"), &["archive"]).unwrap_err(), json!(["\"x\" is not a valid choice."]));
        assert_eq!(drf_bool(&json!("TRUE")), Ok(true));
        assert_eq!(drf_bool(&json!(0)), Ok(false));
        assert!(drf_bool(&json!("maybe")).is_err());
    }

    #[test]
    fn content_types() {
        assert!(is_json_content_type("application/json"));
        assert!(is_json_content_type("Application/JSON; charset=utf-8"));
        assert!(!is_json_content_type("text/plain"));
        assert!(!is_json_content_type(""));
    }
}

#[cfg(test)]
mod compression_tests {
    use super::*;

    /// bzip2/lzma: a clear 400 instead of the former "not supported" 404.
    #[test]
    fn unsupported_compressions() {
        assert_eq!(unsupported_compression(Compression::Bzip2), Some("bzip2"));
        assert_eq!(unsupported_compression(Compression::Lzma), Some("lzma"));
        assert_eq!(unsupported_compression(Compression::Deflated), None);
        assert_eq!(unsupported_compression(Compression::None), None);
        assert_eq!(
            unsupported_compression_error("lzma"),
            serde_json::json!({"compression": ["\"lzma\" is not supported by PaperRust. Use \"none\" or \"deflated\"."]})
        );
    }
}
