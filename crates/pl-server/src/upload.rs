//! `POST /api/documents/post_document/` in Rust (route id `documents.post_document`).
//!
//! upstream: src/documents/views.py@v3.4.2 (PostDocumentView.post),
//! src/documents/serialisers.py@v3.4.2 (PostDocumentSerializer, CustomFieldInstanceSerializer.validate,
//! validate_documentlink_targets), src/documents/validators.py@v3.4.2 (uri_validator),
//! rest_framework@3.16 (fields.py: Field.get_value, CharField, IntegerField, BooleanField,
//! DateTimeField, JSONField, FileField; relations.py: PrimaryKeyRelatedField, ManyRelatedField;
//! request.py: Request._parse; parsers.py: MultiPartParser),
//! django@5.2.18 (http/multipartparser.py: sanitize_file_name, DATA_UPLOAD_MAX_MEMORY_SIZE,
//! DATA_UPLOAD_MAX_NUMBER_FILES; core/validators.py).
//!
//! The body is **not** buffered by the gate (`body::Gate::streaming`): the file part is written
//! to `SCRATCH_DIR/tmpXXXXXXXX/` while it arrives (no 2.5 MiB limit for the file, like Django's
//! upload spooling); non-file fields are limited to `DATA_UPLOAD_MAX_MEMORY_SIZE` like Django.
//! The task `documents.tasks.consume_file` goes through the `TaskQueue` in `AppState`
//! (PaperRust queue).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use pl_authz::AuthUser;
use pl_core::route::{ALLOW_POST_ONLY, drf_view};
use pl_core::{ApiError, ApiRoute, AppState, Drf, RequestContext, ValidationErrors};
use rand::Rng;
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// Route id in routes.toml.
pub const ROUTE_ID: &str = "documents.post_document";
pub const PATH: &str = "/api/documents/post_document/";

/// Routes whose body the gate must not buffer (streaming upload, see [`crate::body::Gate`]).
/// W23: `documents.update_version` (pl-documents-write) reads multipart itself, streamed.
/// Standalone gaps (docs/STANDALONE-GAPS.md): `config.update`/`config.partial_update` read the
/// (multipart) body themselves – logo uploads larger than 2.5 MiB must not be rejected by the body gate.
pub const STREAMING_ROUTES: &[&str] = &[ROUTE_ID, "documents.update_version", "config.update", "config.partial_update"];

/// Django `DATA_UPLOAD_MAX_NUMBER_FILES` (default, not overridden by paperless).
pub const DATA_UPLOAD_MAX_NUMBER_FILES: usize = 100;
/// `Document.ARCHIVE_SERIAL_NUMBER_MAX`.
pub const ASN_MAX: i128 = 0xFFFF_FFFF;
/// DRF `IntegerField.MAX_STRING_LENGTH`.
const MAX_STRING_LENGTH: usize = 1000;
/// `settings.CONSUMER_PDF_RECOVERABLE_MIME_TYPES`.
const PDF_RECOVERABLE_MIME_TYPES: &[&str] = &["application/octet-stream"];

const MSG_CUSTOM_FIELDS_FORMAT: &str = "Custom fields must be a list of integers or an object mapping ids to values.";

pub fn routes() -> Vec<ApiRoute> {
    vec![
        ApiRoute::new(
            ROUTE_ID,
            Method::POST,
            PATH,
            "src/documents/views.py::PostDocumentView.post",
            drf_view(post(post_document).layer(DefaultBodyLimit::disable()), ALLOW_POST_ONLY),
        )
        // parser_classes = (MultiPartParser,): every other content type is answered here (415).
        .accepts_drf_parsers(),
    ]
}

// ---------------------------------------------------------------------------- request data

/// An uploaded file, already on disk.
#[derive(Debug)]
pub struct UploadedFile {
    /// `UploadedFile.name` (after Django's `sanitize_file_name`).
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

/// `request.data` of a multipart request (QueryDict + files).
#[derive(Debug, Default)]
pub struct FormData {
    /// Non-file fields in arrival order.
    pub fields: Vec<(String, String)>,
    /// Last file part named `document`.
    pub document: Option<UploadedFile>,
    /// Data came from a form (`html.is_html_input`); `false` for an empty body (DRF: `{}`).
    pub html: bool,
}

impl FormData {
    /// `QueryDict.__getitem__` (last value).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    /// `QueryDict.getlist`.
    pub fn getlist(&self, key: &str) -> Vec<&str> {
        self.fields.iter().filter(|(k, _)| k == key).map(|(_, v)| v.as_str()).collect()
    }
    pub fn contains(&self, key: &str) -> bool {
        self.fields.iter().any(|(k, _)| k == key)
    }
}

/// New directory only for the own user (security review final W-N2, like
/// `tempfile.mkdtemp`: Unix `0700`). Windows: no own ACL – the directory inherits the
/// permissions of `SCRATCH_DIR` (default `%TEMP%` of the service account or the desktop data directory
/// restricted to the user via ACL); `mkdtemp` does not set an ACL on Windows either.
fn create_private_dir(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new().mode(0o700).create(p)
    }
    #[cfg(not(unix))]
    {
        std::fs::DirBuilder::new().create(p)
    }
}

/// Temporary directory (`tempfile.mkdtemp(dir=SCRATCH_DIR)`), removed on drop unless kept.
pub struct SpoolDir {
    pub path: PathBuf,
    keep: bool,
}

impl SpoolDir {
    pub fn create(scratch: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(scratch)?;
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
        let mut rng = rand::thread_rng();
        for _ in 0..100 {
            let name: String = (0..8).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect();
            let p = scratch.join(format!("tmp{name}"));
            match create_private_dir(&p) {
                Ok(()) => return Ok(Self { path: p, keep: false }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no usable temporary directory name"))
    }
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for SpoolDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

enum ReadError {
    /// DRF `ParseError("Multipart form parse error - …")`.
    Parse(String),
    /// Django `SuspiciousOperation` (`RequestDataTooBig`, `TooManyFilesSent`) → 400 page.
    Suspicious(&'static str),
    Io(std::io::Error),
}

/// Django `MultiPartParser.sanitize_file_name` – shared with pl-documents-write
/// (security review final W-N4).
pub use pl_core::safepath::sanitize_file_name;

/// Read the multipart body: fields into memory (Django limit), file parts to disk.
async fn read_multipart(mut mp: Multipart, spool: &SpoolDir) -> Result<FormData, ReadError> {
    use tokio::io::AsyncWriteExt;
    let limit = pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE;
    let mut data = FormData { html: true, ..Default::default() };
    let mut field_bytes = 0usize;
    let mut num_files = 0usize;
    let mut seq = 0usize;
    let parse = |e: axum::extract::multipart::MultipartError| ReadError::Parse(e.body_text());
    while let Some(mut field) = mp.next_field().await.map_err(parse)? {
        let Some(name) = field.name().map(str::to_string) else {
            // Django: parts without a name are skipped.
            while field.chunk().await.map_err(parse)?.is_some() {}
            continue;
        };
        match field.file_name().map(str::to_string) {
            None => {
                field_bytes += name.len() + 2;
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(parse)? {
                    field_bytes += chunk.len();
                    if field_bytes > limit {
                        return Err(ReadError::Suspicious("RequestDataTooBig"));
                    }
                    buf.extend_from_slice(&chunk);
                }
                data.fields.push((name, String::from_utf8_lossy(&buf).into_owned()));
            }
            Some(raw_name) => {
                let Some(file_name) = sanitize_file_name(&raw_name) else {
                    while field.chunk().await.map_err(parse)?.is_some() {}
                    continue;
                };
                num_files += 1;
                if num_files > DATA_UPLOAD_MAX_NUMBER_FILES {
                    return Err(ReadError::Suspicious("TooManyFilesSent"));
                }
                if name != "document" {
                    // Spooled and dropped by Django; only counted here.
                    while field.chunk().await.map_err(parse)?.is_some() {}
                    continue;
                }
                seq += 1;
                let path = spool.path.join(format!(".paperrust-upload-{seq}.part"));
                let mut f = tokio::fs::File::create(&path).await.map_err(ReadError::Io)?;
                let mut size = 0u64;
                while let Some(chunk) = field.chunk().await.map_err(parse)? {
                    size += chunk.len() as u64;
                    f.write_all(&chunk).await.map_err(ReadError::Io)?;
                }
                f.flush().await.map_err(ReadError::Io)?;
                drop(f);
                if let Some(prev) = data.document.replace(UploadedFile { name: file_name, path, size }) {
                    let _ = std::fs::remove_file(prev.path);
                }
            }
        }
    }
    Ok(data)
}

// ---------------------------------------------------------------------------- validation

/// `PostDocumentSerializer.validated_data` (only what the view reads).
#[derive(Debug, Default, PartialEq)]
pub struct Validated {
    pub created: Option<chrono::NaiveDate>,
    pub title: Option<String>,
    pub correspondent: Option<i64>,
    pub document_type: Option<i64>,
    pub storage_path: Option<i64>,
    pub tags: Option<Vec<i64>>,
    pub archive_serial_number: Option<i64>,
    /// After `validate_custom_fields`: dict (id → value) or list of ids.
    pub custom_fields: Option<CustomFieldsInput>,
    pub from_webui: Option<bool>,
    /// PaperRust: `ai_ocr` (optional, boolean) = AI text recognition for this document.
    pub ai_ocr: Option<bool>,
}

#[derive(Debug, PartialEq)]
pub enum CustomFieldsInput {
    Map(Vec<(i64, Value)>),
    Ids(Vec<i64>),
}

/// Value of one form field as DRF's `Field.get_value` sees it.
enum Raw<'a> {
    /// `empty`: field skipped.
    Empty,
    /// `None` (blank value of an `allow_null` field).
    Null,
    Value(&'a str),
}

/// `Field.get_value` for HTML input (`required=False` fields): missing → `default_empty_html`
/// (`empty`, BooleanField: `False`), `''` → `None` (allow_null) / `empty`.
fn get_value<'a>(form: &'a FormData, key: &str, allow_null: bool) -> Raw<'a> {
    if !form.html {
        return form.get(key).map_or(Raw::Empty, Raw::Value);
    }
    match form.get(key) {
        None => Raw::Empty,
        Some("") if allow_null => Raw::Null,
        Some("") => Raw::Empty,
        Some(v) => Raw::Value(v),
    }
}

/// Result of Python `int(str)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PyInt {
    Int(i128),
    /// Valid, but beyond i128 (sign).
    Huge {
        negative: bool,
    },
    Invalid,
}

/// Python `int(s)` for strings (ASCII digits, surrounding whitespace, sign, single underscores).
pub fn py_int(s: &str) -> PyInt {
    if let Some(v) = pl_core::pagination::python_int(s) {
        return PyInt::Int(v);
    }
    let t = py_strip(s);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let well_formed = !body.is_empty()
        && !body.starts_with('_')
        && !body.ends_with('_')
        && !body.contains("__")
        && body.bytes().all(|b| b.is_ascii_digit() || b == b'_');
    if well_formed { PyInt::Huge { negative: neg } } else { PyInt::Invalid }
}

/// Python `str.strip()` (Unicode whitespace incl. U+001C..U+001F).
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `(data_type, extra_data)` of a custom field.
pub type CustomFieldDef = (String, Option<Value>);

/// Database lookups the validation needs (trait so the rules are testable without a DB).
pub trait Lookups: Send + Sync {
    /// `Model.objects.filter(pk__in=ids)` → existing ids (`table` is one of the related tables).
    fn existing<'a>(&'a self, table: &'static str, ids: &'a [i64]) -> futures::future::BoxFuture<'a, Result<Vec<i64>, ApiError>>;
    /// `CustomField.objects.get(id=…)` → `(data_type, extra_data)`.
    fn custom_field(&self, id: i64) -> futures::future::BoxFuture<'_, Result<Option<CustomFieldDef>, ApiError>>;
    /// `Document.objects.filter(id__in=ids).count()` (not trashed).
    fn count_documents<'a>(&'a self, ids: &'a [i64]) -> futures::future::BoxFuture<'a, Result<i64, ApiError>>;
}

/// [`Lookups`] against PostgreSQL.
pub struct PgLookups(pub sqlx::PgPool);

impl Lookups for PgLookups {
    fn existing<'a>(&'a self, table: &'static str, ids: &'a [i64]) -> futures::future::BoxFuture<'a, Result<Vec<i64>, ApiError>> {
        Box::pin(async move {
            // `table` is a fixed identifier from this module, never user input.
            let ids: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
            let sql = format!("SELECT id FROM {table} WHERE id = ANY($1)");
            let rows: Vec<(i32,)> = sqlx::query_as(&sql).bind(ids).fetch_all(&self.0).await.map_err(ApiError::internal)?;
            Ok(rows.into_iter().map(|(i,)| i64::from(i)).collect())
        })
    }
    fn custom_field(&self, id: i64) -> futures::future::BoxFuture<'_, Result<Option<CustomFieldDef>, ApiError>> {
        Box::pin(async move {
            let Ok(id) = i32::try_from(id) else { return Ok(None) };
            sqlx::query_as::<_, (String, Option<sqlx::types::Json<Value>>)>(
                "SELECT data_type, extra_data FROM documents_customfield WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&self.0)
            .await
            .map(|r| r.map(|(t, e)| (t, e.map(|j| j.0))))
            .map_err(ApiError::internal)
        })
    }
    fn count_documents<'a>(&'a self, ids: &'a [i64]) -> futures::future::BoxFuture<'a, Result<i64, ApiError>> {
        Box::pin(async move {
            let ids: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
            let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM documents_document WHERE deleted_at IS NULL AND id = ANY($1)")
                .bind(ids)
                .fetch_one(&self.0)
                .await
                .map_err(ApiError::internal)?;
            Ok(n)
        })
    }
}

/// `PrimaryKeyRelatedField.to_internal_value` for a form string.
async fn pk_value(lookups: &dyn Lookups, table: &'static str, raw: &str) -> Result<i64, String> {
    let does_not_exist = || format!("Invalid pk \"{raw}\" - object does not exist.");
    match py_int(raw) {
        PyInt::Invalid => Err("Incorrect type. Expected pk value, received str.".into()),
        // integer column: Django answers out-of-range lookups with DoesNotExist
        PyInt::Huge { .. } => Err(does_not_exist()),
        PyInt::Int(v) => {
            let Ok(v) = i64::try_from(v) else { return Err(does_not_exist()) };
            if i32::try_from(v).is_err() {
                return Err(does_not_exist());
            }
            match lookups.existing(table, &[v]).await {
                Ok(found) if found.contains(&v) => Ok(v),
                Ok(_) => Err(does_not_exist()),
                Err(_) => Err(does_not_exist()),
            }
        }
    }
}

/// `PostDocumentSerializer(data=request.data).is_valid()` (field order as declared).
pub async fn validate(
    form: &FormData,
    lookups: &dyn Lookups,
    registry: &pl_parsers::ParserRegistry,
    tz: &chrono_tz::Tz,
) -> Result<Result<Validated, ValidationErrors>, ApiError> {
    let mut out = Validated::default();
    let mut errors = ValidationErrors::new();

    // created = DateTimeField(allow_null=True, required=False); validate_created → .date()
    match get_value(form, "created", true) {
        Raw::Empty | Raw::Null => {}
        Raw::Value(v) => match pl_core::datetime::drf_parse_datetime(v, tz) {
            Ok(dt) => out.created = Some(dt.with_timezone(tz).date_naive()),
            Err(e) => errors.add("created", e),
        },
    }

    // document = FileField(); validate_document
    match &form.document {
        Some(f) if f.size == 0 => errors.add("document", "The submitted file is empty."),
        Some(f) => {
            let detected = pl_parsers::mime::detect_file(&f.path).map_err(ApiError::internal)?;
            if !registry.is_mime_type_supported(&detected)
                && !(PDF_RECOVERABLE_MIME_TYPES.contains(&detected.as_str()) && f.name.ends_with(".pdf"))
            {
                errors.add("document", format!("File type {detected} not supported"));
            }
        }
        None if form.contains("document") => {
            errors.add("document", "The submitted data was not a file. Check the encoding type on the form.")
        }
        None => errors.add("document", "No file was submitted."),
    }

    // title = CharField(required=False)
    match get_value(form, "title", false) {
        Raw::Empty | Raw::Null => {}
        Raw::Value(v) => {
            let t = py_strip(v);
            if t.is_empty() {
                errors.add("title", "This field may not be blank.");
            } else if t.contains('\0') {
                errors.add("title", "Null characters are not allowed.");
            } else {
                out.title = Some(t.to_string());
            }
        }
    }

    // correspondent / document_type / storage_path = PrimaryKeyRelatedField(allow_null=True)
    for (key, table) in [
        ("correspondent", "documents_correspondent"),
        ("document_type", "documents_documenttype"),
        ("storage_path", "documents_storagepath"),
    ] {
        match get_value(form, key, true) {
            Raw::Empty | Raw::Null => {}
            Raw::Value(v) => match pk_value(lookups, table, v).await {
                Ok(id) => {
                    let slot = match key {
                        "correspondent" => &mut out.correspondent,
                        "document_type" => &mut out.document_type,
                        _ => &mut out.storage_path,
                    };
                    *slot = Some(id);
                }
                Err(e) => errors.add(key, e),
            },
        }
    }

    // tags = PrimaryKeyRelatedField(many=True, required=False); validate_tags → ids or None
    let tag_values: Vec<&str> = if form.html { form.getlist("tags") } else { form.get("tags").into_iter().collect() };
    let mut tags = Vec::new();
    let mut tag_error = None;
    for v in tag_values {
        match pk_value(lookups, "documents_tag", v).await {
            Ok(id) => tags.push(id),
            Err(e) => {
                tag_error = Some(e);
                break;
            }
        }
    }
    match tag_error {
        Some(e) => errors.add("tags", e),
        None if !tags.is_empty() => out.tags = Some(tags),
        None => {}
    }

    // archive_serial_number = IntegerField(min_value=0, max_value=0xFFFFFFFF, required=False)
    match get_value(form, "archive_serial_number", false) {
        Raw::Empty | Raw::Null => {}
        Raw::Value(v) => match integer_field(v, 0, ASN_MAX) {
            Ok(n) => out.archive_serial_number = i64::try_from(n).ok(),
            Err(e) => errors.add("archive_serial_number", e),
        },
    }

    // custom_fields = JSONField(required=False); validate_custom_fields
    let cf_raw = if form.html { form.get("custom_fields") } else { None };
    if let Some(raw) = cf_raw {
        match serde_json::from_str::<Value>(raw) {
            Err(_) => errors.add("custom_fields", "Value must be valid JSON."),
            Ok(v) => match validate_custom_fields(v, lookups).await? {
                Ok(cf) => out.custom_fields = cf,
                Err(e) => errors.add("custom_fields", e),
            },
        }
    }

    // from_webui = BooleanField(required=False)
    match get_value(form, "from_webui", false) {
        Raw::Empty | Raw::Null => {
            // BooleanField.default_empty_html = False (missing in a form → False)
            if form.html && !form.contains("from_webui") {
                out.from_webui = Some(false);
            }
        }
        Raw::Value(v) => match drf_boolean(v) {
            Some(b) => out.from_webui = Some(b),
            None => errors.add("from_webui", "Must be a valid boolean."),
        },
    }

    // PaperRust: ai_ocr = BooleanField(required=False) – missing → not chosen
    if let Raw::Value(v) = get_value(form, "ai_ocr", false) {
        match drf_boolean(v) {
            Some(b) => out.ai_ocr = Some(b),
            None => errors.add("ai_ocr", "Must be a valid boolean."),
        }
    }

    Ok(if errors.is_empty() { Ok(out) } else { Err(errors) })
}

/// DRF `BooleanField.to_internal_value` (strings; `allow_null=False`).
pub fn drf_boolean(v: &str) -> Option<bool> {
    match v {
        "t" | "T" | "y" | "Y" | "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" | "1" => Some(true),
        "f" | "F" | "n" | "N" | "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" | "0" => Some(false),
        _ => None,
    }
}

/// DRF `IntegerField.to_internal_value` + Max/MinValueValidator.
pub fn integer_field(v: &str, min: i128, max: i128) -> Result<i128, String> {
    if v.chars().count() > MAX_STRING_LENGTH {
        return Err("String value too large.".into());
    }
    // re_decimal = r'\.0*\s*$' (re.sub: first match only, at the end)
    let s = strip_decimal_zero(v);
    let (big, n) = match py_int(s) {
        PyInt::Int(n) => (None, n),
        PyInt::Huge { negative } => (Some(negative), 0),
        PyInt::Invalid => return Err("A valid integer is required.".into()),
    };
    let too_big = big.map_or(n > max, |neg| !neg);
    let too_small = big.unwrap_or(n < min);
    if too_big {
        return Err(format!("Ensure this value is less than or equal to {max}."));
    }
    if too_small {
        return Err(format!("Ensure this value is greater than or equal to {min}."));
    }
    Ok(n)
}

/// `re.sub(r'\.0*\s*$', '', s)`.
fn strip_decimal_zero(s: &str) -> &str {
    // `$` also matches before a trailing newline; the `\s*` swallows it anyway.
    let trimmed = s.trim_end_matches(|c: char| c.is_whitespace());
    let no_zeros = trimmed.trim_end_matches('0');
    match no_zeros.strip_suffix('.') {
        Some(rest) => rest,
        None => s,
    }
}

/// Python truthiness of a JSON value.
fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `int(x)` for a JSON value (`int`, `float` truncated, `bool`, numeric `str`).
fn py_int_value(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::String(s) => match py_int(s) {
            PyInt::Int(i) => i64::try_from(i).ok(),
            _ => None,
        },
        _ => None,
    }
}

/// `PostDocumentSerializer.validate_custom_fields`.
async fn validate_custom_fields(v: Value, lookups: &dyn Lookups) -> Result<Result<Option<CustomFieldsInput>, String>, ApiError> {
    if !py_truthy(&v) {
        return Ok(Ok(None));
    }
    // Normalize single values to a list (bool is an int in Python)
    let v = match v {
        Value::Bool(_) => Value::Array(vec![v]),
        Value::Number(ref n) if n.is_i64() || n.is_u64() => Value::Array(vec![v]),
        other => other,
    };
    match v {
        Value::Object(map) => {
            let mut normalized: BTreeMap<i64, Value> = BTreeMap::new();
            let mut order: Vec<i64> = Vec::new();
            for (field_id, value) in map {
                let Some(id) = (match py_int(&field_id) {
                    PyInt::Int(i) => i64::try_from(i).ok(),
                    _ => None,
                }) else {
                    return Ok(Err(format!("Custom field id must be an integer: {field_id}")));
                };
                let Some((data_type, extra)) = lookups.custom_field(id).await? else {
                    return Ok(Err(format!("Custom field with id {id} does not exist")));
                };
                if let Err(e) = validate_custom_field_value(&data_type, extra.as_ref(), &value, lookups).await? {
                    return Ok(Err(e));
                }
                if !order.contains(&id) {
                    order.push(id);
                }
                normalized.insert(id, value);
            }
            Ok(Ok(Some(CustomFieldsInput::Map(order.into_iter().filter_map(|id| normalized.remove(&id).map(|v| (id, v))).collect()))))
        }
        Value::Array(items) => {
            let mut ids = Vec::with_capacity(items.len());
            for i in &items {
                match py_int_value(i) {
                    Some(id) => ids.push(id),
                    None => return Ok(Err(MSG_CUSTOM_FIELDS_FORMAT.into())),
                }
            }
            let mut unique = ids.clone();
            unique.sort_unstable();
            unique.dedup();
            let found = lookups.existing("documents_customfield", &unique).await?;
            if found.len() != unique.len() {
                return Ok(Err("Some custom fields don't exist or were specified twice.".into()));
            }
            Ok(Ok(Some(CustomFieldsInput::Ids(ids))))
        }
        _ => Ok(Err(MSG_CUSTOM_FIELDS_FORMAT.into())),
    }
}

/// Python `==` between JSON values (`1 == 1.0 == True`, no str/number coercion).
fn py_eq(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Python `str(value)` for the regex validators.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => pl_core::pyrepr::repr_value(other),
    }
}

/// `CustomFieldInstanceSerializer.validate` (value part). `Err` from the outer result = upstream
/// would answer 500 (TypeError/KeyError inside the validator).
async fn validate_custom_field_value(
    data_type: &str,
    extra: Option<&Value>,
    value: &Value,
    lookups: &dyn Lookups,
) -> Result<Result<(), String>, ApiError> {
    if value.is_null() {
        return Ok(Ok(()));
    }
    let type_error = || ApiError::internal(format!("custom field validation: unsupported value for {data_type}"));
    match data_type {
        "url" => {
            let len = match value {
                Value::String(s) => s.chars().count(),
                Value::Array(a) => a.len(),
                Value::Object(o) => o.len(),
                _ => return Err(type_error()),
            };
            if len > 0 {
                let Value::String(s) = value else {
                    return Ok(Err(format!("Unable to parse URI {}", py_str(value))));
                };
                if let Err(e) = pl_workflows::serializers::url_validator(s) {
                    // uri_validator allows every scheme; url_validator restricts to http(s).
                    if !e.starts_with("URI scheme '") {
                        return Ok(Err(e));
                    }
                }
            }
        }
        "integer" => {
            let s = py_str(value);
            let re_ok = {
                let body = s.strip_prefix('-').unwrap_or(&s);
                // integer_validator = r"^-?\d+\Z" (Python \d also matches non-ASCII digits; ASCII here)
                !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit())
            };
            if !re_ok {
                return Ok(Err("Enter a valid integer.".into()));
            }
            let n = match value {
                Value::Number(n) => n.as_i64().map(i128::from),
                Value::String(s) => match py_int(s) {
                    PyInt::Int(i) => Some(i),
                    PyInt::Huge { negative } => Some(if negative { i128::MIN } else { i128::MAX }),
                    PyInt::Invalid => None,
                },
                _ => None,
            };
            let Some(n) = n else { return Ok(Err("Enter a valid integer.".into())) };
            if n < -2_147_483_648 {
                return Ok(Err("Ensure this value is greater than or equal to -2147483648.".into()));
            }
            if n > 2_147_483_647 {
                return Ok(Err("Ensure this value is less than or equal to 2147483647.".into()));
            }
        }
        "monetary" if value != &Value::String(String::new()) => {
            let s = py_str(value);
            if !decimal_12_2(&s) && !monetary_regex(&s) {
                return Ok(Err("Must be a two-decimal number with optional currency code e.g. GBP123.45".into()));
            }
        }
        "string" => {
            let len = match value {
                Value::String(s) => s.chars().count(),
                Value::Array(a) => a.len(),
                Value::Object(o) => o.len(),
                _ => return Err(type_error()),
            };
            if len > 128 {
                return Ok(Err(format!("Ensure this value has at most 128 characters (it has {len}).")));
            }
        }
        "select" => {
            let Some(options) = extra.and_then(|e| e.get("select_options")) else {
                return Err(type_error());
            };
            let found = options.as_array().is_some_and(|opts| opts.iter().any(|o| o.get("id").is_some_and(|id| py_eq(id, value))));
            if !found {
                return Ok(Err(format!("Value must be an id of an element in {}", pl_core::pyrepr::repr_value(options))));
            }
        }
        "documentlink" => {
            let Value::Array(items) = value else {
                return Ok(Err("Value must be a list".into()));
            };
            let mut ids = Vec::with_capacity(items.len());
            for i in items {
                ids.push(py_int_value(i).ok_or_else(type_error)?);
            }
            if lookups.count_documents(&ids).await? != ids.len() as i64 {
                return Ok(Err("Some documents in value don't exist or were specified twice.".into()));
            }
        }
        "date" => {
            let ok = matches!(value, Value::String(s) if pl_core::datetime::drf_parse_date(s).is_ok());
            if !ok {
                return Ok(Err(pl_core::datetime::DRF_DATE_INVALID.into()));
            }
        }
        _ => {}
    }
    Ok(Ok(()))
}

/// `DecimalValidator(max_digits=12, decimal_places=2)(Decimal(str(value)))` passes.
fn decimal_12_2(s: &str) -> bool {
    let t = s.trim();
    let t = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (mantissa, exp) = match t.find(['e', 'E']) {
        Some(i) => match t[i + 1..].parse::<i64>() {
            Ok(e) => (&t[..i], e),
            Err(_) => return false,
        },
        None => (t, 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((a, b)) => (a, b),
        None => (mantissa, ""),
    };
    let clean = |p: &str| -> Option<String> {
        if p.starts_with('_') || p.ends_with('_') || p.contains("__") {
            return None;
        }
        let d: String = p.chars().filter(|c| *c != '_').collect();
        d.bytes().all(|b| b.is_ascii_digit()).then_some(d)
    };
    let (Some(int_d), Some(frac_d)) = (clean(int_part), clean(frac_part)) else { return false };
    if int_d.is_empty() && frac_d.is_empty() {
        return false;
    }
    // Decimal digit tuple (leading zeros stripped, at least one digit) and exponent
    let all = format!("{int_d}{frac_d}");
    let stripped = all.trim_start_matches('0');
    let digit_tuple_len = if stripped.is_empty() { 1 } else { stripped.len() } as i64;
    let is_zero = stripped.is_empty();
    let exponent = exp - frac_d.len() as i64;
    let (digits, decimals) = if exponent >= 0 {
        (if is_zero { 1 } else { digit_tuple_len + exponent }, 0)
    } else if -exponent > digit_tuple_len {
        (-exponent, -exponent)
    } else {
        (digit_tuple_len, -exponent)
    };
    let whole = digits - decimals;
    digits <= 12 && decimals <= 2 && whole <= 10
}

/// `RegexValidator(r"^[A-Z]{3}-?\d+(\.\d{1,2})$")` on `str(value)`.
fn monetary_regex(s: &str) -> bool {
    static RE: std::sync::LazyLock<Option<regex::Regex>> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^[A-Z]{3}-?\d+(\.\d{1,2})\n?\z").ok());
    RE.as_ref().is_some_and(|r| r.is_match(s))
}

// ---------------------------------------------------------------------------- handler

fn spool_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(format!("upload spooling failed: {e}"))
}

/// `PostDocumentView.post`.
pub async fn post_document(ctx: RequestContext, auth: AuthUser, State(state): State<AppState>, req: Request) -> Response {
    match post_document_inner(&ctx, &auth, &state, req).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn post_document_inner(ctx: &RequestContext, auth: &AuthUser, state: &AppState, req: Request) -> Result<Response, ApiError> {
    if !auth.user().has_perm("documents.add_document") {
        return Err(ApiError::django_text(StatusCode::FORBIDDEN, "Insufficient permissions"));
    }
    let content_type =
        req.headers().get(header::CONTENT_TYPE).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    // Request._parse: no body → empty data (no parser selected, no 415)
    let empty_body = hyper::body::Body::size_hint(req.body()).exact() == Some(0);
    let media = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let mut spool = SpoolDir::create(&state.config().scratch_dir).map_err(spool_error)?;
    let form = if empty_body {
        FormData::default()
    } else if media != "multipart/form-data" {
        return Err(ApiError::unsupported_media_type(&content_type));
    } else {
        let mp = Multipart::from_request(req, &())
            .await
            .map_err(|_| ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")))?;
        match read_multipart(mp, &spool).await {
            Ok(f) => f,
            Err(ReadError::Parse(m)) => {
                tracing::debug!(error = %m, "multipart parse error");
                return Err(ApiError::parse_error(Some("Multipart form parse error - Invalid multipart body")));
            }
            Err(ReadError::Suspicious(kind)) => {
                tracing::warn!(target: "django.security", kind, "upload rejected");
                return Err(ApiError::django_bad_request());
            }
            Err(ReadError::Io(e)) => return Err(spool_error(e)),
        }
    };

    let registry = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
    let lookups = PgLookups(state.db().clone());
    let v = match validate(&form, &lookups, &registry, state.tz()).await? {
        Ok(v) => v,
        Err(errors) => return Err(ApiError::validation(errors.to_value())),
    };
    let Some(file) = form.document else {
        return Err(ApiError::internal("validated upload without file"));
    };

    // doc_name = normalize("NFC", doc_name); temp path = mkdtemp / sanitize_filename(doc_name)
    let doc_name: String = file.name.nfc().collect();
    let target_name = pl_templating::sanitize_filename(&doc_name, "");
    if target_name.is_empty() {
        return Err(ApiError::internal("upload file name is empty after sanitizing"));
    }
    let target = spool.path.join(&target_name);
    std::fs::rename(&file.path, &target).map_err(spool_error)?;
    // os.utime(temp_file_path, times=(t, t)) with t = int(mktime(now))
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(chrono::Utc::now().timestamp().max(0) as u64);
    std::fs::File::options()
        .write(true)
        .open(&target)
        .and_then(|f| f.set_times(std::fs::FileTimes::new().set_accessed(now).set_modified(now)))
        .map_err(spool_error)?;

    let from_webui = v.from_webui.unwrap_or(false);
    let source = if from_webui { pl_consumer::DocumentSource::WebUI } else { pl_consumer::DocumentSource::ApiUpload };
    let input = pl_consumer::ConsumableDocument::new(source, &target).map_err(spool_error)?;
    let custom_fields = match v.custom_fields {
        Some(CustomFieldsInput::Map(m)) if !m.is_empty() => Some(m.into_iter().collect::<BTreeMap<i64, Value>>()),
        Some(CustomFieldsInput::Ids(ids)) if !ids.is_empty() => Some(ids.into_iter().map(|i| (i, Value::Null)).collect()),
        _ => None,
    };
    let overrides = pl_consumer::DocumentMetadataOverrides {
        filename: Some(doc_name),
        title: v.title,
        correspondent_id: v.correspondent,
        document_type_id: v.document_type,
        storage_path_id: v.storage_path,
        tag_ids: v.tags,
        created: v.created,
        asn: v.archive_serial_number,
        owner_id: Some(auth.user().id),
        custom_fields,
        ai_ocr: v.ai_ocr.unwrap_or(false),
        ..Default::default()
    };
    let Some(queue) = state.service::<std::sync::Arc<dyn pl_queue::TaskQueue>>() else {
        return Err(ApiError::internal("no task queue configured"));
    };
    let task_id = pl_consumer::task::enqueue_consume(queue.as_ref(), &input, &overrides)
        .await
        .map_err(|e| ApiError::internal(format!("enqueue consume_file failed: {e}")))?;
    spool.keep();
    Ok(Drf::ok(Value::String(task_id.to_string())).indent(ctx.negotiation.indent).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeDb;
    impl Lookups for FakeDb {
        fn existing<'a>(&'a self, table: &'static str, ids: &'a [i64]) -> futures::future::BoxFuture<'a, Result<Vec<i64>, ApiError>> {
            let have: &[i64] = match table {
                "documents_customfield" => &[1, 2, 3, 5, 7, 9, 10],
                _ => &[1, 2, 3],
            };
            let v = ids.iter().copied().filter(|i| have.contains(i)).collect();
            Box::pin(async move { Ok(v) })
        }
        fn custom_field(&self, id: i64) -> futures::future::BoxFuture<'_, Result<Option<CustomFieldDef>, ApiError>> {
            let r = match id {
                1 => Some(("string".to_string(), None)),
                2 => Some(("url".to_string(), None)),
                3 => Some(("date".to_string(), None)),
                5 => Some(("integer".to_string(), None)),
                7 => Some(("monetary".to_string(), None)),
                9 => Some(("documentlink".to_string(), None)),
                10 => Some(("select".to_string(), Some(serde_json::json!({"select_options": [{"id": "a1", "label": "Low"}]})))),
                _ => None,
            };
            Box::pin(async move { Ok(r) })
        }
        fn count_documents<'a>(&'a self, ids: &'a [i64]) -> futures::future::BoxFuture<'a, Result<i64, ApiError>> {
            let mut u = ids.to_vec();
            u.sort_unstable();
            u.dedup();
            let n = u.iter().filter(|i| [1, 2].contains(*i)).count() as i64;
            Box::pin(async move { Ok(n) })
        }
    }

    fn form(fields: &[(&str, &str)], doc: Option<(&str, &[u8])>, dir: &Path) -> FormData {
        let document = doc.map(|(name, bytes)| {
            let p = dir.join(format!("f{}", name.len()));
            std::fs::write(&p, bytes).unwrap();
            UploadedFile { name: name.into(), path: p, size: bytes.len() as u64 }
        });
        FormData { fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), document, html: true }
    }

    async fn run(f: &FormData) -> Result<Validated, Value> {
        let reg = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
        validate(f, &FakeDb, &reg, &chrono_tz::Europe::Zurich).await.unwrap().map_err(|e| e.to_value())
    }

    /// Reference answers (paperless-ngx v3.4.2, 2026-11-08) for the same forms.
    #[tokio::test]
    async fn errors_like_reference() {
        let dir = tempfile::tempdir().unwrap();
        let e = run(&form(&[("title", "x")], None, dir.path())).await.unwrap_err();
        assert_eq!(e.to_string(), r#"{"document":["No file was submitted."]}"#);
        let e = run(&form(
            &[
                ("created", "abc"),
                ("title", "  "),
                ("correspondent", "abc"),
                ("document_type", "9999"),
                ("storage_path", "1.5"),
                ("tags", "1"),
                ("tags", "999"),
                ("archive_serial_number", "-1"),
                ("custom_fields", "{x"),
                ("from_webui", "maybe"),
            ],
            Some(("b.bin", b"\x00\x01\x02binary")),
            dir.path(),
        ))
        .await
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"{"created":["Datetime has wrong format. Use one of these formats instead: YYYY-MM-DDThh:mm[:ss[.uuuuuu]][+HH:MM|-HH:MM|Z]."],"document":["File type application/octet-stream not supported"],"title":["This field may not be blank."],"correspondent":["Incorrect type. Expected pk value, received str."],"document_type":["Invalid pk \"9999\" - object does not exist."],"storage_path":["Incorrect type. Expected pk value, received str."],"tags":["Invalid pk \"999\" - object does not exist."],"archive_serial_number":["Ensure this value is greater than or equal to 0."],"custom_fields":["Value must be valid JSON."],"from_webui":["Must be a valid boolean."]}"#
        );
        let e = run(&form(
            &[("custom_fields", r#"{"abc":1}"#), ("archive_serial_number", "99999999999")],
            Some(("t.txt", b"hello")),
            dir.path(),
        ))
        .await
        .unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"{"archive_serial_number":["Ensure this value is less than or equal to 4294967295."],"custom_fields":["Custom field id must be an integer: abc"]}"#
        );
        let e = run(&form(&[("custom_fields", "[1,999]"), ("tags", ""), ("correspondent", "")], Some(("t.txt", b"hello")), dir.path()))
            .await
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"{"tags":["Incorrect type. Expected pk value, received str."],"custom_fields":["Some custom fields don't exist or were specified twice."]}"#
        );
        let e = run(&form(&[("document", "notafile")], None, dir.path())).await.unwrap_err();
        assert_eq!(e.to_string(), r#"{"document":["The submitted data was not a file. Check the encoding type on the form."]}"#);
        let e = run(&form(&[], Some(("e.txt", b"")), dir.path())).await.unwrap_err();
        assert_eq!(e.to_string(), r#"{"document":["The submitted file is empty."]}"#);
        for (cf, msg) in [
            (r#""x""#, Some(MSG_CUSTOM_FIELDS_FORMAT)),
            ("1.5", Some(MSG_CUSTOM_FIELDS_FORMAT)),
            (r#"[1.7, "2"]"#, None),
            ("[null]", Some(MSG_CUSTOM_FIELDS_FORMAT)),
            (r#"{"1": "x", "999": 2}"#, Some("Custom field with id 999 does not exist")),
            (r#"{"5": "abc"}"#, Some("Enter a valid integer.")),
            ("true", None),
            ("", Some("Value must be valid JSON.")),
        ] {
            let r = run(&form(&[("custom_fields", cf)], Some(("t.txt", b"hello")), dir.path())).await;
            match msg {
                None => assert!(r.is_ok(), "{cf}: {r:?}"),
                Some(m) => assert_eq!(r.unwrap_err()["custom_fields"][0], m, "{cf}"),
            }
        }
        let e =
            run(&form(&[("archive_serial_number", "12abc"), ("tags", "abc")], Some(("t.txt", b"hello")), dir.path())).await.unwrap_err();
        assert_eq!(
            e.to_string(),
            r#"{"tags":["Incorrect type. Expected pk value, received str."],"archive_serial_number":["A valid integer is required."]}"#
        );
    }

    #[tokio::test]
    async fn valid_values() {
        let dir = tempfile::tempdir().unwrap();
        let v = run(&form(
            &[
                ("created", "2025-01-01"),
                ("title", "  Rechnung  "),
                ("correspondent", " 2"),
                ("tags", "1"),
                ("tags", "3"),
                ("archive_serial_number", "12.00"),
                ("custom_fields", r#"{"1": "abc", "10": "a1", "9": [1, 2], "3": "2025-02-03", "7": "EUR12.50"}"#),
                ("from_webui", "true"),
            ],
            Some(("t.txt", b"hello world")),
            dir.path(),
        ))
        .await
        .unwrap();
        assert_eq!(v.created, chrono::NaiveDate::from_ymd_opt(2025, 1, 1));
        assert_eq!(v.title.as_deref(), Some("Rechnung"));
        assert_eq!(v.correspondent, Some(2));
        assert_eq!(v.tags, Some(vec![1, 3]));
        assert_eq!(v.archive_serial_number, Some(12));
        assert_eq!(v.from_webui, Some(true));
        assert!(matches!(v.custom_fields, Some(CustomFieldsInput::Map(ref m)) if m.len() == 5 && m[0].0 == 1));
        // created with offset → local date in TIME_ZONE (23:30Z = next day in Zurich)
        let v = run(&form(&[("created", "2025-01-01T23:30:00Z")], Some(("t.txt", b"hello")), dir.path())).await.unwrap();
        assert_eq!(v.created, chrono::NaiveDate::from_ymd_opt(2025, 1, 2));
        let v = run(&form(&[], Some(("t.txt", b"hello")), dir.path())).await.unwrap();
        assert_eq!(v.from_webui, Some(false));
        assert_eq!(v.custom_fields, None);
        assert_eq!(v.ai_ocr, None);
        // PaperRust ai_ocr (AI text recognition)
        let v = run(&form(&[("ai_ocr", "true")], Some(("t.txt", b"hello")), dir.path())).await.unwrap();
        assert_eq!(v.ai_ocr, Some(true));
        let e = run(&form(&[("ai_ocr", "vielleicht")], Some(("t.txt", b"hello")), dir.path())).await.unwrap_err();
        assert_eq!(e["ai_ocr"][0], "Must be a valid boolean.");
    }

    #[tokio::test]
    async fn custom_field_values() {
        for (cf, msg) in [
            (r#"{"10": "zz"}"#, Some("Value must be an id of an element in [{'id': 'a1', 'label': 'Low'}]")),
            (r#"{"9": 5}"#, Some("Value must be a list")),
            (r#"{"9": [1, 1]}"#, Some("Some documents in value don't exist or were specified twice.")),
            (r#"{"3": "2025-13-01"}"#, Some(pl_core::datetime::DRF_DATE_INVALID)),
            (r#"{"7": "12.345"}"#, Some("Must be a two-decimal number with optional currency code e.g. GBP123.45")),
            (r#"{"7": "12.34"}"#, None),
            (r#"{"7": 12.5}"#, None),
            (r#"{"7": ""}"#, None),
            (r#"{"2": "no-scheme"}"#, Some("Unable to parse URI no-scheme, missing scheme")),
            (r#"{"2": "ftp://x"}"#, None),
            (r#"{"5": "99999999999"}"#, Some("Ensure this value is less than or equal to 2147483647.")),
            (r#"{"5": 3}"#, None),
            (r#"{"5": null}"#, None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let r = run(&form(&[("custom_fields", cf)], Some(("t.txt", b"hello")), dir.path())).await;
            match msg {
                None => assert!(r.is_ok(), "{cf}: {r:?}"),
                Some(m) => assert_eq!(r.unwrap_err()["custom_fields"][0], m, "{cf}"),
            }
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(sanitize_file_name("C:\\x\\a&amp;b.pdf").as_deref(), Some("a&b.pdf"));
        assert_eq!(sanitize_file_name("../..").as_deref(), None);
        assert_eq!(sanitize_file_name("dir/file\u{0}.txt").as_deref(), Some("file.txt"));
        assert_eq!(integer_field("1.000", 0, ASN_MAX), Ok(1));
        assert_eq!(integer_field("1.5", 0, ASN_MAX), Err("A valid integer is required.".into()));
        assert!(integer_field(&"9".repeat(60), 0, ASN_MAX).unwrap_err().contains("less than"));
        assert!(decimal_12_2("1234567890.12") && !decimal_12_2("12345678901") && !decimal_12_2("1.234") && !decimal_12_2("abc"));
        assert!(decimal_12_2("1e2") && !decimal_12_2("1e20") && decimal_12_2("0.00"));
        assert!(monetary_regex("GBP123.45") && monetary_regex("EUR-1.5") && !monetary_regex("EUR12"));
        assert!(py_eq(&Value::from(1), &Value::from(1.0)) && !py_eq(&Value::from("1"), &Value::from(1)));
    }
}
