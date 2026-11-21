//! Shared task dispatch of the handler crates.
//!
//! Upstream calls `task.apply_async(...)`. In PaperRust there is exactly one path: the
//! built-in PostgreSQL queue (pl-queue). pl-server plugs a
//! [`Dispatcher`] onto the `PgQueue` at startup; handlers only call [`send_task`] (or, for the
//! transaction pattern, [`prepare_task`] + [`PreparedTask::record`] + [`PreparedTask::publish`])
//! and do not know pl-queue (pl-queue depends on pl-core).
//!
//! Also located here are the description of a task ([`TaskRequest`]) and the
//! `documents_paperlesstask` entry of tracked tasks ([`record_task`], like
//! `before_task_publish_handler` upstream). Celery/Redis no longer exist (2026-11-14).
//!
//! upstream: celery `Task.apply_async`, src/documents/signals/handlers.py@v3.4.2
//! (`before_task_publish_handler`, `TRACKED_TASKS`, `_extract_input_data`,
//! `_determine_trigger_source`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::sqlbind::SqlxBinder;
use chrono::{DateTime, Utc};
use sea_query::{Alias, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{ApiError, AppState};

/// Default queue. The name `celery` is historical (paperless-ngx `task_default_queue`)
/// and is stored as such in `paperrust_task_queue.queue`; renaming it would orphan enqueued tasks
/// on update.
pub const DEFAULT_QUEUE: &str = "celery";

/// Tasks for which upstream creates a `PaperlessTask` (`TRACKED_TASKS`): (name, task_type).
pub const TRACKED_TASKS: &[(&str, &str)] = &[
    ("documents.tasks.consume_file", "consume_file"),
    ("documents.tasks.train_classifier", "train_classifier"),
    ("documents.tasks.sanity_check", "sanity_check"),
    ("documents.tasks.llmindex_index", "llm_index"),
    ("documents.tasks.empty_trash", "empty_trash"),
    ("documents.tasks.check_scheduled_workflows", "check_workflows"),
    ("paperless_mail.tasks.process_mail_accounts", "mail_fetch"),
    ("documents.tasks.bulk_update_documents", "bulk_update"),
    ("documents.tasks.update_document_content_maybe_archive_file", "reprocess_document"),
    ("documents.tasks.build_share_link_bundle", "build_share_link"),
    ("documents.bulk_edit.delete", "bulk_delete"),
    // upstream dev 60a35edee (#14711)
    ("documents.bulk_operations.execute_bulk_operation", "bulk_update"),
    ("documents.tasks.apply_ai_suggestions", "apply_ai_suggestions"),
];

/// `PaperlessTask.TriggerSource` values.
pub mod trigger_source {
    pub const SCHEDULED: &str = "scheduled";
    pub const WEB_UI: &str = "web_ui";
    pub const API_UPLOAD: &str = "api_upload";
    pub const FOLDER_CONSUME: &str = "folder_consume";
    pub const EMAIL_CONSUME: &str = "email_consume";
    pub const SYSTEM: &str = "system";
    pub const MANUAL: &str = "manual";
    pub(super) const ALL: &[&str] = &[SCHEDULED, WEB_UI, API_UPLOAD, FOLDER_CONSUME, EMAIL_CONSUME, SYSTEM, MANUAL];
}

/// A task to be enqueued (corresponds to `task.apply_async(args, kwargs, headers=…, countdown=…)`).
#[derive(Debug, Clone, Default)]
pub struct TaskRequest {
    /// Full task name, e.g. `"documents.tasks.bulk_update_documents"`.
    pub name: String,
    pub args: Vec<Value>,
    pub kwargs: Map<String, Value>,
    /// Additional headers (`apply_async(headers=…)`), e.g. `trigger_source`.
    pub headers: Map<String, Value>,
    /// `countdown` in seconds.
    pub countdown: Option<f64>,
    /// `expires` in seconds from now.
    pub expires: Option<f64>,
    /// Fixed task ID (otherwise a new UUIDv4).
    pub task_id: Option<Uuid>,
    /// `(time_limit, soft_time_limit)` – upstream tasks do not set their own.
    pub time_limit: (Option<f64>, Option<f64>),
    pub ignore_result: bool,
}

impl TaskRequest {
    /// New task without arguments.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Default::default() }
    }
    /// Append a positional argument.
    pub fn arg(mut self, v: impl Into<Value>) -> Self {
        self.args.push(v.into());
        self
    }
    /// Set a keyword argument.
    pub fn kwarg(mut self, k: &str, v: impl Into<Value>) -> Self {
        self.kwargs.insert(k.to_string(), v.into());
        self
    }
    /// Header `trigger_source` (see [`trigger_source`]).
    pub fn trigger_source(mut self, source: &str) -> Self {
        self.headers.insert("trigger_source".into(), Value::String(source.into()));
        self
    }
    /// Any header.
    pub fn header(mut self, k: &str, v: impl Into<Value>) -> Self {
        self.headers.insert(k.to_string(), v.into());
        self
    }
}

/// Return value of [`TaskDispatch::dispatch`].
pub type DispatchFuture<'a> = Pin<Box<dyn Future<Output = Result<Uuid, ApiError>> + Send + 'a>>;

/// A way to enqueue tasks (object-safe).
pub trait TaskDispatch: Send + Sync {
    /// `apply_async` → task ID. If `req.task_id` is set, exactly this ID is used.
    fn dispatch(&self, req: TaskRequest) -> DispatchFuture<'_>;

    /// Does the dispatcher create the `PaperlessTask` of tracked tasks itself (PgQueue: yes)?
    /// If `false`, [`send_task`] creates it beforehand (or [`PreparedTask::record`] in the
    /// caller's transaction).
    fn records_task(&self) -> bool {
        true
    }
}

/// Plugged-in dispatcher (`state.set_service(Dispatcher(...))`, at startup by pl-server).
#[derive(Clone)]
pub struct Dispatcher(pub Arc<dyn TaskDispatch>);

/// The plugged-in dispatcher, if present.
pub fn dispatcher(state: &AppState) -> Option<Dispatcher> {
    state.service::<Dispatcher>()
}

fn no_queue(name: &str) -> ApiError {
    // Only reachable without pl-server (crate tests, tools without a queue).
    tracing::error!(task = name, "no task queue attached; task not queued");
    ApiError::internal(format!("no task queue attached (task {name})"))
}

/// `task.apply_async(...)` via the plugged-in dispatcher. Returns: task ID.
pub async fn send_task(state: &AppState, req: TaskRequest) -> Result<Uuid, ApiError> {
    let Some(Dispatcher(d)) = dispatcher(state) else {
        return Err(no_queue(&req.name));
    };
    let mut req = req;
    let task_id = *req.task_id.get_or_insert_with(Uuid::new_v4);
    if !d.records_task()
        && let Err(e) = record_task(state.db(), &req, task_id, Utc::now()).await
    {
        tracing::error!(error = %e, "Creating PaperlessTask failed");
    }
    d.dispatch(req).await
}

/// Prepared task for the pattern "create `PaperlessTask` in your own transaction,
/// enqueue after the commit" (upstream: `on_commit`).
pub struct PreparedTask {
    pub req: TaskRequest,
    pub task_id: Uuid,
    dispatcher: Dispatcher,
}

/// Set the task ID. Error if no queue is plugged in.
pub fn prepare_task(state: &AppState, req: TaskRequest) -> Result<PreparedTask, ApiError> {
    let Some(d) = dispatcher(state) else {
        return Err(no_queue(&req.name));
    };
    let mut req = req;
    let task_id = *req.task_id.get_or_insert_with(Uuid::new_v4);
    Ok(PreparedTask { req, task_id, dispatcher: d })
}

impl PreparedTask {
    /// Whether [`record`](Self::record) would create a row (dispatcher without own creation) –
    /// regardless of whether the task is tracked.
    pub fn records_in_caller(&self) -> bool {
        !self.dispatcher.0.records_task()
    }

    /// `before_task_publish_handler` in the caller's transaction/connection. If the
    /// dispatcher creates the `PaperlessTask` itself, nothing happens (`Ok(false)`).
    pub async fn record<'e, E>(&self, executor: E) -> Result<bool, ApiError>
    where
        E: sqlx::PgExecutor<'e>,
    {
        if !self.records_in_caller() {
            return Ok(false);
        }
        record_task(executor, &self.req, self.task_id, Utc::now()).await
    }

    /// After the commit: `dispatch` (possibly creates the `PaperlessTask` itself, idempotent via
    /// the fixed task ID).
    pub async fn publish(self, _state: &AppState) -> Result<Uuid, ApiError> {
        self.dispatcher.0.dispatch(self.req).await
    }
}

/// `before_task_publish_handler`: creates the `PaperlessTask` if the task is tracked
/// (`Ok(false)` otherwise). `executor` is the caller's pool, connection or transaction.
pub async fn record_task<'e, E>(executor: E, req: &TaskRequest, task_id: Uuid, now: DateTime<Utc>) -> Result<bool, ApiError>
where
    E: sqlx::PgExecutor<'e>,
{
    let Some(task_type) = tracked_task_type(&req.name) else {
        return Ok(false);
    };
    insert_paperless_task(executor, req, task_id, task_type, now).await?;
    Ok(true)
}

/// `TRACKED_TASKS.get(name)`.
pub fn tracked_task_type(name: &str) -> Option<&'static str> {
    TRACKED_TASKS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

/// `_determine_trigger_source(headers)`: valid header value, otherwise `manual`.
pub fn determine_trigger_source(headers: &Map<String, Value>) -> &'static str {
    headers
        .get("trigger_source")
        .and_then(Value::as_str)
        .and_then(|s| trigger_source::ALL.iter().find(|t| **t == s).copied())
        .unwrap_or(trigger_source::MANUAL)
}

/// `_extract_input_data(task_type, kwargs)` (`consume_file` is filled by pl-queue itself).
pub fn extract_input_data(task_type: &str, kwargs: &Map<String, Value>) -> Value {
    let mut m = Map::new();
    match task_type {
        "mail_fetch" => {
            if let Some(v) = kwargs.get("account_ids").filter(|v| !v.is_null()) {
                m.insert("account_ids".into(), v.clone());
            }
        }
        "apply_ai_suggestions" => {
            if let Some(v) = kwargs.get("document_id").filter(|v| !v.is_null()) {
                m.insert("document_id".into(), v.clone());
            }
        }
        _ => {}
    }
    Value::Object(m)
}

async fn insert_paperless_task<'e, E>(
    executor: E,
    req: &TaskRequest,
    task_id: Uuid,
    task_type: &str,
    now: DateTime<Utc>,
) -> Result<(), ApiError>
where
    E: sqlx::PgExecutor<'e>,
{
    let mut q = Query::insert();
    q.into_table(Alias::new("documents_paperlesstask")).columns([
        Alias::new("task_id"),
        Alias::new("task_type"),
        Alias::new("trigger_source"),
        Alias::new("status"),
        Alias::new("date_created"),
        Alias::new("input_data"),
        Alias::new("acknowledged"),
        Alias::new("owner_id"),
    ]);
    q.values([
        task_id.to_string().into(),
        task_type.into(),
        determine_trigger_source(&req.headers).into(),
        "pending".into(),
        now.into(),
        extract_input_data(task_type, &req.kwargs).into(),
        false.into(),
        Option::<i32>::None.into(),
    ])?;
    q.on_conflict(OnConflict::column(Alias::new("task_id")).do_nothing().to_owned());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(executor).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct Rec {
        sent: Mutex<Vec<TaskRequest>>,
        records: bool,
    }

    impl TaskDispatch for Rec {
        fn dispatch(&self, req: TaskRequest) -> DispatchFuture<'_> {
            let id = req.task_id.unwrap_or_else(Uuid::nil);
            if let Ok(mut s) = self.sent.lock() {
                s.push(req);
            }
            Box::pin(async move { Ok(id) })
        }
        fn records_task(&self) -> bool {
            self.records
        }
    }

    fn state() -> AppState {
        let cfg = crate::Config::from_env_map(crate::config::Env::from_pairs([(
            "PAPERLESS_SECRET_KEY",
            "pl-core-tests-only-0123456789abcdefghijklmnopqrstuvwxyz",
        )]))
        .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new().connect_lazy("postgres://u:p@127.0.0.1:1/x").unwrap();
        AppState::with_pool(cfg, pool).unwrap()
    }

    #[tokio::test]
    async fn send_task_uses_dispatcher_with_fixed_id() {
        let st = state();
        let rec = Arc::new(Rec { records: true, ..Default::default() });
        st.set_service(Dispatcher(rec.clone()));
        let req = TaskRequest::new("documents.tasks.train_classifier").trigger_source("manual");
        let id = send_task(&st, req).await.unwrap();
        let sent = rec.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].task_id, Some(id));
        assert_eq!(sent[0].headers["trigger_source"], "manual");
    }

    #[tokio::test]
    async fn prepared_task_with_recording_dispatcher_skips_record() {
        let st = state();
        st.set_service(Dispatcher(Arc::new(Rec { records: true, ..Default::default() })));
        let p = prepare_task(&st, TaskRequest::new("documents.tasks.consume_file")).unwrap();
        assert!(!p.records_in_caller());
        // would hit the (unreachable) pool if it tried to insert
        assert!(!p.record(st.db()).await.unwrap());
        let id = p.task_id;
        assert_eq!(p.publish(&st).await.unwrap(), id);
    }

    #[tokio::test]
    async fn without_queue_is_an_error() {
        let st = state();
        assert!(prepare_task(&st, TaskRequest::new("documents.tasks.train_classifier")).is_err());
        assert!(send_task(&st, TaskRequest::new("documents.tasks.train_classifier")).await.is_err());
    }

    #[tokio::test]
    async fn non_recording_dispatcher_records_in_caller() {
        let st = state();
        st.set_service(Dispatcher(Arc::new(Rec::default())));
        let p = prepare_task(&st, TaskRequest::new("documents.tasks.train_classifier")).unwrap();
        assert!(p.records_in_caller());
    }

    #[test]
    fn tracking_helpers() {
        assert_eq!(tracked_task_type("documents.bulk_edit.delete"), Some("bulk_delete"));
        assert_eq!(tracked_task_type("documents.tasks.index_optimize"), None);
        let mut h = Map::new();
        assert_eq!(determine_trigger_source(&h), "manual");
        h.insert("trigger_source".into(), json!("bogus"));
        assert_eq!(determine_trigger_source(&h), "manual");
        h.insert("trigger_source".into(), json!("web_ui"));
        assert_eq!(determine_trigger_source(&h), "web_ui");
        let mut kw = Map::new();
        kw.insert("account_ids".into(), json!([3]));
        assert_eq!(extract_input_data("mail_fetch", &kw), json!({"account_ids": [3]}));
        assert_eq!(extract_input_data("bulk_update", &kw), json!({}));
    }
}
