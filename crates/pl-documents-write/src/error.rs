//! Errors of this crate; rendered via `pl_core::ApiError` (DRF/Django format).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

#[derive(Debug)]
pub enum ApiError {
    /// Ready-made error from pl-core/pl-authz.
    Core(pl_core::ApiError),
    /// 400 DRF validation error (ready-made body).
    Validation(Value),
    /// Django `HttpResponseForbidden(text)` – `text/html; charset=utf-8`.
    ForbiddenText(&'static str),
    /// Django `HttpResponseBadRequest(text)` – `text/html; charset=utf-8`.
    BadRequestText(String),
    /// Uncaught exception upstream → 500 with Django's default page.
    Uncaught(String),
    /// Database error (rendered like [`ApiError::Uncaught`] `"database: …"`); the SQLSTATE is
    /// kept so that transactions can be retried on deadlock/serialization failure
    /// ([`ApiError::is_retryable`], FINDINGS B4).
    Database(sqlx::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Database(e)
    }
}

impl ApiError {
    /// Deadlock (`40P01`) or serialization failure (`40001`) from the database.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ApiError::Database(e) if pl_consumer::db::is_retryable(e))
    }
}

impl From<pl_core::ApiError> for ApiError {
    fn from(e: pl_core::ApiError) -> Self {
        ApiError::Core(e)
    }
}

impl From<ApiError> for pl_core::ApiError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Core(c) => c,
            ApiError::Validation(v) => pl_core::ApiError::validation(v),
            ApiError::ForbiddenText(t) => pl_core::ApiError::django_text(StatusCode::FORBIDDEN, t),
            ApiError::BadRequestText(t) => pl_core::ApiError::django_text(StatusCode::BAD_REQUEST, t),
            ApiError::Uncaught(internal) => pl_core::ApiError::internal(internal),
            // tests/chaos C1: DB unreachable → 503 with message (pl-core), otherwise 500 as before
            ApiError::Database(e) => pl_core::ApiError::from(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        pl_core::ApiError::from(self).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
