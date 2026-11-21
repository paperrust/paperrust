//! `/api/config/{pk}/` write side – `ApplicationConfigurationViewSet.update` / `partial_update` /
//! `destroy` (standalone without Django, docs/STANDALONE-GAPS.md).
//!
//! upstream: src/paperless/views.py@v3.4.2 (`ApplicationConfigurationViewSet.perform_update`),
//! src/paperless/serialisers.py@v3.4.2 (`ApplicationConfigurationSerializer.run_validation`,
//! `validate_user_args`, `validate_barcode_tag_mapping`, `update`, `validate_app_logo`,
//! `validate_llm_endpoint`, `validate_remote_ocr_endpoint`), src/paperless/models.py@v3.4.2
//! (`ApplicationConfiguration`, `AbstractSingletonModel.save` → pk 1), src/paperless/network.py
//! (`validate_outbound_http_url`, `is_public_ip`), src/paperless/validators.py
//! (`reject_dangerous_svg`, `validate_raster_image`), src/paperless_mail/serialisers.py
//! (`ObfuscatedPasswordField`), rest_framework/fields.py@3.18.3 (`CharField`, `ChoiceField`,
//! `IntegerField`, `FloatField`, `BooleanField`, `JSONField(binary=True)`, `FileField`,
//! `Field.get_value` for HTML input), rest_framework/mixins.py (`UpdateModelMixin`,
//! `DestroyModelMixin`), django/http/multipartparser.py (`sanitize_file_name`).
//!
//! Permissions: `DjangoModelPermissions` (PUT/PATCH `change_`, DELETE `delete_applicationconfiguration`),
//! checked before the object lookup; the body is parsed after the lookup (DRF reads
//! `request.data` lazily in `update`).
//!
//! Known differences (docs/contract/pl-system.md): MIME sniffing of the logo is a magic-number
//! check instead of libmagic (SVG/PNG/JPEG/GIF); other raster formats with an allowed extension are
//! rejected as "Invalid logo image." (Pillow would accept e.g. BMP). Multipart bodies are read up
//! to [`MULTIPART_LIMIT`] (Django spools larger files to disk). `json.loads` uses serde (no
//! `NaN`/`Infinity`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::{FromRequest, Multipart, Path as UrlPath, Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use pl_authz::AuthUser;
use pl_core::{ApiError, AppState, Drf, RequestContext};
use serde_json::{Map, Value};

use crate::error::{Error, Result};

const TABLE: &str = "paperless_applicationconfiguration";

/// Largest multipart body (logo upload) read into memory.
pub const MULTIPART_LIMIT: usize = 64 * 1024 * 1024;

/// `AbstractSingletonModel.save()` always writes this pk.
const SINGLETON_ID: i32 = 1;

// ---------------------------------------------------------------------------------------------
// Request data
// ---------------------------------------------------------------------------------------------

/// An uploaded file (`UploadedFile`: sanitized name + content).
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub name: String,
    pub data: Vec<u8>,
}

/// One value of a form/multipart `QueryDict` (text) or of the merged `MultiValueDict` (files).
#[derive(Debug, Clone, PartialEq)]
pub enum FormItem {
    Text(String),
    /// After `run_validation` set `data[field] = None` on a `QueryDict`.
    Null,
    File(Upload),
}

/// DRF `request.data`.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Json(Value),
    Form(Vec<(String, FormItem)>),
}

impl Data {
    fn contains(&self, key: &str) -> bool {
        match self {
            Data::Json(Value::Object(m)) => m.contains_key(key),
            Data::Json(_) => false,
            Data::Form(p) => p.iter().any(|(k, _)| k == key),
        }
    }

    /// `data[key]` (QueryDict: last value).
    fn get(&self, key: &str) -> Option<FormItem> {
        match self {
            Data::Json(Value::Object(m)) => m.get(key).map(|v| match v {
                Value::Null => FormItem::Null,
                Value::String(s) => FormItem::Text(s.clone()),
                // non-string JSON values never reach a FormItem consumer
                other => FormItem::Text(other.to_string()),
            }),
            Data::Json(_) => None,
            Data::Form(p) => p.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
        }
    }

    fn json_get(&self, key: &str) -> Option<&Value> {
        match self {
            Data::Json(Value::Object(m)) => m.get(key),
            _ => None,
        }
    }

    fn set_null(&mut self, key: &str) {
        match self {
            Data::Json(Value::Object(m)) => {
                m.insert(key.into(), Value::Null);
            }
            Data::Json(_) => {}
            Data::Form(p) => {
                p.retain(|(k, _)| k != key);
                p.push((key.into(), FormItem::Null));
            }
        }
    }

    fn remove(&mut self, key: &str) {
        match self {
            Data::Json(Value::Object(m)) => {
                m.remove(key);
            }
            Data::Json(_) => {}
            Data::Form(p) => p.retain(|(k, _)| k != key),
        }
    }
}

/// Django `MultiPartParser.sanitize_file_name`.
pub fn sanitize_file_name(name: &str) -> Option<String> {
    let unescaped = html_unescape_basic(name);
    let base = unescaped.rsplit('/').next().unwrap_or("");
    let base = base.rsplit('\\').next().unwrap_or("");
    let s: String = base.chars().filter(|c| !c.is_control() && !matches!(*c, '\u{ad}' | '\u{2028}' | '\u{2029}')).collect();
    if s.is_empty() || s == "." || s == ".." {
        return None;
    }
    Some(s)
}

/// `html.unescape` for the entities a browser may put into a file name.
fn html_unescape_basic(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&quot;", "\"").replace("&#x27;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn media_main(ct: Option<&str>) -> String {
    ct.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// `Request._parse()` with JSON, form and multipart parsers (files kept). The flag tells whether
/// `request.data` is mutable: a form `QueryDict` is immutable unless files were uploaded
/// (`_full_data = _data.copy(); update(_files)`); a JSON dict is always mutable.
async fn parse_request(req: Request) -> Result<(Data, bool)> {
    let ct = req.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let main = media_main(ct.as_deref());
    if main == "multipart/form-data" {
        let empty = req.headers().get(axum::http::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).is_some_and(|v| v.trim() == "0");
        if empty {
            return Ok((Data::Form(Vec::new()), false));
        }
        let mut mp = Multipart::from_request(req, &())
            .await
            .map_err(|_| ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")))?;
        let mut out = Vec::new();
        let mut text_total = 0usize;
        let mut total = 0usize;
        loop {
            let field = match mp.next_field().await {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => return Err(ApiError::parse_error(Some(&format!("Multipart form parse error - {e}"))).into()),
            };
            let name = field.name().unwrap_or("").to_string();
            let file_name = field.file_name().map(str::to_string);
            let bytes = field.bytes().await.map_err(|e| ApiError::parse_error(Some(&format!("Multipart form parse error - {e}"))))?;
            total += bytes.len();
            if total > MULTIPART_LIMIT {
                return Err(ApiError::django_bad_request().into());
            }
            match file_name {
                Some(f) => {
                    // FILE item without a usable name is skipped (`if not file_name: continue`)
                    if let Some(n) = sanitize_file_name(&f) {
                        out.push((name, FormItem::File(Upload { name: n, data: bytes.to_vec() })));
                    }
                }
                None => {
                    text_total += bytes.len();
                    if text_total > pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE {
                        return Err(ApiError::django_bad_request().into());
                    }
                    out.push((name, FormItem::Text(String::from_utf8_lossy(&bytes).into_owned())));
                }
            }
        }
        let has_files = out.iter().any(|(_, v)| matches!(v, FormItem::File(_)));
        return Ok((Data::Form(out), has_files));
    }
    let bytes = pl_core::body::read_body(req.into_body()).await?;
    match crate::tasks::body::parse_bytes(ct.as_deref(), &bytes)? {
        crate::tasks::body::Data::Json(v) => Ok((Data::Json(v), true)),
        crate::tasks::body::Data::Form(p) => Ok((Data::Form(p.into_iter().map(|(k, v)| (k, FormItem::Text(v))).collect()), false)),
    }
}

// ---------------------------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------------------------

const OUTPUT_TYPE: &[&str] = &["pdf", "pdfa", "pdfa-1", "pdfa-2", "pdfa-3"];
const MODE: &[&str] = &["auto", "force", "redo", "off"];
const ARCHIVE_FILE_GENERATION: &[&str] = &["auto", "always", "never"];
const CLEAN: &[&str] = &["clean", "clean-final", "none"];
const COLOR_CONVERT: &[&str] = &["LeaveColorUnchanged", "RGB", "UseDeviceIndependentColor", "Gray", "CMYK"];
const REMOTE_OCR_ENGINE: &[&str] = &["azureai"];
const REMOTE_OCR_MODE: &[&str] = &["always", "workflow_only"];
const LLM_EMBEDDING_BACKEND: &[&str] = &["openai-like", "huggingface", "ollama"];
const LLM_BACKEND: &[&str] = &["openai-like", "ollama"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Endpoint {
    Llm,
    RemoteOcr,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FieldKind {
    Choice(&'static [&'static str]),
    Char(usize),
    /// `CharField(max_length=256)` + `validate_*_endpoint`.
    Url(Endpoint),
    /// `PositiveSmallIntegerField(validators=[MinValueValidator(1)])`.
    SmallInt,
    /// `PositiveIntegerField(validators=[MinValueValidator(1)])`.
    Int,
    /// `FloatField(validators=[MinValueValidator(min)])`.
    Float(f64),
    Bool,
    /// `JSONField(binary=True, allow_null=True)`; `true` = `barcode_tag_mapping` (string values).
    Json(bool),
    /// `ObfuscatedPasswordField(required=False, allow_null=True, max_length=1024)`.
    Obfuscated,
    /// `FileField(upload_to="logo/", max_length=100, FileExtensionValidator)`.
    File,
}

/// Writable serializer fields in `fields` order (declared fields first, then the model fields).
const FIELDS: &[(&str, FieldKind)] = &[
    ("user_args", FieldKind::Json(false)),
    ("barcode_tag_mapping", FieldKind::Json(true)),
    ("llm_embedding_api_key", FieldKind::Obfuscated),
    ("llm_api_key", FieldKind::Obfuscated),
    ("remote_ocr_api_key", FieldKind::Obfuscated),
    ("output_type", FieldKind::Choice(OUTPUT_TYPE)),
    ("pages", FieldKind::SmallInt),
    ("language", FieldKind::Char(32)),
    ("mode", FieldKind::Choice(MODE)),
    ("archive_file_generation", FieldKind::Choice(ARCHIVE_FILE_GENERATION)),
    ("image_dpi", FieldKind::SmallInt),
    ("unpaper_clean", FieldKind::Choice(CLEAN)),
    ("deskew", FieldKind::Bool),
    ("rotate_pages", FieldKind::Bool),
    ("rotate_pages_threshold", FieldKind::Float(0.0)),
    ("max_image_pixels", FieldKind::Float(0.0)),
    ("color_conversion_strategy", FieldKind::Choice(COLOR_CONVERT)),
    ("app_title", FieldKind::Char(48)),
    ("app_logo", FieldKind::File),
    ("barcodes_enabled", FieldKind::Bool),
    ("barcode_enable_tiff_support", FieldKind::Bool),
    ("barcode_string", FieldKind::Char(32)),
    ("barcode_retain_split_pages", FieldKind::Bool),
    ("barcode_enable_asn", FieldKind::Bool),
    ("barcode_asn_prefix", FieldKind::Char(32)),
    ("barcode_upscale", FieldKind::Float(1.0)),
    ("barcode_dpi", FieldKind::SmallInt),
    ("barcode_max_pages", FieldKind::SmallInt),
    ("barcode_enable_tag", FieldKind::Bool),
    ("barcode_tag_split", FieldKind::Bool),
    ("barcode_store_values", FieldKind::Bool),
    ("remote_ocr_engine", FieldKind::Choice(REMOTE_OCR_ENGINE)),
    ("remote_ocr_endpoint", FieldKind::Url(Endpoint::RemoteOcr)),
    ("remote_ocr_mode", FieldKind::Choice(REMOTE_OCR_MODE)),
    ("ai_enabled", FieldKind::Bool),
    ("llm_embedding_backend", FieldKind::Choice(LLM_EMBEDDING_BACKEND)),
    ("llm_embedding_model", FieldKind::Char(128)),
    ("llm_embedding_endpoint", FieldKind::Url(Endpoint::Llm)),
    ("llm_embedding_chunk_size", FieldKind::SmallInt),
    ("llm_context_size", FieldKind::Int),
    ("llm_backend", FieldKind::Choice(LLM_BACKEND)),
    ("llm_model", FieldKind::Char(128)),
    ("llm_endpoint", FieldKind::Url(Endpoint::Llm)),
    ("llm_output_language", FieldKind::Char(32)),
    ("llm_request_timeout", FieldKind::SmallInt),
];

const OBFUSCATED: &[&str] = &["llm_embedding_api_key", "llm_api_key", "remote_ocr_api_key"];

/// Validated value of one field.
#[derive(Debug, Clone, PartialEq)]
pub enum Validated {
    /// Column value (JSON for `jsonb_populate_record`).
    Value(Value),
    /// New logo (sanitized name, bytes to store).
    Logo(String, Vec<u8>),
}

/// Input of one field after `get_value` + empty handling.
#[derive(Debug, Clone, PartialEq)]
enum Input {
    Json(Value),
    /// `JSONString` from HTML input (`None` after `run_validation` → `"None"`).
    Text(String),
    File(Upload),
    Null,
    /// Missing although required.
    Required,
}

/// What the field validation needs from the environment.
#[derive(Debug, Clone, Default)]
pub struct ValidationEnv {
    pub max_image_pixels: Option<i64>,
    pub llm_allow_internal: bool,
    pub remote_ocr_allow_internal: bool,
}

/// `get_bool_from_env(key, default)`.
fn env_bool(key: &str, default: &str) -> bool {
    let v = pl_core::config::var(key).unwrap_or_else(|| default.to_string());
    matches!(v.to_lowercase().as_str(), "yes" | "y" | "1" | "t" | "true")
}

impl ValidationEnv {
    pub fn from_env() -> Self {
        Self {
            max_image_pixels: pl_core::config::var("PAPERLESS_MAX_IMAGE_PIXELS")
                .and_then(|v| crate::py::int(&v))
                .and_then(|v| i64::try_from(v).ok()),
            llm_allow_internal: env_bool("PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS", "true"),
            remote_ocr_allow_internal: env_bool("PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS", "true"),
        }
    }
}

fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) if n.is_f64() => pl_core::json::python_float_repr(n.as_f64().unwrap_or(0.0)),
        other => pl_core::pyrepr::repr_value(other),
    }
}

/// `ApplicationConfigurationSerializer.run_validation` pre-processing. `Err` = unhandled
/// Python exception (`AttributeError` on a non-string API key, or any assignment to an
/// immutable `QueryDict`) → 500.
fn preprocess(data: &mut Data, mutable: bool) -> Result<()> {
    let immutable = || -> Error { ApiError::internal("AttributeError: This QueryDict instance is immutable").into() };
    for f in ["user_args", "barcode_tag_mapping", "language", "llm_output_language"] {
        if data.contains(f) && is_empty_string(data, f) {
            if !mutable {
                return Err(immutable());
            }
            data.set_null(f);
        }
    }
    for f in OBFUSCATED {
        if !data.contains(f) {
            continue;
        }
        let s = match data {
            Data::Json(_) => match data.json_get(f) {
                None | Some(Value::Null) => continue,
                Some(Value::String(s)) => s.clone(),
                Some(_) => return Err(ApiError::internal(format!("AttributeError: {f}.replace")).into()),
            },
            Data::Form(_) => match data.get(f) {
                Some(FormItem::Text(s)) => s,
                Some(FormItem::Null) | None => continue,
                Some(FormItem::File(_)) => return Err(ApiError::internal(format!("AttributeError: {f}.replace")).into()),
            },
        };
        if s.is_empty() {
            if !mutable {
                return Err(immutable());
            }
            data.set_null(f);
        } else if s.replace('*', "").is_empty() {
            if !mutable {
                return Err(immutable());
            }
            data.remove(f);
        }
    }
    Ok(())
}

fn is_empty_string(data: &Data, f: &str) -> bool {
    match data {
        Data::Json(_) => data.json_get(f) == Some(&Value::String(String::new())),
        Data::Form(_) => data.get(f) == Some(FormItem::Text(String::new())),
    }
}

/// `Field.get_value` + `validate_empty_values` (`allow_null=True` for every field here).
/// `None` = skip the field.
fn field_input(data: &Data, name: &str, kind: FieldKind, partial: bool) -> Option<Input> {
    // The declared JSONFields (`user_args`, `barcode_tag_mapping`) have no `required=False`:
    // missing on a full update → "This field is required."
    if !partial && matches!(kind, FieldKind::Json(_)) && !data.contains(name) {
        return Some(Input::Required);
    }
    match data {
        Data::Json(_) => data.json_get(name).map(|v| if v.is_null() { Input::Null } else { Input::Json(v.clone()) }),
        Data::Form(_) => {
            if let FieldKind::Json(_) = kind {
                // JSONField.get_value: JSONString(dictionary[name]) (None → "None")
                return data.get(name).map(|v| match v {
                    FormItem::Text(s) => Input::Text(s),
                    FormItem::Null => Input::Text("None".into()),
                    FormItem::File(u) => Input::File(u),
                });
            }
            let Some(item) = data.get(name) else {
                // BooleanField(allow_null=True).default_empty_html = None; others: empty
                return (!partial && kind == FieldKind::Bool).then_some(Input::Null);
            };
            let allow_blank = matches!(kind, FieldKind::Choice(_) | FieldKind::Char(_) | FieldKind::Url(_));
            match item {
                FormItem::Null => Some(Input::Null),
                FormItem::Text(s) if s.is_empty() => Some(if allow_blank { Input::Text(s) } else { Input::Null }),
                FormItem::Text(s) => Some(Input::Text(s)),
                FormItem::File(u) => Some(Input::File(u)),
            }
        }
    }
}

fn input_value(i: &Input) -> Value {
    match i {
        Input::Json(v) => v.clone(),
        Input::Text(s) => Value::String(s.clone()),
        Input::File(u) => Value::String(u.name.clone()),
        Input::Null | Input::Required => Value::Null,
    }
}

const NOT_A_FILE: &str = "The submitted data was not a file. Check the encoding type on the form.";

/// Python `str.strip()`.
fn py_strip(s: &str) -> &str {
    s.trim_matches(crate::py::is_py_space)
}

/// Python `float(str)` (no `inf`/`nan` check).
fn py_float_str(s: &str) -> Option<f64> {
    let t = py_strip(s);
    if t.contains('_') {
        let ok =
            t.split('_').all(|p| !p.is_empty()) && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '_' | '.' | 'e' | 'E' | '+' | '-'));
        if !ok {
            return None;
        }
        return t.replace('_', "").parse().ok();
    }
    t.parse().ok()
}

/// `re.sub(r'\.0*\s*$', '', s)`.
fn strip_decimal_zeros(s: &str) -> &str {
    let t = s.trim_end_matches(crate::py::is_py_space);
    let t2 = t.trim_end_matches('0');
    t2.strip_suffix('.').unwrap_or(s)
}

fn char_validators(value: &str, max: usize, errs: &mut Vec<String>) {
    if value.chars().count() > max {
        errs.push(format!("Ensure this field has no more than {max} characters."));
    }
    if value.contains('\0') {
        errs.push("Null characters are not allowed.".into());
    }
}

/// OCRmyPDF arguments the OCR parser sets itself (its own temp files); sorted like Python's
/// `sorted(OCR_RESERVED_USER_ARGS & value.keys())`.
///
/// upstream fix d05e96cda (#14734, paperless-ngx `dev` after v3.4.2): `paperless/config.py`
/// `OCR_RESERVED_USER_ARGS` + `ApplicationConfigurationSerializer.validate_user_args`. Same list
/// as `pl_ocr::args::OCR_RESERVED_USER_ARGS` (kept here to avoid a pl-ocr dependency).
pub const OCR_RESERVED_USER_ARGS: &[&str] = &["input_file_or_options", "output_file", "sidecar"];

fn reserved_user_args(m: &serde_json::Map<String, Value>) -> Vec<&'static str> {
    OCR_RESERVED_USER_ARGS.iter().copied().filter(|k| m.contains_key(*k)).collect()
}

/// One field: `run_validation` + `validate_<field>`. `Ok(None)` = field skipped.
async fn validate_field(name: &str, kind: FieldKind, input: Input, env: &ValidationEnv) -> std::result::Result<Validated, Vec<String>> {
    let v = |x: Value| Ok(Validated::Value(x));
    if input == Input::Required {
        return Err(vec!["This field is required.".into()]);
    }
    match kind {
        FieldKind::Json(tag_mapping) => {
            let parsed: Value = match &input {
                Input::Null => Value::Null,
                Input::Text(s) => serde_json::from_str(s).map_err(|_| vec!["Value must be valid JSON.".to_string()])?,
                Input::Json(Value::String(s)) => serde_json::from_str(s).map_err(|_| vec!["Value must be valid JSON.".to_string()])?,
                _ => return Err(vec!["Value must be valid JSON.".into()]),
            };
            if !parsed.is_null() && !parsed.is_object() {
                return Err(vec![format!("{name} must be a JSON object.")]);
            }
            if tag_mapping
                && let Value::Object(m) = &parsed
                && !m.values().all(Value::is_string)
            {
                return Err(vec!["barcode_tag_mapping values must be strings.".into()]);
            }
            if name == "user_args"
                && let Value::Object(m) = &parsed
            {
                let reserved = reserved_user_args(m);
                if !reserved.is_empty() {
                    return Err(vec![format!("user_args may not set arguments managed by PaperRust: {}.", reserved.join(", "))]);
                }
            }
            v(parsed)
        }
        FieldKind::Obfuscated => {
            let s = match input {
                Input::Null => return v(Value::Null),
                Input::Json(Value::String(s)) | Input::Text(s) => s,
                _ => return Err(vec![NOT_A_STRING.into()]),
            };
            if py_strip(&s).is_empty() {
                return Err(vec!["This field may not be blank.".into()]);
            }
            let mut errs = Vec::new();
            char_validators(&s, 1024, &mut errs);
            if errs.is_empty() { v(Value::String(s)) } else { Err(errs) }
        }
        FieldKind::Choice(choices) => {
            let raw = match input {
                Input::Null => return v(Value::Null),
                other => input_value(&other),
            };
            if raw == Value::String(String::new()) {
                return v(raw);
            }
            let s = py_str(&raw);
            if choices.contains(&s.as_str()) { v(Value::String(s)) } else { Err(vec![format!("\"{s}\" is not a valid choice.")]) }
        }
        FieldKind::Char(_) | FieldKind::Url(_) => {
            let max = if let FieldKind::Char(m) = kind { m } else { 256 };
            let raw = match input {
                Input::Null => return v(Value::Null),
                other => input_value(&other),
            };
            let as_str = match &raw {
                Value::String(s) => Some(s.clone()),
                Value::Number(_) => Some(py_str(&raw)),
                _ => None,
            };
            // CharField.run_validation: '' / whitespace → '' (allow_blank)
            if as_str.as_deref().map(py_strip) == Some("") || (as_str.is_none() && py_strip(&py_str(&raw)).is_empty()) {
                return v(Value::String(String::new()));
            }
            let Some(s) = as_str else { return Err(vec![NOT_A_STRING.into()]) };
            let s = py_strip(&s).to_string();
            let mut errs = Vec::new();
            char_validators(&s, max, &mut errs);
            if !errs.is_empty() {
                return Err(errs);
            }
            if let FieldKind::Url(ep) = kind {
                let allow_internal = match ep {
                    Endpoint::Llm => env.llm_allow_internal,
                    Endpoint::RemoteOcr => env.remote_ocr_allow_internal,
                };
                if let Err(m) = validate_outbound_http_url(&s, allow_internal).await {
                    return Err(vec![match ep {
                        Endpoint::Llm => format!("Invalid LLM endpoint: {m}, see logs for details"),
                        Endpoint::RemoteOcr => format!("Invalid remote OCR endpoint: {m}, see logs for details"),
                    }]);
                }
            }
            v(Value::String(s))
        }
        FieldKind::SmallInt | FieldKind::Int => {
            let raw = match input {
                Input::Null => return v(Value::Null),
                other => input_value(&other),
            };
            if let Value::String(s) = &raw
                && s.chars().count() > 1000
            {
                return Err(vec!["String value too large.".into()]);
            }
            let s = match &raw {
                Value::Bool(true) => "True".to_string(),
                Value::Bool(false) => "False".to_string(),
                other => py_str(other),
            };
            let Some(i) = crate::py::int(strip_decimal_zeros(&s)) else {
                return Err(vec!["A valid integer is required.".into()]);
            };
            let max: i128 = if kind == FieldKind::SmallInt { 32767 } else { 2_147_483_647 };
            if i > max {
                return Err(vec![format!("Ensure this value is less than or equal to {max}.")]);
            }
            if i < 1 {
                return Err(vec!["Ensure this value is greater than or equal to 1.".into()]);
            }
            v(Value::from(i as i64))
        }
        FieldKind::Float(min) => {
            let raw = match input {
                Input::Null => return v(Value::Null),
                other => input_value(&other),
            };
            let f = match &raw {
                Value::String(s) if s.chars().count() > 1000 => return Err(vec!["String value too large.".into()]),
                Value::String(s) => py_float_str(s),
                Value::Number(n) => n.as_f64(),
                Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                _ => None,
            };
            let Some(f) = f.filter(|f| f.is_finite()) else {
                return Err(vec!["A valid number is required.".into()]);
            };
            if f < min {
                return Err(vec![format!("Ensure this value is greater than or equal to {}.", pl_core::json::python_float_repr(min))]);
            }
            match serde_json::Number::from_f64(f) {
                Some(n) => v(Value::Number(n)),
                None => Err(vec!["A valid number is required.".into()]),
            }
        }
        FieldKind::Bool => {
            const TRUE: &[&str] = &["t", "y", "yes", "true", "on", "1"];
            const FALSE: &[&str] = &["f", "n", "no", "false", "off", "0"];
            match input {
                Input::Null => v(Value::Null),
                other => match input_value(&other) {
                    Value::Bool(b) => v(Value::Bool(b)),
                    Value::Number(n) if n.as_f64() == Some(1.0) => v(Value::Bool(true)),
                    Value::Number(n) if n.as_f64() == Some(0.0) => v(Value::Bool(false)),
                    Value::String(s) => {
                        let l = s.to_lowercase();
                        if TRUE.contains(&l.as_str()) {
                            v(Value::Bool(true))
                        } else if FALSE.contains(&l.as_str()) {
                            v(Value::Bool(false))
                        } else if l == "null" || l.is_empty() {
                            v(Value::Null)
                        } else {
                            Err(vec!["Must be a valid boolean.".into()])
                        }
                    }
                    _ => Err(vec!["Must be a valid boolean.".into()]),
                },
            }
        }
        FieldKind::File => {
            let u = match input {
                Input::Null => return v(Value::Null),
                Input::File(u) => u,
                _ => return Err(vec![NOT_A_FILE.into()]),
            };
            if u.data.is_empty() {
                return Err(vec!["The submitted file is empty.".into()]);
            }
            let len = u.name.chars().count();
            if len > 100 {
                return Err(vec![format!("Ensure this filename has at most 100 characters (it has {len}).")]);
            }
            // FileExtensionValidator: Path(name).suffix[1:].lower()
            let ext = path_suffix(&u.name).to_lowercase();
            if !["jpg", "png", "gif", "svg"].contains(&ext.as_str()) {
                return Err(vec![format!(
                    "File extension \u{201c}{ext}\u{201d} is not allowed. Allowed extensions are: jpg, png, gif, svg."
                )]);
            }
            let (clean, bytes) = pl_settings::logo::validate(&u.name, &u.data, env.max_image_pixels).map_err(|m| vec![m])?;
            Ok(Validated::Logo(clean, bytes))
        }
    }
}

const NOT_A_STRING: &str = "Not a valid string.";

/// `pathlib.Path(name).suffix` without the dot.
fn path_suffix(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 && i + 1 < base.len() => &base[i + 1..],
        _ => "",
    }
}

/// Field-level validation of the whole serializer. `Ok(map)` = validated data (field order),
/// `Err(Value)` = 400 body.
pub async fn validate_data(
    mut data: Data,
    mutable: bool,
    partial: bool,
    env: &ValidationEnv,
) -> Result<std::result::Result<Vec<(&'static str, Validated)>, Value>> {
    preprocess(&mut data, mutable)?;
    if let Data::Json(v) = &data
        && !v.is_object()
    {
        let mut m = Map::new();
        m.insert(
            "non_field_errors".into(),
            Value::Array(vec![Value::from(format!("Invalid data. Expected a dictionary, but got {}.", py_type(v)))]),
        );
        return Ok(Err(Value::Object(m)));
    }
    let mut ok = Vec::new();
    let mut errs = Map::new();
    for (name, kind) in FIELDS {
        let Some(input) = field_input(&data, name, *kind, partial) else { continue };
        match validate_field(name, *kind, input, env).await {
            Ok(v) => ok.push((*name, v)),
            Err(m) => {
                errs.insert((*name).into(), Value::Array(m.into_iter().map(Value::from).collect()));
            }
        }
    }
    Ok(if errs.is_empty() { Ok(ok) } else { Err(Value::Object(errs)) })
}

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

// ---------------------------------------------------------------------------------------------
// validate_outbound_http_url
// ---------------------------------------------------------------------------------------------

/// `urlparse(url)` → (scheme, hostname, port) like Python (`Err` = `parsed.port` ValueError).
fn py_urlparse(url: &str) -> (String, Option<String>, std::result::Result<Option<u32>, ()>) {
    let (scheme, rest) = match url.split_once(':') {
        Some((s, r))
            if !s.is_empty()
                && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            (s.to_ascii_lowercase(), r)
        }
        _ => (String::new(), url),
    };
    let Some(after) = rest.strip_prefix("//") else {
        return (scheme, None, Ok(None));
    };
    let netloc = after.split(['/', '?', '#']).next().unwrap_or("");
    let hostinfo = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let (host, port) = if let Some(r) = hostinfo.strip_prefix('[') {
        match r.split_once(']') {
            Some((h, p)) => (h.to_string(), p.strip_prefix(':').unwrap_or("").to_string()),
            None => (r.to_string(), String::new()),
        }
    } else {
        match hostinfo.split_once(':') {
            Some((h, p)) => (h.to_string(), p.to_string()),
            None => (hostinfo.to_string(), String::new()),
        }
    };
    let host = (!host.is_empty()).then(|| host.to_lowercase());
    let port = if port.is_empty() {
        Ok(None)
    } else if port.bytes().all(|b| b.is_ascii_digit()) {
        match port.parse::<u32>() {
            Ok(p) if p <= 65535 => Ok(Some(p)),
            _ => Err(()),
        }
    } else {
        Err(())
    };
    (scheme, host, port)
}

/// `paperless.network.is_public_ip`.
pub fn is_public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_multicast()
                || v.is_broadcast()
                || v.is_unspecified()
                || v.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240)
        }
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(m));
            }
            let s = v.segments();
            let n = u128::from(v);
            let within = |net: u128, bits: u32| (n ^ net).checked_shr(128 - bits).unwrap_or(0) == 0;
            let ietf = within(0x2001_0000 << 96, 23)
                && !(n == (0x2001_0001 << 96 | 1)
                    || n == (0x2001_0001 << 96 | 2)
                    || within(0x2001_0003 << 96, 32)
                    || within((0x2001_0004_0112_u128) << 80, 48)
                    || within(0x2001_0020 << 96, 28)
                    || within(0x2001_0030 << 96, 28));
            !(v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || within(0x0064_ff9b << 96, 96)
                || within(0x0100 << 112, 64)
                || ietf
                || within(0x2001_0db8 << 96, 32)
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// `validate_outbound_http_url(url, allow_internal=...)` → `Err(e.args[0])`.
pub async fn validate_outbound_http_url(url: &str, allow_internal: bool) -> std::result::Result<(), String> {
    let (scheme, host, port) = py_urlparse(url);
    let Some(host) = host.filter(|_| scheme == "http" || scheme == "https") else {
        return Err("Invalid URL scheme or hostname.".into());
    };
    if port.is_err() {
        return Err("Invalid URL scheme or hostname.".into());
    }
    if allow_internal {
        return Ok(());
    }
    let ips: Vec<std::net::IpAddr> = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => match tokio::net::lookup_host((host.as_str(), 0)).await {
            Ok(a) => a.map(|a| a.ip()).collect(),
            Err(_) => return Err(format!("Could not resolve hostname: {host}")),
        },
    };
    if ips.is_empty() {
        return Err(format!("Could not resolve hostname: {host}"));
    }
    if ips.iter().any(|ip| !is_public_ip(*ip)) {
        return Err(format!("Connection blocked: {host} resolves to a non-public address"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// AI index rebuild (perform_update)
// ---------------------------------------------------------------------------------------------

/// `llm_index_exists()` – set by pl-server when the AI feature is built in (pl-system does not
/// depend on pl-ai). Without it the index counts as missing.
#[derive(Clone)]
pub struct LlmIndexExists(pub Arc<LlmIndexProbe>);

/// Probe behind [`LlmIndexExists`].
pub type LlmIndexProbe = dyn Fn(AppState) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync;

/// The values `perform_update` compares (`x or settings.X`).
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingConfig {
    pub backend: Option<String>,
    pub chunk_size: i64,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub context_size: i64,
}

fn truthy_str(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn env_str(key: &str) -> Option<String> {
    pl_core::config::var(key).filter(|s| !s.is_empty())
}

fn env_int(key: &str, default: i64) -> i64 {
    pl_core::config::var(key).and_then(|v| crate::py::int(&v)).and_then(|v| i64::try_from(v).ok()).unwrap_or(default)
}

/// Embedding configuration of a row (`to_jsonb` object).
pub fn embedding_config(row: &Map<String, Value>) -> EmbeddingConfig {
    let int_or = |k: &str, env: &str, d: i64| row.get(k).and_then(Value::as_i64).filter(|v| *v != 0).unwrap_or_else(|| env_int(env, d));
    EmbeddingConfig {
        backend: row.get("llm_embedding_backend").and_then(truthy_str).or_else(|| env_str("PAPERLESS_AI_LLM_EMBEDDING_BACKEND")),
        chunk_size: int_or("llm_embedding_chunk_size", "PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE", 1024),
        endpoint: row.get("llm_embedding_endpoint").and_then(truthy_str).or_else(|| env_str("PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT")),
        model: row.get("llm_embedding_model").and_then(truthy_str).or_else(|| env_str("PAPERLESS_AI_LLM_EMBEDDING_MODEL")),
        context_size: int_or("llm_context_size", "PAPERLESS_AI_LLM_CONTEXT_SIZE", 8192),
    }
}

/// `new_ai_index_enabled` of the new row.
pub fn ai_index_enabled(row: &Map<String, Value>, emb: &EmbeddingConfig) -> bool {
    let ai = match row.get("ai_enabled") {
        Some(Value::Bool(b)) => *b,
        _ => env_bool("PAPERLESS_AI_ENABLED", "NO"),
    };
    ai && emb.backend.is_some()
}

async fn maybe_rebuild_llm_index(state: &AppState, old: &Map<String, Value>, new: &Map<String, Value>) {
    let old_emb = embedding_config(old);
    let new_emb = embedding_config(new);
    if !ai_index_enabled(new, &new_emb) {
        return;
    }
    let exists = match state.service::<LlmIndexExists>() {
        Some(LlmIndexExists(f)) => f(state.clone()).await,
        None => false,
    };
    if exists && old_emb == new_emb {
        return;
    }
    let req = pl_core::tasks::TaskRequest::new("documents.tasks.llmindex_index").kwarg("rebuild", true).trigger_source("system");
    if let Err(e) = pl_core::tasks::send_task(state, req).await {
        tracing::error!(error = %e, "could not queue llmindex_index after a configuration change");
    }
}

// ---------------------------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------------------------

/// `get_object()`: pk → existing row id.
async fn get_object(state: &AppState, pk: &str) -> Result<i32> {
    let Some(pk) = crate::py::int(pk) else {
        return Err(ApiError::not_found().into());
    };
    let Ok(id) = i32::try_from(pk) else {
        return Err(ApiError::not_found_model("ApplicationConfiguration").into());
    };
    let exists: Option<i32> =
        sqlx::query_scalar(&format!("SELECT id FROM {TABLE} WHERE id = $1")).bind(id).fetch_optional(state.db()).await?;
    exists.ok_or_else(|| Error::Api(ApiError::not_found_model("ApplicationConfiguration")))
}

async fn row_json(conn: &mut sqlx::PgConnection, sql: &str, id: Option<i32>) -> Result<Option<Map<String, Value>>> {
    let q = sqlx::query_scalar::<_, Value>(sql);
    let q = match id {
        Some(i) => q.bind(i),
        None => q,
    };
    Ok(q.fetch_optional(&mut *conn).await?.and_then(|v| match v {
        Value::Object(m) => Some(m),
        _ => None,
    }))
}

async fn remove_media_file(state: &AppState, name: &str) {
    if name.is_empty() {
        return;
    }
    if let Some(p) = pl_settings::logo::media_path(&state.config().media_root, name)
        && let Err(e) = tokio::fs::remove_file(&p).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(error = %e, "could not delete the old app logo");
    }
}

async fn update_impl(state: AppState, ctx: RequestContext, auth: AuthUser, pk: String, req: Request, partial: bool) -> Result<Response> {
    if !auth.user().has_perm("paperless.change_applicationconfiguration") {
        return Err(pl_authz::AuthError::permission_denied().into());
    }
    let id = get_object(&state, &pk).await?;
    let (data, mutable) = parse_request(req).await?;
    let env = ValidationEnv::from_env();
    let validated = match validate_data(data, mutable, partial, &env).await? {
        Ok(v) => v,
        Err(body) => return Err(ApiError::validation(body).into()),
    };

    let db = state.db();
    let mut tx = db.begin().await?;
    // perform_update: old_instance = ApplicationConfiguration.objects.all().first()
    let old_first = row_json(&mut tx, &format!("SELECT to_jsonb(c) FROM {TABLE} c ORDER BY c.id LIMIT 1"), None).await?.unwrap_or_default();
    let mut instance = row_json(&mut tx, &format!("SELECT to_jsonb(c) FROM {TABLE} c WHERE c.id = $1 FOR UPDATE"), Some(id))
        .await?
        .ok_or_else(|| Error::Api(ApiError::not_found_model("ApplicationConfiguration")))?;
    let old_logo = instance.get("app_logo").and_then(Value::as_str).unwrap_or("").to_string();
    let mut stored_logo: Option<String> = None;
    let logo_given = validated.iter().any(|(n, _)| *n == "app_logo");
    // ApplicationConfigurationSerializer.update: `instance.app_logo.delete()` before the new file
    // is stored (a re-upload with the same name gets the same storage name, like Django).
    if logo_given && !old_logo.is_empty() {
        remove_media_file(&state, &old_logo).await;
    }
    for (name, v) in validated {
        match v {
            Validated::Value(x) => {
                instance.insert(name.into(), x);
            }
            Validated::Logo(clean, bytes) => {
                let stored = pl_settings::logo::store(&state.config().media_root, &clean, &bytes)
                    .await
                    .map_err(|e| ApiError::internal(format!("storing the app logo failed: {e}")))?;
                instance.insert(name.into(), Value::String(stored.clone()));
                stored_logo = Some(stored);
            }
        }
    }
    instance.insert("id".into(), Value::from(SINGLETON_ID));
    // FileField.get_prep_value(FieldFile(None)) → str(...) → "": every save stores "" for "no logo"
    if instance.get("app_logo").is_none_or(Value::is_null) {
        instance.insert("app_logo".into(), Value::String(String::new()));
    }
    let cols: Vec<&str> = crate::config::model_field_names().collect();
    let set = cols.iter().map(|c| format!("{c} = EXCLUDED.{c}")).collect::<Vec<_>>().join(", ");
    let sql = format!("INSERT INTO {TABLE} SELECT * FROM jsonb_populate_record(NULL::{TABLE}, $1) ON CONFLICT (id) DO UPDATE SET {set}");
    let res = sqlx::query(&sql).bind(Value::Object(instance.clone())).execute(&mut *tx).await;
    if let Err(e) = res {
        if let Some(s) = &stored_logo {
            remove_media_file(&state, s).await;
        }
        return Err(e.into());
    }
    if let Err(e) = tx.commit().await {
        if let Some(s) = &stored_logo {
            remove_media_file(&state, s).await;
        }
        return Err(e.into());
    }
    maybe_rebuild_llm_index(&state, &old_first, &instance).await;
    let body = crate::config::serialize_by_id(&state, &ctx, SINGLETON_ID).await?;
    Ok(Drf::ok(body).into_response())
}

/// `PUT /api/config/{pk}/`
pub async fn update(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    UrlPath(pk): UrlPath<String>,
    req: Request,
) -> Response {
    match update_impl(state, ctx, auth, pk, req, false).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `PATCH /api/config/{pk}/`
pub async fn partial_update(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    UrlPath(pk): UrlPath<String>,
    req: Request,
) -> Response {
    match update_impl(state, ctx, auth, pk, req, true).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `DELETE /api/config/{pk}/` – `instance.delete()` (the logo file stays, like Django).
pub async fn destroy(
    State(state): State<AppState>,
    _ctx: RequestContext,
    auth: AuthUser,
    UrlPath(pk): UrlPath<String>,
) -> Result<Response> {
    if !auth.user().has_perm("paperless.delete_applicationconfiguration") {
        return Err(pl_authz::AuthError::permission_denied().into());
    }
    let id = get_object(&state, &pk).await?;
    sqlx::query(&format!("DELETE FROM {TABLE} WHERE id = $1")).bind(id).execute(state.db()).await?;
    Ok(pl_core::json::drf_no_content())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn env() -> ValidationEnv {
        ValidationEnv { max_image_pixels: None, llm_allow_internal: true, remote_ocr_allow_internal: true }
    }

    async fn run(v: Value, partial: bool) -> std::result::Result<Vec<(&'static str, Validated)>, Value> {
        match validate_data(Data::Json(v), true, partial, &env()).await {
            Ok(r) => r,
            Err(_) => Err(json!("500")),
        }
    }

    #[tokio::test]
    async fn json_fields_like_drf() {
        let ok =
            run(json!({"pages": "5.0", "deskew": "yes", "rotate_pages_threshold": true, "output_type": "", "app_title": "  T  "}), true)
                .await
                .unwrap();
        assert_eq!(
            ok,
            vec![
                ("output_type", Validated::Value(json!(""))),
                ("pages", Validated::Value(json!(5))),
                ("deskew", Validated::Value(json!(true))),
                ("rotate_pages_threshold", Validated::Value(json!(1.0))),
                ("app_title", Validated::Value(json!("T"))),
            ]
        );
        let e = run(
            json!({"pages": 0, "image_dpi": 40000, "mode": "x", "barcode_upscale": 0.5, "deskew": "maybe", "user_args": {"a": 1},
                   "barcode_tag_mapping": "{\"a\": 1}", "app_logo": "x.png", "language": 5, "llm_context_size": 1.5}),
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(
            e,
            json!({
                "user_args": ["Value must be valid JSON."],
                "barcode_tag_mapping": ["barcode_tag_mapping values must be strings."],
                "pages": ["Ensure this value is greater than or equal to 1."],
                "mode": ["\"x\" is not a valid choice."],
                "image_dpi": ["Ensure this value is less than or equal to 32767."],
                "deskew": ["Must be a valid boolean."],
                "app_logo": [NOT_A_FILE],
                "barcode_upscale": ["Ensure this value is greater than or equal to 1.0."],
                "llm_context_size": ["A valid integer is required."],
            })
        );
        // language 5 → "5" (CharField accepts int)
        assert!(run(json!({"language": 5}), true).await.is_ok());
        assert_eq!(run(json!({"user_args": "[1]"}), true).await.unwrap_err(), json!({"user_args": ["user_args must be a JSON object."]}));
        // upstream fix d05e96cda (#14734): test_api_update_config_rejects_reserved_user_args
        assert_eq!(
            run(json!({"user_args": "{\"sidecar\": \"/tmp/x.py\"}"}), true).await.unwrap_err(),
            json!({"user_args": ["user_args may not set arguments managed by PaperRust: sidecar."]})
        );
        assert_eq!(
            run(json!({"user_args": r#"{"sidecar": 1, "output_file": 2, "input_file_or_options": 3, "deskew": true}"#}), true)
                .await
                .unwrap_err(),
            json!({"user_args": ["user_args may not set arguments managed by PaperRust: input_file_or_options, output_file, sidecar."]})
        );
        assert!(run(json!({"user_args": "{\"unpaper_args\": \"--pre-rotate 90\"}"}), true).await.is_ok());
        assert_eq!(run(json!({"user_args": ""}), true).await.unwrap(), vec![("user_args", Validated::Value(Value::Null))]);
        assert_eq!(
            run(json!([1]), true).await.unwrap_err(),
            json!({"non_field_errors": ["Invalid data. Expected a dictionary, but got list."]})
        );
    }

    #[tokio::test]
    async fn obfuscated_keys() {
        assert_eq!(run(json!({"llm_api_key": "**********"}), true).await.unwrap(), vec![]);
        assert_eq!(run(json!({"llm_api_key": ""}), true).await.unwrap(), vec![("llm_api_key", Validated::Value(Value::Null))]);
        assert_eq!(run(json!({"llm_api_key": "a*b"}), true).await.unwrap(), vec![("llm_api_key", Validated::Value(json!("a*b")))]);
        assert_eq!(run(json!({"llm_api_key": "  "}), true).await.unwrap_err(), json!({"llm_api_key": ["This field may not be blank."]}));
        assert_eq!(run(json!({"llm_api_key": 5}), true).await.unwrap_err(), json!("500"));
    }

    #[tokio::test]
    async fn form_input_rules() {
        let d = Data::Form(vec![
            ("user_args".into(), FormItem::Text(String::new())),
            ("pages".into(), FormItem::Text(String::new())),
            ("language".into(), FormItem::Text(String::new())),
        ]);
        // user_args "" → None → JSONString("None") → invalid JSON; pages "" → None; language "" → None
        let e = validate_data(d.clone(), true, true, &env()).await.unwrap().unwrap_err();
        assert_eq!(e, json!({"user_args": ["Value must be valid JSON."]}));
        // immutable QueryDict (form / multipart without files): any pre-processing write → 500
        assert!(validate_data(d.clone(), false, true, &env()).await.is_err());
        // PUT: the declared JSONFields are required
        let e = validate_data(Data::Form(vec![]), false, false, &env()).await.unwrap().unwrap_err();
        assert_eq!(e, json!({"user_args": ["This field is required."], "barcode_tag_mapping": ["This field is required."]}));
        // PUT: missing booleans become NULL
        let full = Data::Form(vec![
            ("user_args".into(), FormItem::Text("{}".into())),
            ("barcode_tag_mapping".into(), FormItem::Text("null".into())),
        ]);
        let ok = validate_data(full, false, false, &env()).await.unwrap().unwrap();
        assert_eq!(ok.len(), 2 + FIELDS.iter().filter(|(_, k)| *k == FieldKind::Bool).count());
        let up = Upload { name: "a.txt".into(), data: b"x".to_vec() };
        let e = validate_data(Data::Form(vec![("app_logo".into(), FormItem::File(up))]), true, true, &env()).await.unwrap().unwrap_err();
        assert_eq!(
            e,
            json!({"app_logo": ["File extension \u{201c}txt\u{201d} is not allowed. Allowed extensions are: jpg, png, gif, svg."]})
        );
        let up = Upload { name: "a.png".into(), data: Vec::new() };
        let e = validate_data(Data::Form(vec![("app_logo".into(), FormItem::File(up))]), true, true, &env()).await.unwrap().unwrap_err();
        assert_eq!(e, json!({"app_logo": ["The submitted file is empty."]}));
    }

    #[tokio::test]
    async fn outbound_urls() {
        assert_eq!(validate_outbound_http_url("ftp://x", true).await, Err("Invalid URL scheme or hostname.".into()));
        assert_eq!(validate_outbound_http_url("http://", true).await, Err("Invalid URL scheme or hostname.".into()));
        assert_eq!(validate_outbound_http_url("http://h:99999", true).await, Err("Invalid URL scheme or hostname.".into()));
        assert_eq!(validate_outbound_http_url("HTTP://Host:8080/x", true).await, Ok(()));
        assert_eq!(
            validate_outbound_http_url("http://127.0.0.1:11434", false).await,
            Err("Connection blocked: 127.0.0.1 resolves to a non-public address".into())
        );
        assert_eq!(
            validate_outbound_http_url("http://[::1]/", false).await.unwrap_err(),
            "Connection blocked: ::1 resolves to a non-public address"
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(sanitize_file_name("C:\\x\\my logo.png").as_deref(), Some("my logo.png"));
        assert_eq!(sanitize_file_name("../.."), None);
        assert_eq!(path_suffix("a.tar.GZ"), "GZ");
        assert_eq!(path_suffix(".hidden"), "");
        assert_eq!(strip_decimal_zeros("5.00"), "5");
        assert_eq!(py_float_str(" 1_0.5 "), Some(10.5));
        assert!(py_float_str("nan").is_some_and(f64::is_nan));
    }

    #[test]
    fn embedding_change_detection() {
        let a = json!({"llm_embedding_backend": "ollama", "llm_embedding_chunk_size": null, "llm_context_size": 0});
        let b = json!({"llm_embedding_backend": "ollama", "llm_embedding_chunk_size": 1024, "llm_context_size": null});
        let (Value::Object(a), Value::Object(b)) = (a, b) else { unreachable!() };
        // null/0 fall back to the defaults (1024, 8192) → unchanged
        assert_eq!(embedding_config(&a), embedding_config(&b));
    }
}
