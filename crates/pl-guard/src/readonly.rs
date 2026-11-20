//! Read-only mode: process-wide switch, kept in sync from the database (several processes:
//! web and `--worker-only`), plus
//! * [`middleware`]: writing API requests → 423 with a clear message,
//! * [`gate`]: worker tasks that change files or data fail.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use pl_queue::{TaskContext, TaskError, TaskHandler};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::watch;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Message to clients (API, tasks). English like all API error texts; the UI translates.
pub const MESSAGE: &str = "PaperRust is in read-only mode because the ransomware guard detected suspicious changes in the media folder. \
An administrator must review the event and lift read-only mode (Administration → Protection).";

/// Error code in the JSON (`code`) so that clients recognise the state.
pub const CODE: &str = "read_only_mode";

/// Is read-only mode active (state of this process)?
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Set this process's switch (immediately after a DB change, without waiting for [`refresher`]).
pub fn set_local(on: bool) {
    let was = ACTIVE.swap(on, Ordering::Relaxed);
    if was != on {
        if on {
            tracing::error!(target: "paperless.guard", "read-only mode is ON: uploads, consumption and API writes are blocked");
        } else {
            tracing::warn!(target: "paperless.guard", "read-only mode lifted");
        }
    }
}

/// Read the state once from the DB and adopt it.
pub async fn refresh(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let s = crate::store::state(pool).await?;
    set_local(s.read_only);
    Ok(s.read_only)
}

/// Runs in every process: keeps the state in sync from the DB every 5 s.
pub async fn refresher(pool: PgPool, mut stop: watch::Receiver<bool>) {
    let mut warned = false;
    loop {
        match refresh(&pool).await {
            Ok(_) => warned = false,
            Err(e) if !warned => {
                tracing::warn!(target: "paperless.guard", error = %e, "could not read guard state");
                warned = true;
            }
            Err(_) => {}
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = stop.changed() => if *stop.borrow() { break },
        }
    }
}

/// May this request pass in read-only mode?
pub fn allowed(method: &Method, path: &str) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    let api = path.starts_with("/api/") || path.starts_with("/rs/api/");
    if !api {
        // login/logout, account pages, static files: no archive data
        return true;
    }
    const ALLOW: &[&str] = &[
        // checking and lifting the event
        "/api/x-guard/",
        // login via token, UI settings (display only, no archive data)
        "/api/token/",
        "/api/ui_settings/",
        "/rs/api/v2/auth/",
    ];
    ALLOW.iter().any(|p| path.starts_with(p))
}

/// Response for blocked write accesses.
pub fn blocked_response() -> Response {
    let body: Value = json!({"detail": MESSAGE, "code": CODE});
    (StatusCode::LOCKED, axum::Json(body)).into_response()
}

/// axum middleware (wrapped around all routes in pl-server, also paths without a route).
pub async fn middleware(req: Request, next: Next) -> Response {
    if is_active() && !allowed(req.method(), req.uri().path()) {
        return blocked_response();
    }
    next.run(req).await
}

/// Tasks that may also run in read-only mode (they only read or protect the archive).
pub const TASKS_ALLOWED: &[&str] = &[
    "documents.tasks.sanity_check",
    crate::TASK_BACKUP,
    "documents.tasks.index_optimize",
    "documents.tasks.clear_expired_sessions",
];

/// Returns `true` if a task is rejected in read-only mode.
pub fn task_blocked(name: &str) -> bool {
    is_active() && !TASKS_ALLOWED.contains(&name)
}

struct Gate {
    name: String,
    inner: Arc<dyn TaskHandler>,
}

impl TaskHandler for Gate {
    fn call(&self, ctx: TaskContext) -> BoxFuture<'static, Result<Value, TaskError>> {
        if task_blocked(&self.name) {
            let name = self.name.clone();
            return Box::pin(async move {
                tracing::warn!(target: "paperless.guard", task = %name, "task refused: read-only mode");
                Err(TaskError::fail("ReadOnlyMode", MESSAGE))
            });
        }
        self.inner.call(ctx)
    }
}

/// Wrap task handlers with a lock in read-only mode.
pub fn gate(name: &str, handler: impl TaskHandler + 'static) -> impl TaskHandler + 'static {
    Gate { name: name.to_string(), inner: Arc::new(handler) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list() {
        assert!(allowed(&Method::GET, "/api/documents/"));
        assert!(!allowed(&Method::POST, "/api/documents/post_document/"));
        assert!(!allowed(&Method::PATCH, "/api/documents/1/"));
        assert!(!allowed(&Method::DELETE, "/rs/api/v2/documents/1"));
        assert!(allowed(&Method::POST, "/api/x-guard/lift/"));
        assert!(allowed(&Method::POST, "/accounts/login/"));
        assert!(allowed(&Method::POST, "/api/token/"));
    }
}
