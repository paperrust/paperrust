//! Request/response helpers shared by the handlers of this crate: DRF `request.data`
//! (JSON / form / multipart parsers), DRF field `get_value` semantics for HTML (form) input,
//! `BooleanField` parsing and the error type that unifies `ApiError` and `AuthError`.
//!
//! upstream: DRF 3.18.3 rest_framework/request.py (Request._parse, _load_data_and_files),
//! rest_framework/parsers.py (JSONParser, FormParser, MultiPartParser),
//! rest_framework/fields.py (Field.get_value, BooleanField, CharField),
//! rest_framework/serializers.py (Serializer.to_internal_value "Invalid data").

use axum::body::Body;
use axum::extract::{FromRequest, Multipart, Request};
use axum::http::header::CONTENT_TYPE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use pl_authz::AuthError;
use pl_core::ApiError;
use serde_json::{Map, Value};

/// Handler error: either a pl-core `ApiError` or a pl-authz `AuthError`.
#[derive(Debug)]
pub enum HErr {
    Api(ApiError),
    Auth(AuthError),
}

impl From<ApiError> for HErr {
    fn from(e: ApiError) -> Self {
        HErr::Api(e)
    }
}

impl From<AuthError> for HErr {
    fn from(e: AuthError) -> Self {
        HErr::Auth(e)
    }
}

impl From<sqlx::Error> for HErr {
    fn from(e: sqlx::Error) -> Self {
        HErr::Api(ApiError::from(e))
    }
}

impl From<sea_query::error::Error> for HErr {
    fn from(e: sea_query::error::Error) -> Self {
        HErr::Api(ApiError::from(e))
    }
}

impl IntoResponse for HErr {
    fn into_response(self) -> Response {
        match self {
            HErr::Api(e) => e.into_response(),
            HErr::Auth(e) => e.into_response(),
        }
    }
}

pub type HResult<T> = Result<T, HErr>;

/// Unhandled Python exception → Django 500 page (message only logged).
pub fn server_error(what: impl std::fmt::Display) -> HErr {
    HErr::Api(ApiError::internal(what))
}

/// `HttpResponseForbidden(text)` / `HttpResponseNotFound(text)` / `HttpResponseBadRequest(text)`.
pub fn django_text(status: StatusCode, text: &str) -> HErr {
    HErr::Api(ApiError::django_text(status, text))
}

/// DRF `request.data`.
#[derive(Debug, Clone)]
pub enum Data {
    /// JSON body (any JSON value; usually an object).
    Json(Value),
    /// `QueryDict` from form-urlencoded or multipart (text fields).
    Form(Vec<(String, String)>),
}

impl Data {
    pub fn empty() -> Self {
        Data::Json(Value::Object(Map::new()))
    }

    pub fn is_form(&self) -> bool {
        matches!(self, Data::Form(_))
    }

    /// `key in request.data`.
    pub fn contains(&self, key: &str) -> bool {
        match self {
            Data::Json(Value::Object(m)) => m.contains_key(key),
            Data::Json(_) => false,
            Data::Form(p) => p.iter().any(|(k, _)| k == key),
        }
    }

    /// `request.data[key]` / `request.data.get(key)` (QueryDict: last value).
    pub fn get(&self, key: &str) -> Option<Value> {
        match self {
            Data::Json(Value::Object(m)) => m.get(key).cloned(),
            Data::Json(_) => None,
            Data::Form(p) => p.iter().rev().find(|(k, _)| k == key).map(|(_, v)| Value::String(v.clone())),
        }
    }

    /// `QueryDict.getlist(key)`.
    pub fn get_list(&self, key: &str) -> Vec<String> {
        match self {
            Data::Form(p) => p.iter().filter(|(k, _)| k == key).map(|(_, v)| v.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// `Serializer.to_internal_value`: data must be a mapping.
    pub fn require_mapping(&self) -> Result<(), ApiError> {
        match self {
            Data::Json(Value::Object(_)) | Data::Form(_) => Ok(()),
            Data::Json(v) => Err(ApiError::non_field_error(format!("Invalid data. Expected a dictionary, but got {}.", py_type_name(v)))),
        }
    }
}

/// Python type name of a parsed JSON value (`type(x).__name__`).
pub fn py_type_name(v: &Value) -> &'static str {
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

fn media_type(headers: &HeaderMap) -> Option<String> {
    headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn base_type(ct: &str) -> String {
    ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// `Request._parse()` with parser classes `[JSONParser, FormParser, MultiPartParser]`
/// (DRF default and `ObtainAuthToken`). Body empty / no content type → empty data.
pub async fn parse_data(parts: &Parts, body: axum::body::Bytes) -> Result<Data, ApiError> {
    let ct = media_type(&parts.headers);
    if body.is_empty() {
        return Ok(match ct.as_deref().map(base_type).as_deref() {
            Some("application/x-www-form-urlencoded") | Some("multipart/form-data") => Data::Form(Vec::new()),
            _ => Data::empty(),
        });
    }
    let Some(ct) = ct else {
        return Ok(Data::empty());
    };
    match base_type(&ct).as_str() {
        "application/json" => match serde_json::from_slice::<Value>(&body) {
            Ok(v) => Ok(Data::Json(v)),
            Err(e) => Err(ApiError::json_parse_error(&crate::pyjson::decode_error(&body).unwrap_or_else(|| e.to_string()))),
        },
        "application/x-www-form-urlencoded" => {
            let pairs = pl_core::QueryParams::parse(&String::from_utf8_lossy(&body));
            Ok(Data::Form(pairs.pairs().to_vec()))
        }
        "multipart/form-data" => {
            let mut req = Request::new(Body::from(body));
            *req.headers_mut() = parts.headers.clone();
            let mut mp = Multipart::from_request(req, &())
                .await
                .map_err(|_| ApiError::parse_error(Some("Multipart form parse error - Invalid boundary in multipart: None")))?;
            let mut out = Vec::new();
            loop {
                match mp.next_field().await {
                    Ok(Some(f)) => {
                        let name = f.name().unwrap_or("").to_string();
                        if f.file_name().is_some() {
                            continue; // files are not used by these views
                        }
                        let text = f.text().await.unwrap_or_default();
                        out.push((name, text));
                    }
                    Ok(None) => break,
                    Err(e) => return Err(ApiError::parse_error(Some(&format!("Multipart form parse error - {e}")))),
                }
            }
            Ok(Data::Form(out))
        }
        _ => Err(ApiError::unsupported_media_type(&ct)),
    }
}

/// Outcome of `Field.get_value` + empty-value handling.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldInput {
    /// `empty` – field skipped (or "required" error if required).
    Missing,
    /// A value (JSON or form string).
    Value(Value),
}

/// `Field.get_value(dictionary)` for scalar fields. `default_empty_html` is `False` for
/// `BooleanField`, `empty` otherwise.
pub fn get_value(data: &Data, field: &str, opts: FieldOpts) -> FieldInput {
    let FieldOpts { partial, required, allow_null, allow_blank, bool_field } = opts;
    match data {
        Data::Form(_) => {
            if !data.contains(field) {
                if partial {
                    return FieldInput::Missing;
                }
                return if bool_field { FieldInput::Value(Value::Bool(false)) } else { FieldInput::Missing };
            }
            let ret = data.get(field).unwrap_or(Value::Null);
            if ret == Value::String(String::new()) && allow_null {
                return if allow_blank { FieldInput::Value(ret) } else { FieldInput::Value(Value::Null) };
            } else if ret == Value::String(String::new()) && !required {
                return if allow_blank { FieldInput::Value(ret) } else { FieldInput::Missing };
            }
            FieldInput::Value(ret)
        }
        Data::Json(_) => match data.get(field) {
            Some(v) => FieldInput::Value(v),
            None => FieldInput::Missing,
        },
    }
}

/// Options of a serializer field relevant for `get_value`.
#[derive(Debug, Clone, Copy, Default)]
pub struct FieldOpts {
    pub partial: bool,
    pub required: bool,
    pub allow_null: bool,
    pub allow_blank: bool,
    pub bool_field: bool,
}

/// `ManyRelatedField.get_value`: form → `getlist` (missing + partial → empty).
pub fn get_list_value(data: &Data, field: &str, partial: bool) -> FieldInput {
    match data {
        Data::Form(_) => {
            if !data.contains(field) && partial {
                return FieldInput::Missing;
            }
            FieldInput::Value(Value::Array(data.get_list(field).into_iter().map(Value::String).collect()))
        }
        Data::Json(_) => match data.get(field) {
            Some(v) => FieldInput::Value(v),
            None => FieldInput::Missing,
        },
    }
}

/// `BooleanField.to_internal_value` (`allow_null=False`): `None` = invalid.
pub fn drf_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => {
            let f = n.as_f64()?;
            if f == 1.0 {
                Some(true)
            } else if f == 0.0 {
                Some(false)
            } else {
                None
            }
        }
        // DRF 3.18.3 `BooleanField._lower_if_str`: lowercased before the set lookup
        Value::String(s) => match s.to_lowercase().as_str() {
            "t" | "T" | "y" | "Y" | "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" | "1" => Some(true),
            "f" | "F" | "n" | "N" | "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `CharField` (`trim_whitespace=True`) validation shared by text fields: returns the
/// internal string or the DRF error message.
pub fn drf_char(v: &Value, allow_blank: bool, max_length: Option<usize>, trim: bool) -> Result<String, String> {
    let s = match v {
        Value::Null => return Err("This field may not be null.".into()),
        Value::String(s) => s.clone(),
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => return Err("Not a valid string.".into()),
        Value::Number(n) => n.to_string(),
    };
    let s = if trim { s.trim().to_string() } else { s };
    if s.is_empty() && !allow_blank {
        return Err("This field may not be blank.".into());
    }
    if let Some(max) = max_length
        && s.chars().count() > max
    {
        return Err(format!("Ensure this field has no more than {max} characters."));
    }
    if s.contains('\0') {
        return Err("Null characters are not allowed.".into());
    }
    Ok(s)
}

/// Django `EmailValidator` (simplified to the same accept/reject rules for common input).
pub fn is_valid_email(value: &str) -> bool {
    if value.is_empty() || value.len() > 320 {
        return false;
    }
    let Some((user, domain)) = value.rsplit_once('@') else {
        return false;
    };
    if user.is_empty() || domain.is_empty() {
        return false;
    }
    let user_ok = if user.starts_with('"') && user.ends_with('"') && user.len() >= 2 {
        true
    } else {
        user.split('.').all(|atom| !atom.is_empty() && atom.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~-".contains(c)))
    };
    if !user_ok {
        return false;
    }
    if domain.starts_with('[') && domain.ends_with(']') {
        return domain[1..domain.len() - 1].parse::<std::net::IpAddr>().is_ok();
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return domain == "localhost";
    }
    let tld = labels.last().copied().unwrap_or("");
    let tld_ok = (tld.len() >= 2 && tld.chars().all(|c| c.is_alphabetic() || c == '-') && !tld.starts_with('-') && !tld.ends_with('-'))
        || (tld.starts_with("xn--") && tld.len() > 4);
    tld_ok
        && labels[..labels.len() - 1].iter().all(|l| {
            !l.is_empty()
                && l.chars().count() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
}

/// Django `UnicodeUsernameValidator`: `^[\w.@+-]+\Z`.
pub fn is_valid_username(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_alphanumeric() || c == '_' || ".@+-".contains(c))
}

/// Client address for throttling (`REMOTE_ADDR`), if the server provided `ConnectInfo`.
pub fn remote_addr(parts: &Parts) -> Option<String> {
    parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 1 N4 (checked against DRF 3.18.3 of the reference): `BooleanField` compares lowercased,
    /// `"tRuE"`/`"yES"` are valid, unknown values are not.
    #[test]
    fn drf_bool_is_case_insensitive_like_drf_3_18() {
        use serde_json::json;
        assert_eq!(drf_bool(&json!("tRuE")), Some(true));
        assert_eq!(drf_bool(&json!("oFf")), Some(false));
        assert_eq!(drf_bool(&json!("maybe")), None);
    }
    use serde_json::json;

    #[test]
    fn bool_like_drf() {
        assert_eq!(drf_bool(&json!("yes")), Some(true));
        assert_eq!(drf_bool(&json!(1.0)), Some(true));
        assert_eq!(drf_bool(&json!(0)), Some(false));
        assert_eq!(drf_bool(&json!("null")), None);
        assert_eq!(drf_bool(&json!(2)), None);
    }

    #[test]
    fn form_get_value() {
        let d = Data::Form(vec![("a".into(), "1".into())]);
        let b = FieldOpts { bool_field: true, ..Default::default() };
        assert_eq!(get_value(&d, "is_staff", b), FieldInput::Value(json!(false)));
        assert_eq!(get_value(&d, "is_staff", FieldOpts { partial: true, ..b }), FieldInput::Missing);
        assert_eq!(get_value(&d, "a", FieldOpts::default()), FieldInput::Value(json!("1")));
        let e = Data::Form(vec![("code".into(), "".into())]);
        assert_eq!(get_value(&e, "code", FieldOpts::default()), FieldInput::Missing);
    }

    #[test]
    fn email_and_username() {
        assert!(is_valid_email("a@example.com"));
        assert!(!is_valid_email("a@b"));
        assert!(!is_valid_email("not an email"));
        assert!(is_valid_username("max.power+1@x"));
        assert!(!is_valid_username("max power"));
    }
}
