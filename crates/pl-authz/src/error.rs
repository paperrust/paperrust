//! DRF exceptions raised by authentication and permission checks.
//!
//! upstream: rest_framework/exceptions.py@3.16 (NotAuthenticated, AuthenticationFailed,
//!           PermissionDenied, NotFound, MethodNotAllowed),
//!           rest_framework/views.py@3.16 (APIView.handle_exception: 401 + WWW-Authenticate if the
//!           first authenticator has an `authenticate_header`, otherwise 403; permission_denied).

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

/// `NotAuthenticated.default_detail`.
pub const NOT_AUTHENTICATED: &str = "Authentication credentials were not provided.";
/// `PermissionDenied.default_detail`.
pub const PERMISSION_DENIED: &str = "You do not have permission to perform this action.";
/// `NotFound.default_detail` (also `Http404()` without message).
pub const NOT_FOUND: &str = "Not found.";
/// `OwnedObjectSerializer.update` / several views: `_("Insufficient permissions.")`.
pub const INSUFFICIENT_PERMISSIONS: &str = "Insufficient permissions.";

/// An auth/permission failure, rendered like DRF (`{"detail": "..."}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// `NotAuthenticated` – status 401 with `WWW-Authenticate`, or 403 when the first
    /// authentication class has no challenge (remote-user API mode).
    NotAuthenticated { www_authenticate: Option<String> },
    /// `AuthenticationFailed(detail)` – same status rule as `NotAuthenticated`.
    AuthenticationFailed { detail: String, www_authenticate: Option<String> },
    /// `PermissionDenied(detail)` – 403 (also used for "CSRF Failed: ...").
    PermissionDenied { detail: String },
    /// `NotFound(detail)` / `Http404` – 404.
    NotFound { detail: String },
    /// `MethodNotAllowed(method)` – 405 `Method "X" not allowed.`
    MethodNotAllowed { method: String },
    /// Database or internal failure – 500 (Django would answer with its 500 page).
    Internal { message: String },
    /// PaperRust (tests/chaos C1): the database is not reachable – 503 with a clear message.
    Unavailable { message: String },
}

impl AuthError {
    pub fn permission_denied() -> Self {
        Self::PermissionDenied { detail: PERMISSION_DENIED.into() }
    }
    pub fn insufficient_permissions() -> Self {
        Self::PermissionDenied { detail: INSUFFICIENT_PERMISSIONS.into() }
    }
    pub fn not_found() -> Self {
        Self::NotFound { detail: NOT_FOUND.into() }
    }
    /// `get_object_or_404`: `No <ObjectName> matches the given query.`
    pub fn no_match(object_name: &str) -> Self {
        Self::NotFound { detail: format!("No {object_name} matches the given query.") }
    }
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal { message: e.to_string() }
    }

    /// HTTP status like DRF.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotAuthenticated { www_authenticate } | Self::AuthenticationFailed { www_authenticate, .. } => {
                if www_authenticate.is_some() { StatusCode::UNAUTHORIZED } else { StatusCode::FORBIDDEN }
            }
            Self::PermissionDenied { .. } => StatusCode::FORBIDDEN,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed { .. } => StatusCode::METHOD_NOT_ALLOWED,
            Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// The `detail` text.
    pub fn detail(&self) -> String {
        match self {
            Self::NotAuthenticated { .. } => NOT_AUTHENTICATED.into(),
            Self::AuthenticationFailed { detail, .. } | Self::PermissionDenied { detail } | Self::NotFound { detail } => detail.clone(),
            Self::MethodNotAllowed { method } => format!("Method \"{method}\" not allowed."),
            Self::Internal { .. } => "A server error occurred.".into(),
            Self::Unavailable { .. } => pl_core::dbconn::DB_UNAVAILABLE_DETAIL.into(),
        }
    }

    /// Value of the `WWW-Authenticate` header, if any.
    pub fn www_authenticate(&self) -> Option<&str> {
        match self {
            Self::NotAuthenticated { www_authenticate } | Self::AuthenticationFailed { www_authenticate, .. } => {
                www_authenticate.as_deref()
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status().as_u16(), self.detail())
    }
}

impl std::error::Error for AuthError {}

impl IntoResponse for AuthError {
    /// Minimal DRF-like rendering. Handlers should prefer converting into `pl_core::ApiError`
    /// once available (CORE-REQUEST), which adds DRF's `Allow`/`Vary` headers.
    fn into_response(self) -> Response {
        if let Self::Internal { message } = &self {
            tracing::error!(target: "paperless.auth", "internal auth error: {message}");
        }
        if let Self::Unavailable { message } = &self {
            tracing::warn!(target: "paperless.auth", "database not reachable: {message}");
        }
        let body = serde_json::json!({ "detail": self.detail() }).to_string();
        let mut resp = (self.status(), body).into_response();
        resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if matches!(self, Self::Unavailable { .. }) {
            resp.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static(pl_core::dbconn::DB_UNAVAILABLE_RETRY_AFTER));
        }
        if let Some(w) = self.www_authenticate()
            && let Ok(v) = HeaderValue::from_str(w)
        {
            resp.headers_mut().insert(header::WWW_AUTHENTICATE, v);
        }
        resp
    }
}

impl From<sqlx::Error> for AuthError {
    fn from(e: sqlx::Error) -> Self {
        pl_core::dbconn::note_error(&e);
        if pl_core::dbconn::is_unavailable(&e) {
            return Self::Unavailable { message: e.to_string() };
        }
        Self::internal(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses() {
        let e = AuthError::NotAuthenticated { www_authenticate: Some("Token".into()) };
        assert_eq!(e.status(), StatusCode::UNAUTHORIZED);
        let e = AuthError::NotAuthenticated { www_authenticate: None };
        assert_eq!(e.status(), StatusCode::FORBIDDEN);
        assert_eq!(e.detail(), "Authentication credentials were not provided.");
        let e = AuthError::MethodNotAllowed { method: "TRACE".into() };
        assert_eq!(e.detail(), "Method \"TRACE\" not allowed.");
    }
}
