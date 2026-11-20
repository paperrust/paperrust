//! Errors in DRF format.
//!
//! upstream: rest_framework/exceptions.py@3.18.3, rest_framework/views.py@3.18.3
//! (`exception_handler`, `APIView.handle_exception`), django/views/defaults.py@5.2.
//!
//! * DRF exceptions → `{"detail": "..."}` with status; validation errors → field map
//!   `{"field": ["msg"]}` / `{"non_field_errors": ["msg"]}` or list `["msg"]`.
//! * Django's `Http404` from `get_object_or_404` → `{"detail": "No <Model> matches the given query."}`
//!   (DRF ≥ 3.15 passes the text through); DRF's `generics.get_object_or_404` for an invalid
//!   key (`ValueError`/`TypeError`) → `{"detail": "Not found."}`.
//! * `NotAuthenticated`/`AuthenticationFailed` → 401 with `WWW-Authenticate` if the first
//!   authenticator supplies a header, otherwise 403 (DRF `handle_exception`).
//! * Unexpected errors → Django 500 page (HTML, DEBUG=False), cause only in the log.

use std::fmt;

use axum::http::header::{CONTENT_TYPE, RETRY_AFTER, WWW_AUTHENTICATE};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

use crate::json;

/// Result type for handlers.
pub type ApiResult<T> = Result<T, ApiError>;

/// Django's fallback error page (`django.views.defaults.ERROR_PAGE_TEMPLATE`).
fn django_error_page(title: &str) -> String {
    format!(
        "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>{title}</title>\n</head>\n<body>\n  <h1>{title}</h1><p></p>\n</body>\n</html>\n"
    )
}

#[derive(Debug, Clone)]
enum Body {
    /// JSON body (DRF).
    Json(Value),
    /// Raw body with content type (Django `HttpResponse*`).
    Raw { content_type: &'static str, body: String },
    /// No body.
    Empty,
}

/// Error that is rendered as an HTTP response like DRF/Django.
///
/// Constructors cover the DRF exceptions. Texts are the English originals
/// (paperless only translates API errors when a language is active – see contract,
/// "Known differences").
#[derive(Debug, Clone)]
pub struct ApiError(Box<Inner>);

#[derive(Debug, Clone)]
struct Inner {
    status: StatusCode,
    body: Body,
    headers: Vec<(HeaderName, HeaderValue)>,
    /// For logs only; never in the response.
    internal: Option<String>,
    /// Response carries [`crate::markers::DelegateToDjango`] (pl-server passes it on to Django).
    delegate: bool,
    /// Response comes (upstream) from Django's exception handler (`response_for_exception`), not
    /// from DRF: [`crate::markers::DjangoHandlerResponse`], `drf_view` sets no `Allow`/`Vary`.
    django_handler: bool,
}


impl ApiError {
    fn detail(status: StatusCode, detail: impl Into<String>) -> Self {
        let mut m = Map::new();
        m.insert("detail".into(), Value::String(detail.into()));
        Self(Box::new(Inner { status, body: Body::Json(Value::Object(m)), headers: Vec::new(), internal: None, delegate: false, django_handler: false }))
    }

    /// Any DRF `APIException` case: `{"detail": detail}` with status.
    pub fn with_detail(status: StatusCode, detail: impl Into<String>) -> Self {
        Self::detail(status, detail)
    }

    /// Any JSON body with status (e.g. `{"error": "..."}` from paperless views).
    pub fn json(status: StatusCode, body: Value) -> Self {
        Self(Box::new(Inner { status, body: Body::Json(body), headers: Vec::new(), internal: None, delegate: false, django_handler: false }))
    }

    /// Django `HttpResponse(text, status=...)` (Content-Type `text/html; charset=utf-8`),
    /// e.g. `HttpResponseForbidden("Insufficient permissions")`,
    /// `HttpResponseBadRequest("...")`, `HttpResponseServerError("...")`.
    pub fn django_text(status: StatusCode, text: impl Into<String>) -> Self {
        Self(Box::new(Inner {
            status,
            body: Body::Raw { content_type: "text/html; charset=utf-8", body: text.into() },
            headers: Vec::new(),
            internal: None,
            delegate: false,
            django_handler: false,
        }))
    }

    // ---- 400 -------------------------------------------------------------------------------

    /// DRF `ValidationError` with any body (dict or list), status 400.
    pub fn validation(body: Value) -> Self {
        Self::json(StatusCode::BAD_REQUEST, body)
    }

    /// `{"<field>": ["<msg>"]}`, Status 400.
    pub fn field_error(field: &str, msg: impl Into<String>) -> Self {
        let mut v = ValidationErrors::new();
        v.add(field, msg);
        v.into()
    }

    /// `{"non_field_errors": ["<msg>"]}`, status 400 (serializer `validate()` error).
    pub fn non_field_error(msg: impl Into<String>) -> Self {
        Self::field_error(NON_FIELD_ERRORS, msg)
    }

    /// `serializers.ValidationError("msg")` outside a field → body `["msg"]`.
    pub fn validation_list(msgs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::validation(Value::Array(msgs.into_iter().map(|m| Value::String(m.into())).collect()))
    }

    /// DRF `ParseError` – default text `"Malformed request."`.
    pub fn parse_error(detail: Option<&str>) -> Self {
        Self::detail(StatusCode::BAD_REQUEST, detail.unwrap_or("Malformed request."))
    }

    /// `JSONParser` error: `"JSON parse error - <python-message>"`. The caller must supply the
    /// Python message (`json.JSONDecodeError`) if parity is needed.
    pub fn json_parse_error(python_message: &str) -> Self {
        Self::detail(StatusCode::BAD_REQUEST, format!("JSON parse error - {python_message}"))
    }

    /// `JSONParser` error for an invalid body with CPython's message
    /// ([`crate::pyjson::decode_error`]); `fallback` (e.g. the serde error) only if Python
    /// would have accepted the body (serde is stricter, e.g. for `NaN` or numbers > f64).
    pub fn json_body_parse_error(body: &[u8], fallback: impl std::fmt::Display) -> Self {
        Self::json_parse_error(&crate::pyjson::decode_error(body).unwrap_or_else(|| fallback.to_string()))
    }

    /// Django's 400 page (e.g. `DisallowedHost`, `SuspiciousOperation`).
    pub fn django_bad_request() -> Self {
        Self(Box::new(Inner {
            status: StatusCode::BAD_REQUEST,
            body: Body::Raw { content_type: "text/html; charset=utf-8", body: django_error_page("Bad Request (400)") },
            headers: Vec::new(),
            internal: None,
            delegate: false,
            django_handler: true,
        }))
    }

    // ---- 401/403 ---------------------------------------------------------------------------

    /// DRF `NotAuthenticated`: `"Authentication credentials were not provided."`.
    ///
    /// `www_authenticate` = result of `authenticators[0].authenticate_header(request)`
    /// (paperless: `"Basic realm=\"api\""` for `Authorization: Basic …`, otherwise `"Token"`;
    /// with `PAPERLESS_ENABLE_HTTP_REMOTE_USER_API` the first authenticator is RemoteUser →
    /// `None`). With header → 401, without → 403. Supplied by `pl-authz`.
    pub fn not_authenticated(www_authenticate: Option<&str>) -> Self {
        Self::detail(StatusCode::UNAUTHORIZED, "Authentication credentials were not provided.")
            .auth_status(www_authenticate)
    }

    /// DRF `AuthenticationFailed` (default `"Incorrect authentication credentials."`,
    /// paperless e.g. `"Invalid token."`, `"MFA required"`, `"Invalid username/password."`).
    pub fn authentication_failed(detail: Option<&str>, www_authenticate: Option<&str>) -> Self {
        Self::detail(
            StatusCode::UNAUTHORIZED,
            detail.unwrap_or("Incorrect authentication credentials."),
        )
        .auth_status(www_authenticate)
    }

    fn auth_status(mut self, www_authenticate: Option<&str>) -> Self {
        match www_authenticate.and_then(|h| HeaderValue::from_str(h).ok()) {
            Some(v) => self.0.headers.push((WWW_AUTHENTICATE, v)),
            None => self.0.status = StatusCode::FORBIDDEN,
        }
        self
    }

    /// DRF `PermissionDenied`: `"You do not have permission to perform this action."`.
    pub fn permission_denied() -> Self {
        Self::detail(StatusCode::FORBIDDEN, "You do not have permission to perform this action.")
    }

    /// DRF `PermissionDenied(detail)` or permission class with `message`.
    pub fn permission_denied_msg(detail: impl Into<String>) -> Self {
        Self::detail(StatusCode::FORBIDDEN, detail)
    }

    // ---- 404 -------------------------------------------------------------------------------

    /// DRF `NotFound` without text or `Http404()` without argument: `"Not found."`.
    pub fn not_found() -> Self {
        Self::detail(StatusCode::NOT_FOUND, "Not found.")
    }

    /// Django `get_object_or_404(Model…)`: `"No <verbose object_name> matches the given query."`.
    /// `object_name` is `Model._meta.object_name`, e.g. `"Document"`, `"Tag"`, `"SavedView"`.
    pub fn not_found_model(object_name: &str) -> Self {
        Self::detail(StatusCode::NOT_FOUND, format!("No {object_name} matches the given query."))
    }

    /// `StandardPagination`: invalid page → `"Invalid page."`.
    pub fn invalid_page() -> Self {
        Self::detail(StatusCode::NOT_FOUND, "Invalid page.")
    }

    // ---- 405/406/415/429 -------------------------------------------------------------------

    /// DRF `MethodNotAllowed`: `"Method \"<METHOD>\" not allowed."`.
    pub fn method_not_allowed(method: &str) -> Self {
        Self::detail(StatusCode::METHOD_NOT_ALLOWED, format!("Method \"{method}\" not allowed."))
    }

    /// DRF `NotAcceptable` (Content-Negotiation): `"Could not satisfy the request Accept header."`.
    pub fn not_acceptable() -> Self {
        Self::detail(StatusCode::NOT_ACCEPTABLE, "Could not satisfy the request Accept header.")
    }

    /// `AcceptHeaderVersioning`: `"Invalid version in \"Accept\" header."` (406).
    pub fn invalid_version() -> Self {
        Self::detail(StatusCode::NOT_ACCEPTABLE, "Invalid version in \"Accept\" header.")
    }

    /// DRF `UnsupportedMediaType`: `"Unsupported media type \"<ct>\" in request."` (415).
    pub fn unsupported_media_type(media_type: &str) -> Self {
        Self::detail(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("Unsupported media type \"{media_type}\" in request."),
        )
    }

    /// DRF `Throttled(wait)`: `"Request was throttled. Expected available in N second(s)."`
    /// plus `Retry-After`.
    pub fn throttled(wait_seconds: Option<f64>) -> Self {
        match wait_seconds {
            None => Self::detail(StatusCode::TOO_MANY_REQUESTS, "Request was throttled."),
            Some(w) => {
                let w = w.ceil().max(0.0) as u64;
                let unit = if w == 1 { "second" } else { "seconds" };
                let mut e = Self::detail(
                    StatusCode::TOO_MANY_REQUESTS,
                    format!("Request was throttled. Expected available in {w} {unit}."),
                );
                if let Ok(v) = HeaderValue::from_str(&w.to_string()) {
                    e.0.headers.push((RETRY_AFTER, v));
                }
                e
            }
        }
    }

    // ---- 500 -------------------------------------------------------------------------------

    /// Unexpected error: logged with `tracing::error!`, the response is Django's
    /// generic page `Server Error (500)` (no internals).
    pub fn internal(err: impl fmt::Display) -> Self {
        Self(Box::new(Inner {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: Body::Raw { content_type: "text/html; charset=utf-8", body: django_error_page("Server Error (500)") },
            headers: Vec::new(),
            internal: Some(err.to_string()),
            delegate: false,
            django_handler: true,
        }))
    }

    /// PaperRust (tests/chaos C1): database unreachable → 503 with an understandable message and
    /// `Retry-After` (upstream: Django's 500 page after `OperationalError`).
    pub fn db_unavailable() -> Self {
        Self::detail(StatusCode::SERVICE_UNAVAILABLE, crate::dbconn::DB_UNAVAILABLE_DETAIL).with_header(
            axum::http::header::RETRY_AFTER,
            HeaderValue::from_static(crate::dbconn::DB_UNAVAILABLE_RETRY_AFTER),
        )
    }

    // ---- Access / customization ---------------------------------------------------------------

    /// HTTP-Status.
    pub fn status(&self) -> StatusCode {
        self.0.status
    }

    /// JSON body, if it is a DRF error.
    pub fn body_json(&self) -> Option<&Value> {
        match &self.0.body {
            Body::Json(v) => Some(v),
            _ => None,
        }
    }

    /// Internal cause (for logs/tests only).
    pub fn internal_message(&self) -> Option<&str> {
        self.0.internal.as_deref()
    }

    /// Set an additional response header.
    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.0.headers.push((name, value));
        self
    }

    /// Override the status (rarely needed).
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.0.status = status;
        self
    }

    /// Delegation to Django (see [`crate::markers`]): 501 placeholder with
    /// [`crate::markers::DelegateToDjango`]; `pl-server` replaces it with Django's response.
    pub fn delegate_to_django(reason: &str) -> Self {
        tracing::debug!(reason, "request must be handled by Django");
        let mut e = Self::django_text(crate::markers::DELEGATE_STATUS, "Not implemented in PaperRust");
        e.0.delegate = true;
        e
    }

    /// Is this a delegation to Django?
    pub fn is_delegation(&self) -> bool {
        self.0.delegate
    }

    /// Response without a body (e.g. 204/304-like special cases).
    pub fn empty(status: StatusCode) -> Self {
        Self(Box::new(Inner { status, body: Body::Empty, headers: Vec::new(), internal: None, delegate: false, django_handler: false }))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.0.body, &self.0.internal) {
            (_, Some(i)) => write!(f, "{}: {i}", self.0.status),
            (Body::Json(v), None) => write!(f, "{}: {v}", self.0.status),
            (Body::Raw { body, .. }, None) => write!(f, "{}: {body}", self.0.status),
            (Body::Empty, None) => write!(f, "{}", self.0.status),
        }
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Some(i) = &self.0.internal {
            tracing::error!(error = %i, "internal error");
        }
        let inner = *self.0;
        let mut resp = match inner.body {
            Body::Json(v) => json::drf_response(inner.status, &v),
            Body::Raw { content_type, body } => {
                let mut r = (inner.status, body).into_response();
                r.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
                r
            }
            Body::Empty => inner.status.into_response(),
        };
        for (k, v) in inner.headers {
            resp.headers_mut().append(k, v);
        }
        if inner.delegate {
            resp.extensions_mut().insert(crate::markers::DelegateToDjango);
        }
        if inner.django_handler {
            resp.extensions_mut().insert(crate::markers::DjangoHandlerResponse);
        }
        resp
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        crate::dbconn::note_error(&e);
        if crate::dbconn::is_unavailable(&e) {
            tracing::warn!(error = %e, "database not reachable");
            return ApiError::db_unavailable();
        }
        ApiError::internal(format!("database error: {e}"))
    }
}

impl From<sea_query::error::Error> for ApiError {
    fn from(e: sea_query::error::Error) -> Self {
        ApiError::internal(format!("query build error: {e}"))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::internal(format!("json error: {e}"))
    }
}

/// DRF `api_settings.NON_FIELD_ERRORS_KEY`.
pub const NON_FIELD_ERRORS: &str = "non_field_errors";

/// Collector for serializer/filter validation errors in field order.
///
/// ```
/// use pl_core::ValidationErrors;
/// let mut e = ValidationErrors::new();
/// e.add("name", "This field is required.");
/// e.add("name", "Second message.");
/// assert_eq!(e.to_value().to_string(), r#"{"name":["This field is required.","Second message."]}"#);
/// ```
#[derive(Debug, Clone, Default)]
pub struct ValidationErrors {
    map: Map<String, Value>,
}

impl ValidationErrors {
    /// Empty collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a message to a field (the list is created).
    pub fn add(&mut self, field: &str, msg: impl Into<String>) {
        let entry = self.map.entry(field.to_string()).or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(a) = entry {
            a.push(Value::String(msg.into()));
        } else {
            *entry = Value::Array(vec![Value::String(msg.into())]);
        }
    }

    /// Message under `non_field_errors`.
    pub fn add_non_field(&mut self, msg: impl Into<String>) {
        self.add(NON_FIELD_ERRORS, msg);
    }

    /// Set a nested error (e.g. of a child serializer).
    pub fn set(&mut self, field: &str, value: Value) {
        self.map.insert(field.to_string(), value);
    }

    /// No errors collected?
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// As a JSON object.
    pub fn to_value(&self) -> Value {
        Value::Object(self.map.clone())
    }

    /// `Ok(())` if empty, otherwise a 400 error.
    pub fn into_result(self) -> Result<(), ApiError> {
        if self.is_empty() { Ok(()) } else { Err(self.into()) }
    }
}

impl From<ValidationErrors> for ApiError {
    fn from(v: ValidationErrors) -> Self {
        ApiError::validation(Value::Object(v.map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(e: ApiError) -> (StatusCode, String, Option<String>) {
        let status = e.status();
        let resp = e.into_response();
        let ct = resp.headers().get(CONTENT_TYPE).map(|v| v.to_str().unwrap().to_string());
        let www = resp.headers().get(WWW_AUTHENTICATE).map(|v| v.to_str().unwrap().to_string());
        let _ = ct;
        let b = futures_body(resp);
        (status, b, www)
    }

    fn futures_body(resp: Response) -> String {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        })
    }

    #[test]
    fn drf_texts() {
        assert_eq!(
            body(ApiError::not_authenticated(Some("Token"))),
            (
                StatusCode::UNAUTHORIZED,
                r#"{"detail":"Authentication credentials were not provided."}"#.into(),
                Some("Token".into())
            )
        );
        let (s, _, w) = body(ApiError::not_authenticated(None));
        assert_eq!((s, w), (StatusCode::FORBIDDEN, None));
        assert_eq!(
            body(ApiError::permission_denied()).1,
            r#"{"detail":"You do not have permission to perform this action."}"#
        );
        assert_eq!(body(ApiError::not_found()).1, r#"{"detail":"Not found."}"#);
        assert_eq!(
            body(ApiError::not_found_model("Document")).1,
            r#"{"detail":"No Document matches the given query."}"#
        );
        assert_eq!(body(ApiError::method_not_allowed("DELETE")).1, r#"{"detail":"Method \"DELETE\" not allowed."}"#);
        assert_eq!(body(ApiError::invalid_version()).1, r#"{"detail":"Invalid version in \"Accept\" header."}"#);
        assert_eq!(
            body(ApiError::throttled(Some(0.2))).1,
            r#"{"detail":"Request was throttled. Expected available in 1 second."}"#
        );
        assert_eq!(
            body(ApiError::throttled(Some(30.0))).1,
            r#"{"detail":"Request was throttled. Expected available in 30 seconds."}"#
        );
        assert_eq!(
            body(ApiError::unsupported_media_type("text/plain")).1,
            r#"{"detail":"Unsupported media type \"text/plain\" in request."}"#
        );
    }

    #[test]
    fn validation_shapes() {
        let mut v = ValidationErrors::new();
        v.add("name", "This field is required.");
        v.add_non_field("bad");
        assert_eq!(
            body(v.into()).1,
            r#"{"name":["This field is required."],"non_field_errors":["bad"]}"#
        );
        assert_eq!(body(ApiError::validation_list(["x"])).1, r#"["x"]"#);
    }

    #[test]
    fn internal_hides_details() {
        let e = ApiError::internal("secret connection string");
        let (s, b, _) = body(e);
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!b.contains("secret"));
        assert!(b.contains("<h1>Server Error (500)</h1>"));
    }
}
