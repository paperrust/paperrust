//! Errors as DRF/Django produce them for the document list.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

/// Error from filter/ordering.
#[derive(Debug)]
pub enum FilterError {
    /// DRF `ValidationError` → HTTP 400, `body` is exactly DRF's `response.data`
    /// (object `{"field": ["…"]}` or list `["…"]`).
    Validation(Value),
    /// Upstream raises an unhandled exception here (Django responds with HTTP 500).
    /// The text is for the log only, never for the response.
    Internal(String),
    /// Database error.
    Db(sqlx::Error),
}

impl FilterError {
    pub fn status(&self) -> StatusCode {
        match self {
            FilterError::Validation(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// `{"<field>": ["<msg>"]}`
    pub fn field(field: &str, msg: impl Into<String>) -> Self {
        FilterError::Validation(field_errors(field, vec![msg.into()]))
    }

    /// `["<msg>"]` (list without field name)
    pub fn list(msg: impl Into<String>) -> Self {
        FilterError::Validation(Value::Array(vec![Value::String(msg.into())]))
    }

    /// Nests the error under `prefix` (like `handle_validation_prefix` in upstream).
    pub fn prefixed(self, prefix: &str) -> Self {
        match self {
            FilterError::Validation(detail) => {
                let mut m = Map::new();
                m.insert(prefix.to_string(), detail);
                FilterError::Validation(Value::Object(m))
            }
            other => other,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        FilterError::Internal(msg.into())
    }
}

pub(crate) fn field_errors(field: &str, msgs: Vec<String>) -> Value {
    let mut m = Map::new();
    m.insert(field.to_string(), Value::Array(msgs.into_iter().map(Value::String).collect()));
    Value::Object(m)
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterError::Validation(v) => write!(f, "validation error: {v}"),
            FilterError::Internal(m) => write!(f, "internal error: {m}"),
            FilterError::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for FilterError {}

impl From<sqlx::Error> for FilterError {
    fn from(e: sqlx::Error) -> Self {
        FilterError::Db(e)
    }
}

impl From<FilterError> for pl_core::ApiError {
    fn from(e: FilterError) -> Self {
        match e {
            FilterError::Validation(body) => pl_core::ApiError::validation(body),
            FilterError::Internal(msg) => pl_core::ApiError::internal(msg),
            FilterError::Db(e) => pl_core::ApiError::from(e),
        }
    }
}

impl IntoResponse for FilterError {
    fn into_response(self) -> Response {
        match self {
            FilterError::Validation(body) => {
                (StatusCode::BAD_REQUEST, axum::Json(body)).into_response()
            }
            // Like Django with DEBUG=False (handler500).
            // Django's handler, not DRF: no Allow/Vary (pl_core::DjangoHandlerResponse)
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Extension(pl_core::DjangoHandlerResponse),
                [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                "\n<!doctype html>\n<html lang=\"en\">\n<head>\n  <title>Server Error (500)</title>\n</head>\n<body>\n  <h1>Server Error (500)</h1><p></p>\n</body>\n</html>\n",
            )
                .into_response(),
        }
    }
}
