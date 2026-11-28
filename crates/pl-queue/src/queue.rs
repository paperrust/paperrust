//! Trait [`TaskQueue`] and shared types.
//!
//! upstream: celery/app/task.py@5.6.3 (`apply_async(args, kwargs, task_id, countdown, eta,
//! expires, headers, link, link_error, queue, priority)`), celery/canvas.py (`Signature`,
//! `.s()`/`.si()`, `group`, `chord(...).on_error(...)`).

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// Queue error.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("query build error: {0}")]
    Query(#[from] sea_query::error::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The implementation cannot do this (e.g. test queues without groups).
    #[error("unsupported by this queue backend: {0}")]
    Unsupported(String),
    #[error("invalid request: {0}")]
    Invalid(String),
}

/// Options like `apply_async(...)`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnqueueOptions {
    /// Positional arguments (`args=[...]`).
    pub args: Vec<Value>,
    /// Fixed task ID (otherwise UUIDv4). Enqueueing the same ID twice is idempotent.
    pub task_id: Option<Uuid>,
    /// Header `trigger_source` (siehe `pl_core::tasks::trigger_source`).
    pub trigger_source: Option<String>,
    /// Further headers.
    pub headers: Map<String, Value>,
    /// `countdown` in seconds.
    pub countdown: Option<f64>,
    /// `eta` (takes precedence over `countdown`).
    pub eta: Option<DateTime<Utc>>,
    /// `expires` in seconds from now.
    pub expires: Option<f64>,
    /// Queue (default `celery`).
    pub queue: Option<String>,
    /// Priority 0..=9, 0 = highest. Default 0.
    pub priority: Option<i16>,
    /// Hard time limit in seconds (default `PAPERLESS_WORKER_TIMEOUT`).
    pub time_limit: Option<f64>,
    /// `link=[sig, ...]`: after success with the result as first argument (unless `immutable`).
    pub link: Vec<Signature>,
    /// `link_error=[sig, ...]`: after final failure with `(request, exc, traceback)`.
    pub link_error: Vec<Signature>,
}

impl EnqueueOptions {
    pub fn trigger(source: &str) -> Self {
        Self { trigger_source: Some(source.to_string()), ..Default::default() }
    }
    /// All headers incl. `trigger_source`.
    pub fn all_headers(&self) -> Map<String, Value> {
        let mut h = self.headers.clone();
        if let Some(t) = &self.trigger_source {
            h.insert("trigger_source".into(), Value::String(t.clone()));
        }
        h
    }
}

/// `celery.canvas.Signature` (JSON-capable).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub task: String,
    #[serde(default)]
    pub args: Vec<Value>,
    #[serde(default)]
    pub kwargs: Map<String, Value>,
    /// `.si()`: the predecessor's result is **not** prepended.
    #[serde(default)]
    pub immutable: bool,
    /// Options (`.set(headers=..., queue=..., ...)`); `link`/`link_error` allowed in it.
    #[serde(default)]
    pub options: EnqueueOptions,
}

impl Signature {
    /// `task.s(**kwargs)`.
    pub fn new(task: impl Into<String>, kwargs: Map<String, Value>) -> Self {
        Self { task: task.into(), kwargs, ..Default::default() }
    }
    /// `task.si(**kwargs)`.
    pub fn immutable(task: impl Into<String>, kwargs: Map<String, Value>) -> Self {
        Self { immutable: true, ..Self::new(task, kwargs) }
    }
    /// `.set(headers={"trigger_source": ...})`.
    pub fn with_trigger(mut self, source: &str) -> Self {
        self.options.trigger_source = Some(source.to_string());
        self
    }
}

/// State of a task (Celery states).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Started,
    Retry,
    Success,
    Failure,
    Revoked,
}

impl TaskStatus {
    /// Celery-State-Text (`PENDING`, …).
    pub fn as_celery(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Started => "STARTED",
            Self::Retry => "RETRY",
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Revoked => "REVOKED",
        }
    }
    /// `PaperlessTask.Status` value (`RETRY` does not exist there → `started`).
    pub fn as_paperless(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Started | Self::Retry => "started",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Revoked => "revoked",
        }
    }
    /// `PaperlessTask.Status` → Status.
    pub fn from_paperless(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "started" => Self::Started,
            "success" => Self::Success,
            "failure" => Self::Failure,
            "revoked" => Self::Revoked,
            _ => return None,
        })
    }
    /// `READY_STATES`.
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Success | Self::Failure | Self::Revoked)
    }
}

/// Result of a status query.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskInfo {
    pub task_id: Uuid,
    /// Full task name (None if only known from `documents_paperlesstask`).
    pub task_name: Option<String>,
    pub status: TaskStatus,
    /// Return value (PgQueue only; JSON).
    pub result: Option<Value>,
    /// `{"error_type", "error_message", "traceback"?}` or `{"reason"}` for REVOKED.
    pub error: Option<Value>,
    pub retries: i32,
    pub date_done: Option<DateTime<Utc>>,
}

/// Task queue. Object-safe (`Arc<dyn TaskQueue>`).
pub trait TaskQueue: Send + Sync {
    /// `task.apply_async(kwargs=kwargs, **opts)` → task ID. Tracked tasks first get
    /// their `PaperlessTask` entry (`status=pending`).
    fn enqueue<'a>(
        &'a self,
        task_name: &'a str,
        kwargs: Map<String, Value>,
        opts: EnqueueOptions,
    ) -> BoxFuture<'a, Result<Uuid, QueueError>>;

    /// Enqueue inside the caller's PostgreSQL transaction (atomic with the caller's changes, e.g.
    /// the beat's "last run" record). Default: `Unsupported`; callers then fall back to
    /// [`Self::enqueue`].
    fn enqueue_in_conn<'a>(
        &'a self,
        conn: &'a mut sqlx::PgConnection,
        task_name: &'a str,
        kwargs: Map<String, Value>,
        opts: EnqueueOptions,
    ) -> BoxFuture<'a, Result<Uuid, QueueError>> {
        let _ = (conn, task_name, kwargs, opts);
        Box::pin(async { Err(QueueError::Unsupported("enqueue in transaction".into())) })
    }

    /// State (None = unknown).
    fn status(&self, task_id: Uuid) -> BoxFuture<'_, Result<Option<TaskInfo>, QueueError>>;

    /// `app.control.revoke(task_id, terminate=...)`. Returns: whether a task was affected.
    fn revoke(&self, task_id: Uuid, terminate: bool) -> BoxFuture<'_, Result<bool, QueueError>> {
        let _ = (task_id, terminate);
        Box::pin(async { Err(QueueError::Unsupported("revoke".into())) })
    }

    /// `group(sigs).delay()` → group ID.
    fn enqueue_group(&self, header: Vec<Signature>) -> BoxFuture<'_, Result<Uuid, QueueError>> {
        let _ = header;
        Box::pin(async { Err(QueueError::Unsupported("group".into())) })
    }

    /// `chord(header, body).on_error(on_error).delay()` → group ID.
    fn enqueue_chord(
        &self,
        header: Vec<Signature>,
        body: Signature,
        on_error: Option<Signature>,
    ) -> BoxFuture<'_, Result<Uuid, QueueError>> {
        let _ = (header, body, on_error);
        Box::pin(async { Err(QueueError::Unsupported("chord".into())) })
    }
}
