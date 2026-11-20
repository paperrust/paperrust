//! Domain errors. `pl-api-v2` maps them to the v2 error format
//! `{"error":{"code","message","fields"}}` (ARCHITECTURE-V2 §3).

use serde_json::{Map, Value};

#[derive(Debug, Clone, thiserror::Error)]
pub enum DomainError {
    /// Object is missing **or** not visible to the user (no difference to the outside).
    #[error("not found")]
    NotFound,
    /// Not signed in.
    #[error("authentication required")]
    Unauthenticated,
    /// Signed in, but without permission (model permission or object permission to change/delete).
    #[error("{0}")]
    Forbidden(String),
    /// Invalid input; `fields` = field name → list of messages.
    #[error("{message}")]
    Invalid { message: String, fields: Map<String, Value> },
    /// `If-Match` does not match the current `rev` (→ 412).
    #[error("revision mismatch (current rev {current})")]
    RevMismatch { current: i64 },
    /// Uniqueness violated or similar (→ 409).
    #[error("{0}")]
    Conflict(String),
    /// Cursor of the change feed too old / foreign server (→ 410).
    #[error("cursor expired")]
    CursorExpired,
    /// Required extension not installed (migration missing) (→ 503).
    #[error("{0} is not installed")]
    NotInstalled(&'static str),
    /// Function not (yet) available (→ 501).
    #[error("{0}")]
    NotImplemented(String),
    /// Zu viele Anfragen (Anmelde-Drossel) (→ 429), Wartezeit in Sekunden.
    #[error("request was throttled")]
    Throttled { wait: Option<u64> },
    /// Internal error (text only for the log).
    #[error("internal error: {0}")]
    Internal(String),
}

impl DomainError {
    pub fn invalid(message: impl Into<String>) -> Self {
        DomainError::Invalid { message: message.into(), fields: Map::new() }
    }

    pub fn field(field: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        let mut fields = Map::new();
        fields.insert(field.to_string(), Value::Array(vec![Value::String(message.clone())]));
        DomainError::Invalid { message, fields }
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        DomainError::Internal(e.to_string())
    }

    /// Stable error code in v2 format.
    pub fn code(&self) -> &'static str {
        match self {
            DomainError::NotFound => "not_found",
            DomainError::Unauthenticated => "not_authenticated",
            DomainError::Forbidden(_) => "permission_denied",
            DomainError::Invalid { .. } => "invalid",
            DomainError::RevMismatch { .. } => "precondition_failed",
            DomainError::Conflict(_) => "conflict",
            DomainError::CursorExpired => "cursor_expired",
            DomainError::NotInstalled(_) => "not_installed",
            DomainError::NotImplemented(_) => "not_implemented",
            DomainError::Throttled { .. } => "throttled",
            DomainError::Internal(_) => "internal",
        }
    }
}

pub type DomainResult<T> = Result<T, DomainError>;
