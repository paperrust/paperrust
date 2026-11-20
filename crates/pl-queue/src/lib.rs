//! pl-queue – embedded task queue in PostgreSQL (the only way since 2026-11-14;
//! Celery/Redis no longer exist).
//!
//! See docs/ARCHITECTURE.md, docs/STANDALONE.md and docs/contract/pl-queue.md.
//!
//! * [`TaskQueue`]: `enqueue(task_name, kwargs, opts) -> task_id`, `status`, `revoke`, groups/chords.
//! * [`PgQueue`] + [`Worker`]: PostgreSQL queue (`paperrust_task_queue`,
//!   `FOR UPDATE SKIP LOCKED`, lease/heartbeat, retries, time limit, revocation).
//! * [`beat`]: scheduler (schedule as in paperless-ngx, crontab syntax of the `PAPERLESS_*_CRON` settings).
//! * [`paperless_task`]: `documents_paperlesstask` lifecycle like the upstream signal handlers.
//! * [`events`]: status messages (`status_updates`) as an in-process channel (+ optional NOTIFY).

pub mod backoff;
pub mod beat;
pub mod config;
pub mod cron;
pub mod events;
pub mod maintenance;
pub mod paperless_task;
pub mod pg;
pub mod queue;
pub mod registry;
pub mod worker;

pub use config::QueueConfig;
pub use events::{EventBus, ProgressManager, ProgressStatus, WsPayload};
pub use pg::{Failure, Job, PgQueue};
pub use queue::{EnqueueOptions, QueueError, Signature, TaskInfo, TaskQueue, TaskStatus};
pub use worker::{Handlers, TaskContext, TaskError, TaskHandler, Worker};

/// Default queue (historical name `celery`, see `pl_core::tasks::DEFAULT_QUEUE`).
pub const DEFAULT_QUEUE: &str = pl_core::tasks::DEFAULT_QUEUE;

/// Migrations (SQL files, expand-only). They are **not** run automatically.
pub mod migrations {
    /// `migrations/0001_paperrust_task_queue.sql`.
    pub const M0001_TASK_QUEUE: &str = include_str!("../migrations/0001_paperrust_task_queue.sql");

    /// All migrations in order `(name, sql)`.
    pub const ALL: &[(&str, &str)] = &[("0001_paperrust_task_queue", M0001_TASK_QUEUE)];

    /// Applies all migrations (idempotent). Call only deliberately (operator/tests).
    pub async fn apply(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        for (_, sql) in ALL {
            sqlx::raw_sql(sql).execute(pool).await?;
        }
        Ok(())
    }
}

/// Queue for callers without an attached service (tools, tests): a [`PgQueue`] on
/// the pool of `state`.
pub fn from_config(cfg: &QueueConfig, state: &pl_core::AppState) -> std::sync::Arc<dyn TaskQueue> {
    std::sync::Arc::new(PgQueue::new(state.db().clone(), cfg.max_deliveries))
}
