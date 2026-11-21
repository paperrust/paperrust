//! `/api/logs/` and `/api/logs/{pk}/`.
//!
//! upstream: src/documents/views.py@v3.4.2 (`LogViewSet.list`, `LogViewSet.retrieve`).
//! Permissions: `(IsAuthenticated, PaperlessAdminPermissions)`.

use std::path::{Path, PathBuf};

use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use pl_authz::AuthUser;
use pl_core::{ApiError, AppState, Drf, QueryParams, RequestContext};
use serde_json::{Value, json};

use crate::error::Result;
use crate::py;

/// `LogViewSet.ALLOWED_LOG_FILES` (dict order). PaperRust writes `paperrust.log` instead of
/// upstream's `paperless.log`; an older `paperless.log` (earlier PaperRust, paperless-ngx) stays
/// readable under its old key (Known difference: extra key `paperrust` first).
pub const ALLOWED_LOG_FILES: &[(&str, &str)] =
    &[("paperrust", "paperrust.log"), ("paperless", "paperless.log"), ("mail", "mail.log"), ("celery", "celery.log")];

/// `paperless` falls back to `paperrust.log` when no `paperless.log` exists, so clients that ask
/// for upstream's key (`/api/logs/paperless/`) keep working (parity rerun 2026-11-20).
fn log_file(dir: &Path, key: &str) -> Option<PathBuf> {
    let path = ALLOWED_LOG_FILES.iter().find(|(k, _)| *k == key).map(|(_, f)| dir.join(f))?;
    if key == "paperless" && !path.is_file() {
        return Some(dir.join("paperrust.log"));
    }
    Some(path)
}

/// Keys whose log file exists (`Path.is_file()`).
pub fn existing_logs(dir: &Path) -> Vec<&'static str> {
    ALLOWED_LOG_FILES.iter().filter(|(_, f)| dir.join(f).is_file()).map(|(k, _)| *k).collect()
}

/// Outcome of `retrieve` without HTTP concerns.
#[derive(Debug, PartialEq, Eq)]
pub enum LogResult {
    Lines(Vec<String>),
    NotFound,
    InvalidLimit,
    /// Python would raise (decode error, OverflowError, I/O error) → 500.
    Error(String),
}

/// `LogViewSet.retrieve` core.
pub fn read_log(dir: &Path, key: &str, limit_param: Option<&str>) -> LogResult {
    let Some(path) = log_file(dir, key) else { return LogResult::NotFound };
    if !path.is_file() {
        return LogResult::NotFound;
    }
    let limit = match limit_param {
        None => None,
        Some(raw) => match py::int(raw) {
            Some(n) if n >= 1 => Some(n),
            _ => return LogResult::InvalidLimit,
        },
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return LogResult::Error(format!("reading {}: {e}", path.display())),
    };
    let mut lines = match py::text_lines(&bytes) {
        Ok(l) => l,
        Err(e) => return LogResult::Error(format!("decoding {}: {e}", path.display())),
    };
    if let Some(n) = limit {
        // deque(maxlen=n): OverflowError beyond ssize_t
        if n > isize::MAX as i128 {
            return LogResult::Error("Python int too large to convert to C ssize_t".into());
        }
        let n = n as usize;
        if lines.len() > n {
            lines.drain(..lines.len() - n);
        }
    }
    LogResult::Lines(lines)
}

/// `GET /api/logs/`
pub async fn list(State(state): State<AppState>, _ctx: RequestContext, auth: AuthUser) -> Result<Response> {
    auth.require_admin()?;
    let dir = &state.config().logging_dir;
    Ok(Drf::ok(Value::from(existing_logs(dir))).into_response())
}

/// `GET /api/logs/{pk}/`
pub async fn retrieve(
    State(state): State<AppState>,
    _ctx: RequestContext,
    auth: AuthUser,
    UrlPath(pk): UrlPath<String>,
    query: QueryParams,
) -> Result<Response> {
    auth.require_admin()?;
    // Review 2 N16: file I/O off the runtime threads
    let dir = state.config().logging_dir.clone();
    let limit = query.get("limit").map(str::to_string);
    let result = tokio::task::spawn_blocking(move || read_log(&dir, &pk, limit.as_deref()))
        .await
        .unwrap_or_else(|e| LogResult::Error(format!("log reader task failed: {e}")));
    match result {
        LogResult::Lines(l) => Ok(Drf::ok(Value::from(l)).into_response()),
        LogResult::NotFound => Err(ApiError::not_found().into()),
        LogResult::InvalidLimit => Err(ApiError::validation(json!({"limit": "Must be a positive integer"})).into()),
        LogResult::Error(e) => Err(ApiError::internal(e).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paperless_key_falls_back_to_paperrust_log() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("paperrust.log"),
            "a
b
",
        )
        .unwrap();
        assert_eq!(read_log(dir.path(), "paperless", Some("1")), LogResult::Lines(vec!["b".into()]));
        assert_eq!(read_log(dir.path(), "paperless", Some("x")), LogResult::InvalidLimit);
    }

    #[test]
    fn list_and_read_like_upstream_tests() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mail.log"), "test\ntest2\n").unwrap();
        std::fs::write(dir.path().join("paperless.log"), "test1\ntest2\ntest3\n").unwrap();
        assert_eq!(existing_logs(dir.path()), vec!["paperless", "mail"]);
        std::fs::write(dir.path().join("paperrust.log"), "neu\n").unwrap();
        assert_eq!(existing_logs(dir.path()), vec!["paperrust", "paperless", "mail"]);
        assert_eq!(read_log(dir.path(), "paperrust", None), LogResult::Lines(vec!["neu".into()]));
        assert_eq!(read_log(dir.path(), "bogus_log", None), LogResult::NotFound);
        assert_eq!(read_log(dir.path(), "celery", None), LogResult::NotFound);
        assert_eq!(read_log(dir.path(), "paperless", None), LogResult::Lines(vec!["test1".into(), "test2".into(), "test3".into()]));
        assert_eq!(read_log(dir.path(), "paperless", Some("2")), LogResult::Lines(vec!["test2".into(), "test3".into()]));
        assert_eq!(read_log(dir.path(), "paperless", Some("abc")), LogResult::InvalidLimit);
        assert_eq!(read_log(dir.path(), "paperless", Some("-5")), LogResult::InvalidLimit);
        assert_eq!(read_log(dir.path(), "paperless", Some("0")), LogResult::InvalidLimit);
        assert_eq!(read_log(dir.path(), "paperless", Some("100")), LogResult::Lines(vec!["test1".into(), "test2".into(), "test3".into()]));
    }

    #[test]
    fn missing_dir() {
        assert!(existing_logs(Path::new("bogus_dir")).is_empty());
        assert_eq!(read_log(Path::new("bogus_dir"), "paperless", None), LogResult::NotFound);
    }
}
