//! Error shape like API v2 (`{"error":{"code","message","fields"?}}`, pl-api-v2 `error.rs`), without
//! dependency on pl-api-v2/pl-domain. Internals (DB texts, paths) never go outside.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

#[derive(Debug)]
pub enum AErr {
    Unauthenticated,
    Forbidden(String),
    NotFound,
    /// 400 with field errors (`fields`, for workflows in the DRF format of `/api/workflows/`).
    Invalid {
        message: String,
        fields: Map<String, Value>,
    },
    Conflict(String),
    /// 503: function needs something that is off (AI).
    Unavailable(String),
    NotImplemented(String),
    Throttled(Option<u64>),
    Internal(String),
}

pub type AResult<T> = Result<T, AErr>;

impl AErr {
    pub fn invalid(message: impl Into<String>) -> Self {
        AErr::Invalid { message: message.into(), fields: Map::new() }
    }
    pub fn field(name: &str, message: &str) -> Self {
        let mut fields = Map::new();
        fields.insert(name.into(), json!([message]));
        AErr::Invalid { message: "Invalid input.".into(), fields }
    }
    /// DRF error body (object) as field errors.
    pub fn drf(detail: Value) -> Self {
        match detail {
            Value::Object(fields) => AErr::Invalid { message: "Invalid input.".into(), fields },
            other => {
                let mut fields = Map::new();
                fields.insert("non_field_errors".into(), other);
                AErr::Invalid { message: "Invalid input.".into(), fields }
            }
        }
    }
    pub fn forbidden() -> Self {
        AErr::Forbidden("You do not have permission to perform this action.".into())
    }
    pub fn status(&self) -> StatusCode {
        match self {
            AErr::Unauthenticated => StatusCode::UNAUTHORIZED,
            AErr::Forbidden(_) => StatusCode::FORBIDDEN,
            AErr::NotFound => StatusCode::NOT_FOUND,
            AErr::Invalid { .. } => StatusCode::BAD_REQUEST,
            AErr::Conflict(_) => StatusCode::CONFLICT,
            AErr::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AErr::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            AErr::Throttled(_) => StatusCode::TOO_MANY_REQUESTS,
            AErr::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            AErr::Unauthenticated => "unauthenticated",
            AErr::Forbidden(_) => "forbidden",
            AErr::NotFound => "not_found",
            AErr::Invalid { .. } => "invalid",
            AErr::Conflict(_) => "conflict",
            AErr::Unavailable(_) => "not_installed",
            AErr::NotImplemented(_) => "not_implemented",
            AErr::Throttled(_) => "throttled",
            AErr::Internal(_) => "internal",
        }
    }
    pub fn body(&self) -> Value {
        let message = match self {
            AErr::Unauthenticated => "Authentication credentials were not provided or are invalid.".to_string(),
            AErr::NotFound => "Not found.".to_string(),
            AErr::Internal(detail) => {
                tracing::error!(target: "paperrust.automation", "internal error: {detail}");
                "Internal server error.".to_string()
            }
            AErr::Throttled(_) => "Request was throttled.".to_string(),
            AErr::Forbidden(m) | AErr::Conflict(m) | AErr::Unavailable(m) | AErr::NotImplemented(m) => m.clone(),
            AErr::Invalid { message, .. } => message.clone(),
        };
        let mut err = Map::new();
        err.insert("code".into(), json!(self.code()));
        err.insert("message".into(), json!(message));
        if let AErr::Invalid { fields, .. } = self
            && !fields.is_empty()
        {
            err.insert("fields".into(), Value::Object(fields.clone()));
        }
        json!({ "error": err })
    }
}

impl From<sqlx::Error> for AErr {
    fn from(e: sqlx::Error) -> Self {
        AErr::Internal(format!("database: {e}"))
    }
}

impl From<pl_workflows::drf::VErr> for AErr {
    fn from(e: pl_workflows::drf::VErr) -> Self {
        match e {
            pl_workflows::drf::VErr::Detail(d) => AErr::drf(d),
            pl_workflows::drf::VErr::Server(s) => AErr::Internal(s),
        }
    }
}

impl From<pl_authz::AuthError> for AErr {
    fn from(e: pl_authz::AuthError) -> Self {
        match e.status() {
            StatusCode::UNAUTHORIZED => AErr::Unauthenticated,
            StatusCode::FORBIDDEN => AErr::Forbidden(e.detail()),
            _ => AErr::Internal(e.detail()),
        }
    }
}

/// pl-mail error (DRF shape) → v2 shape; status stays.
pub fn from_mail(e: pl_mail::error::ApiError) -> AErr {
    if let pl_mail::error::ApiError::Json(s, v) = &e
        && *s == StatusCode::BAD_REQUEST
    {
        return AErr::drf(v.clone());
    }
    let r = e.into_response();
    match r.status() {
        StatusCode::BAD_REQUEST => AErr::invalid("Invalid mail rule."),
        StatusCode::NOT_FOUND => AErr::NotFound,
        StatusCode::FORBIDDEN => AErr::forbidden(),
        StatusCode::UNAUTHORIZED => AErr::Unauthenticated,
        s => AErr::Internal(format!("mail rule: status {s}")),
    }
}

impl IntoResponse for AErr {
    fn into_response(self) -> Response {
        let mut r = (self.status(), axum::Json(self.body())).into_response();
        match &self {
            AErr::Unauthenticated => {
                r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"paperrust\""));
            }
            AErr::Throttled(Some(w)) => {
                r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(*w));
            }
            _ => {}
        }
        r
    }
}

/// JSON response (`Cache-Control: private, no-cache` like v2).
pub fn json<T: serde::Serialize>(status: StatusCode, v: &T) -> Response {
    let mut r = (status, axum::Json(v)).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let b = AErr::field("name", "This field is required.").body();
        assert_eq!(b["error"]["code"], "invalid");
        assert_eq!(b["error"]["fields"]["name"][0], "This field is required.");
        let b = AErr::Internal("secret /path".into()).body();
        assert_eq!(b["error"]["message"], "Internal server error.");
        assert_eq!(AErr::NotFound.status(), StatusCode::NOT_FOUND);
        let b = AErr::drf(json!({"triggers": {"0": {"type": ["x"]}}})).body();
        assert_eq!(b["error"]["fields"]["triggers"]["0"]["type"][0], "x");
    }
}
