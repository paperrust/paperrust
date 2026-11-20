//! Handler error type: either a DRF/Django error from `pl-core` or an auth error from `pl-authz`.

use axum::response::{IntoResponse, Response};
use pl_authz::AuthError;
use pl_core::ApiError;

/// Error of every handler in this crate.
#[derive(Debug)]
pub enum Error {
    Api(ApiError),
    Auth(AuthError),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        Error::Api(e)
    }
}

impl From<AuthError> for Error {
    fn from(e: AuthError) -> Self {
        Error::Auth(e)
    }
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Error::Api(ApiError::from(e))
    }
}

impl From<sea_query::error::Error> for Error {
    fn from(e: sea_query::error::Error) -> Self {
        Error::Api(ApiError::from(e))
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Error::Api(e) => e.into_response(),
            Error::Auth(e) => e.into_response(),
        }
    }
}

/// `HttpResponseForbidden("Insufficient permissions")` (text/html, no JSON).
pub fn insufficient_permissions() -> Error {
    Error::Api(ApiError::django_text(axum::http::StatusCode::FORBIDDEN, "Insufficient permissions"))
}
