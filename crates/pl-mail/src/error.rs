//! Response types in DRF/Django style for pl-mail.
//!
//! upstream: rest_framework/views.py@3.16.1 (exception_handler), django/views/defaults.py (server_error),
//! django/http/response.py (HttpResponseBadRequest/HttpResponseForbidden).
//! Lokal, bis `pl_core::ApiError` existiert (CORE-REQUEST).

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

/// Django default page for unhandled exceptions (DEBUG=False, no custom `handler500`).
pub const DJANGO_500_PAGE: &str = "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>Server Error (500)</title>\n</head>\n<body>\n  <h1>Server Error (500)</h1><p></p>\n</body>\n</html>\n";

pub const MSG_NO_PERMISSION: &str = "You do not have permission to perform this action.";
pub const MSG_NOT_AUTHENTICATED: &str = "Authentication credentials were not provided.";
pub const MSG_NOT_FOUND: &str = "Not found.";

#[derive(Debug)]
pub enum ApiError {
    /// DRF JSON response (`{"detail": ...}` or field errors).
    Json(StatusCode, Value),
    /// Django `HttpResponse*` with text body (`text/html; charset=utf-8`).
    Html(StatusCode, &'static str),
    /// 401 like DRF `NotAuthenticated` with `PaperlessBasicAuthentication` as first class.
    NotAuthenticated,
    /// Unhandled exception upstream → Django 500 page. The text is only logged.
    Internal(String),
    /// Finished response from pl-core/pl-authz (authentication, Celery, ...).
    Raw(Box<Response>),
}

impl ApiError {
    pub fn detail(status: StatusCode, msg: impl Into<String>) -> Self {
        ApiError::Json(status, serde_json::json!({ "detail": msg.into() }))
    }
    pub fn forbidden() -> Self {
        Self::detail(StatusCode::FORBIDDEN, MSG_NO_PERMISSION)
    }
    pub fn not_found_model(model: &str) -> Self {
        Self::detail(StatusCode::NOT_FOUND, format!("No {model} matches the given query."))
    }
    pub fn not_found() -> Self {
        Self::detail(StatusCode::NOT_FOUND, MSG_NOT_FOUND)
    }
    pub fn bad_request(body: Value) -> Self {
        ApiError::Json(StatusCode::BAD_REQUEST, body)
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        ApiError::Internal(msg.into())
    }
}

impl From<pl_core::ApiError> for ApiError {
    fn from(e: pl_core::ApiError) -> Self {
        ApiError::Raw(Box::new(e.into_response()))
    }
}

impl From<pl_authz::AuthError> for ApiError {
    fn from(e: pl_authz::AuthError) -> Self {
        ApiError::Raw(Box::new(e.into_response()))
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Internal(format!("database error: {e}"))
    }
}

/// JSON like DRF `JSONRenderer` (compact, UTF-8, U+2028/U+2029 escaped).
pub fn json_response(status: StatusCode, value: &Value) -> Response {
    let mut body = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    if body.contains('\u{2028}') || body.contains('\u{2029}') {
        body = body.replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029");
    }
    let mut resp = (status, body).into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    resp.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept"));
    resp
}

pub fn html_response(status: StatusCode, body: &'static str) -> Response {
    let mut resp = (status, body).into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    resp
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Json(status, v) => json_response(status, &v),
            ApiError::Html(status, body) => html_response(status, body),
            ApiError::NotAuthenticated => {
                let mut r = json_response(StatusCode::UNAUTHORIZED, &serde_json::json!({ "detail": MSG_NOT_AUTHENTICATED }));
                r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"api\""));
                r
            }
            ApiError::Raw(r) => *r,
            ApiError::Internal(msg) => {
                tracing::error!(target: "pl_mail", "internal error: {msg}");
                // Django's handler, not DRF: drf_view sets no Allow/Vary
                let mut r = html_response(StatusCode::INTERNAL_SERVER_ERROR, DJANGO_500_PAGE);
                r.extensions_mut().insert(pl_core::DjangoHandlerResponse);
                r
            }
        }
    }
}

pub type ApiResult<T = Response> = Result<T, ApiError>;
