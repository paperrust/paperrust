//! v2 error format: `{"error":{"code":"not_found","message":"…","fields":{…}}}`
//! (ARCHITECTURE-V2 §3, RFC-9457-like, `application/json`).

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_domain::DomainError;
use serde_json::{Map, Value, json};

/// Error response of API v2.
#[derive(Debug)]
pub struct V2Error(pub DomainError);

impl From<DomainError> for V2Error {
    fn from(e: DomainError) -> Self {
        V2Error(e)
    }
}

pub type V2Result<T> = Result<T, V2Error>;

pub fn status_of(e: &DomainError) -> StatusCode {
    match e {
        DomainError::NotFound => StatusCode::NOT_FOUND,
        DomainError::Unauthenticated => StatusCode::UNAUTHORIZED,
        DomainError::Forbidden(_) => StatusCode::FORBIDDEN,
        DomainError::Invalid { .. } => StatusCode::BAD_REQUEST,
        DomainError::RevMismatch { .. } => StatusCode::PRECONDITION_FAILED,
        DomainError::Conflict(_) => StatusCode::CONFLICT,
        DomainError::CursorExpired => StatusCode::GONE,
        DomainError::NotInstalled(_) => StatusCode::SERVICE_UNAVAILABLE,
        DomainError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
        DomainError::Throttled { .. } => StatusCode::TOO_MANY_REQUESTS,
        DomainError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Body of an error (internals never go outside).
pub fn body_of(e: &DomainError) -> Value {
    let message = match e {
        DomainError::Internal(detail) => {
            tracing::error!(target: "paperrust.api_v2", "internal error: {detail}");
            "Internal server error.".to_string()
        }
        DomainError::NotFound => "Not found.".to_string(),
        DomainError::Unauthenticated => "Authentication credentials were not provided or are invalid.".to_string(),
        other => other.to_string(),
    };
    let mut err = Map::new();
    err.insert("code".into(), json!(e.code()));
    err.insert("message".into(), json!(message));
    match e {
        DomainError::Invalid { fields, .. } if !fields.is_empty() => {
            err.insert("fields".into(), Value::Object(fields.clone()));
        }
        DomainError::RevMismatch { current } => {
            err.insert("current_rev".into(), json!(current));
        }
        _ => {}
    }
    json!({ "error": err })
}

impl IntoResponse for V2Error {
    fn into_response(self) -> Response {
        let status = status_of(&self.0);
        let mut r = (status, axum::Json(body_of(&self.0))).into_response();
        match &self.0 {
            DomainError::Unauthenticated => {
                r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"paperrust\""));
            }
            DomainError::RevMismatch { current } => {
                if let Ok(v) = HeaderValue::from_str(&format!("\"{current}\"")) {
                    r.headers_mut().insert(header::ETAG, v);
                }
            }
            DomainError::Throttled { wait: Some(w) } => {
                r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(*w));
            }
            _ => {}
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let b = body_of(&DomainError::field("name", "This field is required."));
        assert_eq!(b["error"]["code"], "invalid");
        assert_eq!(b["error"]["fields"]["name"][0], "This field is required.");
        let b = body_of(&DomainError::Internal("secret path /x".into()));
        assert_eq!(b["error"]["message"], "Internal server error.");
        assert_eq!(status_of(&DomainError::RevMismatch { current: 3 }), StatusCode::PRECONDITION_FAILED);
        assert_eq!(body_of(&DomainError::RevMismatch { current: 3 })["error"]["current_rev"], 3);
    }
}
